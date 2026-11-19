use blitz_html::HtmlDocument;
use blitz_traits::net::{AbortController, Request};
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: String,
    pub scroll_y: f64,
    pub js_mode: bool,
    pub force_blitz: bool,
}

pub struct Tab {
    pub id: u64,
    pub load_seq: u64,
    pub title: String,
    pub url: String,
    pub scroll_y: f64,
    pub is_loading: bool,
    pub reader_mode: bool,
    pub js_mode: bool,
    pub force_blitz: bool,
    /// Page zoom, kept on the tab so it survives unload/reload.
    pub zoom: f32,
    /// Showing a disk-cached copy while the fresh page is fetched in the background.
    pub revalidating: bool,
    /// Pending non-GET navigation (form POST) consumed by the next load.
    pub pending_request: Option<Request>,
    pub history_back: Vec<HistoryEntry>,
    pub history_forward: Vec<HistoryEntry>,
    pub abort_controller: Option<AbortController>,
    /// Only the active tab holds a live `HtmlDocument`.
    /// Inactive tabs drop their DOM/layout/images immediately to minimize RAM.
    pub doc: Option<HtmlDocument>,
    /// Retained only while this tab is active (for instant Reader Mode toggle).
    pub raw_html: Option<Arc<str>>,
}

impl Tab {
    pub fn new(id: u64, url: String) -> Self {
        Self {
            id,
            load_seq: 0,
            title: short_title_for_url(&url),
            url,
            scroll_y: 0.0,
            is_loading: false,
            reader_mode: false,
            js_mode: false,
            force_blitz: false,
            zoom: 1.0,
            revalidating: false,
            pending_request: None,
            history_back: Vec::new(),
            history_forward: Vec::new(),
            abort_controller: None,
            doc: None,
            raw_html: None,
        }
    }

    pub fn cancel_inflight(&mut self) {
        self.load_seq = self.load_seq.wrapping_add(1);
        if let Some(ctrl) = self.abort_controller.take() {
            ctrl.abort();
        }
        self.is_loading = false;
        self.revalidating = false;
        self.pending_request = None;
    }

    /// Release DOM, layout trees, and raw HTML when this tab becomes inactive.
    /// Returns `true` if a live document was actually dropped.
    pub fn unload(&mut self) -> bool {
        let had_doc = self.doc.is_some();
        if let Some(doc) = &self.doc {
            self.scroll_y = doc.viewport_scroll().y;
        }
        self.cancel_inflight();
        self.doc = None;
        self.raw_html = None;
        had_doc
    }

    pub fn is_loaded(&self) -> bool {
        self.doc.is_some()
    }
}

pub fn short_title_for_url(url: &str) -> String {
    match url {
        "rust://home" => "새 탭".to_string(),
        u if u.starts_with("rust://bookmarks") => "북마크".to_string(),
        u if u.starts_with("rust://history") => "방문 기록".to_string(),
        _ => url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url)
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(url)
            .to_string(),
    }
}

#[derive(Clone)]
pub struct CachedPage {
    pub url: String,
    pub raw_html: Arc<str>,
}

/// Small bounded LRU cache of raw HTML strings so tab switching & Back/Forward
/// stay instant without keeping parsed DOM/layout/image memory allocated.
pub struct HtmlCache {
    entries: VecDeque<CachedPage>,
    max_entries: usize,
    max_total_bytes: usize,
}

impl HtmlCache {
    pub fn new(max_entries: usize, max_total_bytes: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries),
            max_entries,
            max_total_bytes,
        }
    }

    pub fn get(&mut self, url: &str) -> Option<CachedPage> {
        let pos = self.entries.iter().position(|e| e.url == url)?;
        let item = self.entries.remove(pos)?;
        self.entries.push_back(item.clone());
        Some(item)
    }

    pub fn insert(&mut self, url: String, raw_html: Arc<str>) {
        if raw_html.len() > self.max_total_bytes / 2 {
            return;
        }
        if let Some(pos) = self.entries.iter().position(|e| e.url == url) {
            self.entries.remove(pos);
        }
        self.entries.push_back(CachedPage { url, raw_html });
        while self.entries.len() > self.max_entries || self.total_bytes() > self.max_total_bytes {
            self.entries.pop_front();
        }
    }

    fn total_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|e| e.raw_html.len() + e.url.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_cache_evicts_oldest_by_entry_and_bytes() {
        let mut cache = HtmlCache::new(2, 100);
        cache.insert("https://a.test".into(), Arc::from("1234"));
        cache.insert("https://b.test".into(), Arc::from("5678"));
        assert!(cache.get("https://a.test").is_some());
        assert!(cache.get("https://b.test").is_some());

        cache.insert("https://c.test".into(), Arc::from("9012"));
        assert!(cache.get("https://a.test").is_none());
        assert!(cache.get("https://c.test").is_some());
    }
}
