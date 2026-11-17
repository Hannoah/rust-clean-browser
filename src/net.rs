use blitz_net::Provider as BlitzNetProvider;
use blitz_traits::net::{AbortSignal, Bytes, NetHandler, NetProvider, NetWaker, Request};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use url::Url;

const BLOCKED_TRACKER_DOMAINS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "google-analytics.com",
    "googletagmanager.com",
    "googletagservices.com",
    "googleadservices.com",
    "adservice.google.com",
    "pagead2.googlesyndication.com",
    "connect.facebook.net",
    "facebook.net",
    "pixel.facebook.com",
    "analytics.twitter.com",
    "ads-twitter.com",
    "analytics.tiktok.com",
    "hotjar.com",
    "static.hotjar.com",
    "mixpanel.com",
    "segment.io",
    "cdn.segment.com",
    "api.segment.io",
    "sentry.io",
    "browser.sentry-cdn.com",
    "newrelic.com",
    "nr-data.net",
    "scorecardresearch.com",
    "quantserve.com",
    "outbrain.com",
    "taboola.com",
    "criteo.com",
    "pubmatic.com",
    "rubiconproject.com",
    "openx.net",
    "clarity.ms",
    "bat.bing.com",
    "adnxs.com",
    "amazon-adsystem.com",
    "omtrdc.net",
    "demdex.net",
    "chartbeat.com",
    "pingdom.net",
    "optimizely.com",
    "fullstory.com",
];

pub fn is_blocked_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    // `Url` already lowercases hosts; compare suffixes without allocating per domain.
    BLOCKED_TRACKER_DOMAINS.iter().any(|domain| {
        host.strip_suffix(domain)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

/// Cookie jar persisted to disk so logins survive restarts (session cookies included,
/// like a browser's "continue where you left off"). Stored as JSON in the app data dir.
pub struct PersistentCookieJar {
    store: Mutex<cookie_store::CookieStore>,
    path: Option<PathBuf>,
    dirty: AtomicBool,
}

impl PersistentCookieJar {
    pub fn load(path: Option<PathBuf>) -> Self {
        let store = path
            .as_ref()
            .and_then(|p| std::fs::File::open(p).ok())
            .and_then(|f| cookie_store::serde::json::load(std::io::BufReader::new(f)).ok())
            .unwrap_or_default();
        Self {
            store: Mutex::new(store),
            path,
            dirty: AtomicBool::new(false),
        }
    }

    /// Writes the jar if it changed since the last save (atomic temp-file + rename).
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut buf = Vec::new();
        if let Ok(store) = self.store.lock() {
            if cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut buf).is_err() {
                return;
            }
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &buf).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

impl reqwest::cookie::CookieStore for PersistentCookieJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &http::HeaderValue>, url: &Url) {
        let cookies = headers
            .filter_map(|v| v.to_str().ok())
            .filter_map(|s| cookie_store::RawCookie::parse(s.to_owned()).ok());
        if let Ok(mut store) = self.store.lock() {
            store.store_response_cookies(cookies, url);
            self.dirty.store(true, Ordering::Release);
        }
    }

    fn cookies(&self, url: &Url) -> Option<http::HeaderValue> {
        let store = self.store.lock().ok()?;
        let header = store
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if header.is_empty() {
            None
        } else {
            http::HeaderValue::from_str(&header).ok()
        }
    }
}

pub struct CleanNetProvider {
    inner: Arc<BlitzNetProvider>,
    pub blocked_count: Arc<AtomicUsize>,
    pub fetched_count: Arc<AtomicUsize>,
    pub filter_enabled: AtomicBool,
    pub cookies: Arc<PersistentCookieJar>,
}

impl CleanNetProvider {
    pub fn new(waker: Option<Arc<dyn NetWaker>>, cookies: Arc<PersistentCookieJar>) -> Self {
        Self {
            inner: Arc::new(BlitzNetProvider::with_cookie_provider(waker, Arc::clone(&cookies))),
            cookies,
            blocked_count: Arc::new(AtomicUsize::new(0)),
            fetched_count: Arc::new(AtomicUsize::new(0)),
            filter_enabled: AtomicBool::new(true),
        }
    }

    pub fn is_filter_enabled(&self) -> bool {
        self.filter_enabled.load(Ordering::Relaxed)
    }

    pub fn toggle_filter(&self) -> bool {
        let prev = self.filter_enabled.fetch_xor(true, Ordering::Relaxed);
        !prev
    }

