//! Browsing history (`<app data>/history.json`) and address-bar suggestions ranked from
//! history + bookmarks, roughly like Chrome's omnibox: prefix matches of the URL first, then
//! title / URL word matches, boosted by visit count, recency and bookmarks.

use crate::bookmarks::BookmarkStore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 5000;
/// Unsaved visits before the file is rewritten (also saved on exit).
const SAVE_EVERY: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub url: String,
    pub title: String,
    pub count: u32,
    /// Unix seconds of the last visit.
    pub last: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestKind {
    /// Go to / search for exactly what was typed.
    Typed,
    OpenTab,
    History,
    Bookmark,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub url: String,
    pub title: String,
    pub kind: SuggestKind,
}

pub struct History {
    visits: Vec<HistoryItem>,
    path: Option<PathBuf>,
    unsaved: usize,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// URL without scheme and `www.`, lowercased: what users type.
fn bare(url: &str) -> String {
    let lower = url.to_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    rest.strip_prefix("www.").unwrap_or(rest).to_string()
}

/// True if typing `query` should inline-complete to `url` (it's a prefix of the bare URL).
pub fn inline_match(query: &str, url: &str) -> bool {
    let q = query.trim().to_lowercase();
    !q.is_empty() && !q.contains(char::is_whitespace) && bare(url).starts_with(&q)
}

/// Precomputed lowercase query patterns so `suggest` doesn't allocate 3 strings per history item.
struct QueryPatterns<'a> {
    q: &'a str,
    dot: String,
    slash: String,
    space: String,
}

impl<'a> QueryPatterns<'a> {
    fn new(q: &'a str) -> Self {
        Self {
            q,
            dot: format!(".{q}"),
            slash: format!("/{q}"),
            space: format!(" {q}"),
        }
    }

    /// How well `self.q` (lowercase) matches; `None` if it doesn't.
    fn score(&self, url: &str, title: &str) -> Option<i64> {
        let b = bare(url);
        let t = title.to_lowercase();
        if b.starts_with(self.q) {
            Some(1000)
        } else if b.contains(&self.dot) || b.contains(&self.slash) || t.starts_with(self.q) || t.contains(&self.space) {
            Some(400)
        } else if t.contains(self.q) {
            Some(250)
        } else if b.contains(self.q) {
            Some(150)
        } else {
            None
        }
    }
}

