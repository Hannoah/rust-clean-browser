//! On-disk cache of cleaned (script-free) page HTML for stale-while-revalidate navigation:
//! a previously visited page renders instantly from disk while the fresh copy is fetched in
//! the background. Files live in `<app data>/pages/<hash>.html`; the first line is the final
//! URL. Bounded by age and total size (oldest files evicted first).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
const MAX_TOTAL_BYTES: u64 = 48 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

pub struct DiskPageCache {
    dir: Option<PathBuf>,
    writes_since_prune: usize,
}

pub struct CachedDoc {
    pub final_url: String,
    pub html: Arc<str>,
}

impl DiskPageCache {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            writes_since_prune: usize::MAX / 2, // prune on first write
        }
    }

    fn cache_key(url: &str) -> String {
        url.split('#').next().unwrap_or(url).to_string()
    }

    fn path_for(&self, url: &str) -> Option<PathBuf> {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in Self::cache_key(url).as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        Some(self.dir.as_ref()?.join(format!("{h:016x}.html")))
    }

    pub fn get(&self, url: &str) -> Option<CachedDoc> {
        let path = self.path_for(url)?;
        let meta = std::fs::metadata(&path).ok()?;
        // Bound allocation: put() caps pages at MAX_PAGE_BYTES; refuse oversized files.
        if meta.len() > (MAX_PAGE_BYTES as u64) + 4096 {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        let age = meta.modified().ok()?.elapsed().unwrap_or_default();
        if age > MAX_AGE {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        let (req_key, rest) = text.split_once('\n')?;
        let req_key = req_key.trim_end_matches('\r');
        let (final_url, html) = rest.split_once('\n')?;
        let final_url = final_url.trim_end_matches('\r');
        // Verify the stored request key and ensure `final_url` is a real hierarchical URL;
        // legacy 2-line entries (`url\nhtml`) put `<!DOCTYPE html>` on line 2, which would
        // fall back to a `data:` base_url in blitz-dom and panic on relative subresource URLs.
        let valid_final = url::Url::parse(final_url).ok().is_some_and(|u| !u.cannot_be_a_base());
        if req_key != Self::cache_key(url) {
            return None;
        }
        if !valid_final {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(CachedDoc {
            final_url: final_url.to_string(),
            html: Arc::from(html),
        })
    }

    pub fn put(&mut self, url: &str, final_url: &str, html: &str) {
        if html.len() > MAX_PAGE_BYTES || final_url.contains(['\r', '\n']) {
            return;
        }
        let Ok(parsed) = url::Url::parse(final_url) else {
            return;
        };
        if parsed.cannot_be_a_base() || parsed.scheme() == "file" {
            return;
        }
        let req_key = Self::cache_key(url);
        if req_key.contains(['\r', '\n']) {
            return;
        }
        let Some(path) = self.path_for(url) else {
            return;
        };
        let mut buf = String::with_capacity(req_key.len() + final_url.len() + 2 + html.len());
        buf.push_str(&req_key);
        buf.push('\n');
        buf.push_str(final_url);
        buf.push('\n');
        buf.push_str(html);
        let _ = crate::net::atomic_write_private(&path, buf.as_bytes());
        self.writes_since_prune += 1;
        if self.writes_since_prune >= 25 {
            self.writes_since_prune = 0;
            self.prune();
        }
    }

    /// Drops expired files, then the oldest ones until the cache fits `MAX_TOTAL_BYTES`.
    fn prune(&self) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                // Crashed-write leftovers must not count toward the budget.
                if path.extension().is_some_and(|x| x == "tmp") {
                    let _ = std::fs::remove_file(&path);
                    return None;
                }
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), path))
            })
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.0)); // newest first
        let mut total = 0u64;
        for (modified, len, path) in files {
            let expired = modified.elapsed().unwrap_or_default() > MAX_AGE;
            if expired {
                let _ = std::fs::remove_file(path);
                continue;
            }
            total += len;
            if total > MAX_TOTAL_BYTES {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_pages_and_ignores_fragments() {
        let dir = std::env::temp_dir().join(format!("rb-pages-{}", std::process::id()));
        let mut cache = DiskPageCache::new(Some(dir.clone()));
        assert!(cache.get("https://a.test/x").is_none());
        cache.put("https://a.test/x", "https://a.test/x/", "<p>hello</p>\n<p>2</p>");
        let doc = cache.get("https://a.test/x#section").expect("hit");
        assert_eq!(doc.final_url, "https://a.test/x/");
        assert_eq!(&*doc.html, "<p>hello</p>\n<p>2</p>");
        assert!(cache.get("https://a.test/y").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_and_evicts_legacy_two_line_cache_entry() {
        let dir = std::env::temp_dir().join(format!("rb-pages-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = DiskPageCache::new(Some(dir.clone()));
        let url = "https://example.test/auth";
        let path = cache.path_for(url).expect("cache path");
        std::fs::create_dir_all(&dir).unwrap();
        // Legacy 2-line format stored `<!DOCTYPE html>` as line 2.
        std::fs::write(&path, format!("{url}\n<!DOCTYPE html>\n<html></html>")).unwrap();
        assert!(path.exists());
        assert!(cache.get(url).is_none());
        assert!(!path.exists(), "stale/legacy entry should be deleted");
        let _ = std::fs::remove_dir_all(dir);
    }
}
