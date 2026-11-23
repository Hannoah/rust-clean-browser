//! On-disk cache of cleaned (script-free) page HTML for stale-while-revalidate navigation:
//! a previously visited page renders instantly from disk while the fresh copy is fetched in
//! the background. Files live in `<app data>/pages/<hash>.html`; the first line is the final
//! URL. Bounded by age and total size (oldest files evicted first).

use std::hash::{Hash, Hasher};
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
        let mut h = std::collections::hash_map::DefaultHasher::new();
        Self::cache_key(url).hash(&mut h);
        Some(self.dir.as_ref()?.join(format!("{:016x}.html", h.finish())))
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
        let (final_url, html) = rest.split_once('\n')?;
        // Verify the stored request key to survive 64-bit hash collisions.
        if req_key != Self::cache_key(url) {
            return None;
        }
        Some(CachedDoc {
            final_url: final_url.to_string(),
            html: Arc::from(html),
        })
    }

    pub fn put(&mut self, url: &str, final_url: &str, html: &str) {
        if html.len() > MAX_PAGE_BYTES || final_url.contains('\n') {
            return;
        }
        let req_key = Self::cache_key(url);
        if req_key.contains('\n') {
            return;
        }
        let Some(path) = self.path_for(url) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut buf = String::with_capacity(req_key.len() + final_url.len() + 2 + html.len());
        buf.push_str(&req_key);
        buf.push('\n');
        buf.push_str(final_url);
        buf.push('\n');
        buf.push_str(html);
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, &buf).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
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
            total += len;
            let expired = modified.elapsed().unwrap_or_default() > MAX_AGE;
            if expired || total > MAX_TOTAL_BYTES {
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
}