impl History {
    pub fn load(path: Option<PathBuf>) -> Self {
        let visits = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            visits,
            path,
            unsaved: 0,
        }
    }

    /// Counts a visit to an http(s) page (fragment ignored).
    pub fn record(&mut self, url: &str, title: &str) {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return;
        }
        let url = url.split('#').next().unwrap_or(url);
        let ts = now();
        match self.visits.iter_mut().find(|v| v.url == url) {
            Some(v) => {
                v.count = v.count.saturating_add(1);
                v.last = ts;
                if !title.trim().is_empty() {
                    v.title = title.to_string();
                }
            }
            None => self.visits.push(HistoryItem {
                url: url.to_string(),
                title: title.to_string(),
                count: 1,
                last: ts,
            }),
        }
        // ponytail: linear scans over <= 5000 entries; index by URL if it ever shows up in profiles.
        if self.visits.len() > MAX_ENTRIES + MAX_ENTRIES / 10 {
            self.visits.sort_by_key(|v| std::cmp::Reverse(v.last));
            self.visits.truncate(MAX_ENTRIES);
        }
        self.unsaved += 1;
        if self.unsaved >= SAVE_EVERY {
            self.save();
        }
    }

    /// Updates the title of an already recorded page (JS pages set it after load).
    pub fn set_title(&mut self, url: &str, title: &str) {
        let url = url.split('#').next().unwrap_or(url);
        if let Some(v) = self.visits.iter_mut().find(|v| v.url == url)
            && v.title != title
            && !title.trim().is_empty()
        {
            v.title = title.to_string();
            self.unsaved += 1;
        }
    }

    /// Writes the file if anything changed (atomic temp-file + rename).
    pub fn save(&mut self) {
        if self.unsaved == 0 {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_string(&self.visits) else {
            return;
        };
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, json).is_err() {
            return;
        }
        if std::fs::rename(&tmp, path).is_ok() {
            self.unsaved = 0;
        }
    }

    /// Best `limit` history / bookmark matches for `query`, best first, one per URL.
    pub fn suggest(&self, bookmarks: &BookmarkStore, query: &str, limit: usize) -> Vec<Suggestion> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let pat = QueryPatterns::new(&q);
        let ts = now();
        let mut scored: Vec<(i64, Suggestion)> = Vec::new();
        for v in &self.visits {
            let Some(base) = pat.score(&v.url, &v.title) else {
                continue;
            };
            let age = ts.saturating_sub(v.last);
            let recency = match age {
                a if a < 86_400 => 200,
                a if a < 7 * 86_400 => 100,
                a if a < 30 * 86_400 => 50,
                _ => 0,
            };
            let score = base + i64::from(v.count.min(50)) * 20 + recency;
            scored.push((
                score,
                Suggestion {
                    url: v.url.clone(),
                    title: v.title.clone(),
                    kind: SuggestKind::History,
                },
            ));
        }
        for b in bookmarks.search(&q) {
            let Some(base) = pat.score(&b.url, &b.title) else {
                continue;
            };
            let score = base + 150;
            match scored.iter_mut().find(|(_, s)| s.url == b.url) {
                Some((s, sug)) => {
                    *s += 150;
                    sug.kind = SuggestKind::Bookmark;
                    if sug.title.trim().is_empty() {
                        sug.title = b.title.clone();
                    }
                }
                None => scored.push((
                    score,
                    Suggestion {
                        url: b.url,
                        title: b.title,
                        kind: SuggestKind::Bookmark,
                    },
                )),
            }
        }
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        scored.into_iter().take(limit).map(|(_, s)| s).collect()
    }

    /// Most recent `limit` visits, newest first.
    pub fn recent(&self, limit: usize) -> Vec<HistoryItem> {
        let mut items = self.visits.clone();
        items.sort_by_key(|v| std::cmp::Reverse(v.last));
        items.truncate(limit);
        items
    }

    /// Search history items by title/URL substring (empty query returns newest items).
    pub fn search_history(&self, query: &str, limit: usize) -> Vec<HistoryItem> {
        let q = query.trim().to_lowercase();
        let mut items: Vec<HistoryItem> = if q.is_empty() {
            self.visits.clone()
        } else {
            self.visits
                .iter()
                .filter(|v| v.url.to_lowercase().contains(&q) || v.title.to_lowercase().contains(&q))
                .cloned()
                .collect()
        };
        items.sort_by_key(|v| std::cmp::Reverse(v.last));
        items.truncate(limit);
        items
    }

    pub fn remove_url(&mut self, url: &str) -> bool {
        let before = self.visits.len();
        self.visits.retain(|v| v.url != url);
        if self.visits.len() != before {
            self.unsaved += 1;
            self.save();
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.visits.clear();
        self.unsaved += 1;
        self.save();
    }

    pub fn len(&self) -> usize {
        self.visits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::BookmarkItem;

    #[test]
    fn ranks_prefix_matches_frequency_and_bookmarks() {
        let mut h = History::load(None);
        h.record("https://github.com/sassoftware", "sassoftware · GitHub");
        for _ in 0..5 {
            h.record("https://www.google.com/search?q=git", "git - Google 검색");
        }
        h.record("https://news.ycombinator.com/item#c1", "Hacker News");
        h.record("rust://home", "새 탭");
        assert_eq!(h.visits.len(), 3, "fragment dropped, rust:// ignored");

        let bm = BookmarkStore {
            custom: vec![BookmarkItem {
                title: "GitLab".into(),
                url: "https://gitlab.com/".into(),
            }],
            ..Default::default()
        };
        let s = h.suggest(&bm, "git", 5);
        let urls: Vec<&str> = s.iter().map(|x| x.url.as_str()).collect();
        // URL-prefix matches beat a frequently visited title match; a visited page edges out
        // a never-visited bookmark.
        assert_eq!(
            urls[..3],
            [
                "https://github.com/sassoftware",
                "https://gitlab.com/",
                "https://www.google.com/search?q=git"
            ]
        );
        assert_eq!(s[1].kind, SuggestKind::Bookmark);
        assert!(
            h.suggest(&bm, "hacker", 5)[0]
                .url
                .starts_with("https://news.ycombinator.com/item")
        );
        assert!(h.suggest(&bm, "  ", 5).is_empty());

        assert!(inline_match("git", "https://www.github.com/x"));
        assert!(!inline_match("hub", "https://github.com/"));
        assert!(!inline_match("git hub", "https://github.com/"));
    }

    #[test]
    fn saves_and_reloads() {
        let path = std::env::temp_dir().join(format!("rb-history-{}.json", std::process::id()));
        let mut h = History::load(Some(path.clone()));
        h.record("https://a.test/", "A");
        h.set_title("https://a.test/", "A2");
        h.save();
        let h2 = History::load(Some(path.clone()));
        assert_eq!(h2.visits.len(), 1);
        assert_eq!(h2.visits[0].title, "A2");
        let _ = std::fs::remove_file(path);
    }
}