    /// Fetches a top-level document. `req` may be a GET or a form POST.
    pub fn fetch_document(
        &self,
        mut req: Request,
        signal: AbortSignal,
        on_done: impl FnOnce(String, Bytes) + Send + Sync + 'static,
        on_fail: impl FnOnce(String) + Send + Sync + 'static,
    ) {
        req.signal = Some(signal.clone());
        self.fetched_count.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch(
            0,
            req,
            Box::new(CallbackHandler {
                signal,
                on_done: Some(Box::new(on_done)),
                on_fail: Some(Box::new(on_fail)),
            }),
        );
    }

    pub fn blocked_total(&self) -> usize {
        self.blocked_count.load(Ordering::Relaxed)
    }
}

/// Ensures subresource handlers ALWAYS resolve (even on HTTP 4xx/5xx, timeout, or tracker block),
/// preventing `blitz-dom`'s `pending_critical_resources` from permanently blocking page paint.
struct SafeSubresourceHandler {
    url: String,
    inner: Option<Box<dyn NetHandler>>,
}

impl NetHandler for SafeSubresourceHandler {
    fn bytes(mut self: Box<Self>, resolved_url: String, bytes: Bytes) {
        if let Some(h) = self.inner.take() {
            h.bytes(resolved_url, bytes);
        }
    }
}

impl Drop for SafeSubresourceHandler {
    fn drop(&mut self) {
        if let Some(h) = self.inner.take() {
            h.bytes(std::mem::take(&mut self.url), Bytes::new());
        }
    }
}

impl NetProvider for CleanNetProvider {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url_str = request.url.to_string();
        if self.is_filter_enabled() && is_blocked_url(&request.url) {
            self.blocked_count.fetch_add(1, Ordering::Relaxed);
            // Resolve cleanly with empty bytes so critical resource counters never hang.
            handler.bytes(url_str, Bytes::new());
            return;
        }
        self.fetched_count.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch(
            doc_id,
            request,
            Box::new(SafeSubresourceHandler {
                url: url_str,
                inner: Some(handler),
            }),
        );
    }
}

struct CallbackHandler {
    signal: AbortSignal,
    on_done: Option<Box<dyn FnOnce(String, Bytes) + Send + Sync>>,
    on_fail: Option<Box<dyn FnOnce(String) + Send + Sync>>,
}

impl NetHandler for CallbackHandler {
    fn bytes(mut self: Box<Self>, resolved_url: String, bytes: Bytes) {
        self.on_fail = None;
        if let Some(cb) = self.on_done.take() {
            cb(resolved_url, bytes);
        }
    }
}

impl Drop for CallbackHandler {
    fn drop(&mut self) {
        if let Some(fail_cb) = self.on_fail.take() {
            if !self.signal.aborted() {
                fail_cb("네트워크 연결 실패, HTTP 오류 또는 타임아웃입니다.".to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_jar_round_trips_session_cookies_to_disk() {
        use reqwest::cookie::CookieStore;
        let path = std::env::temp_dir().join(format!("rb-cookies-{}.json", std::process::id()));
        let url = Url::parse("https://news.example.com/login").unwrap();
        let jar = PersistentCookieJar::load(Some(path.clone()));
        let session = http::HeaderValue::from_static("user=abc; Path=/; HttpOnly; Secure");
        let persistent = http::HeaderValue::from_static("pref=dark; Path=/; Max-Age=3600");
        jar.set_cookies(&mut [&session, &persistent].into_iter(), &url);
        jar.save();

        let reloaded = PersistentCookieJar::load(Some(path.clone()));
        let header = reloaded.cookies(&url).expect("cookies restored");
        let header = header.to_str().unwrap();
        assert!(header.contains("user=abc") && header.contains("pref=dark"), "{header}");
        assert!(reloaded.cookies(&Url::parse("https://other.example.org/").unwrap()).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn blocks_known_trackers_and_allows_content_hosts() {
        let tracker = Url::parse("https://www.google-analytics.com/analytics.js").unwrap();
        let sub_tracker = Url::parse("https://ad.doubleclick.net/pixel").unwrap();
        let normal = Url::parse("https://en.wikipedia.org/wiki/Rust").unwrap();
        let lookalike = Url::parse("https://notdoubleclick.net/").unwrap();
        assert!(!is_blocked_url(&lookalike));

        assert!(is_blocked_url(&tracker));
        assert!(is_blocked_url(&sub_tracker));
        assert!(!is_blocked_url(&normal));
    }
}
