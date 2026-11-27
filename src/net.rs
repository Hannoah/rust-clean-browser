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
    "facebook.net",
    "pixel.facebook.com",
    "analytics.twitter.com",
    "ads-twitter.com",
    "analytics.tiktok.com",
    "hotjar.com",
    "mixpanel.com",
    "segment.io",
    "cdn.segment.com",
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

/// True when a `file://` URL points inside the favicon cache, the only local files pages may load.
/// (Not the whole app-data dir: that holds cookies, history and the WebView2 profile.)
pub(crate) fn is_app_data_file_url(url: &Url) -> bool {
    let Ok(path) = url.to_file_path() else {
        return false;
    };
    // Reject `..` escapes: `starts_with` is component-wise and would otherwise
    // still match `base/../elsewhere`.
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    let Some(base) = crate::bookmarks::app_data_dir() else {
        return false;
    };
    path.starts_with(base.join("favicons2"))
}

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

/// At-rest protection for `cookies.json`: session cookies are login tokens, so on Windows the file is
/// wrapped with DPAPI (bound to the user account, like Chrome's cookie store). Other platforms use 0600.
// ponytail: macOS Keychain not wired; the 0600 file is the ceiling there.
pub(crate) mod seal {
    // Escaped on purpose: a literal newline here followed git's autocrlf setting, so shipped
    // Windows builds wrote CRLF. Builds from an LF checkout must still read those files.
    const MAGIC: &[u8] = b"RCBDPAPI1\r\n";
    const MAGIC_LF: &[u8] = b"RCBDPAPI1\n";

    /// Plain JSON (legacy files, non-Windows) passes through; sealed files are unwrapped.
    pub fn open(bytes: Vec<u8>) -> Option<Vec<u8>> {
        match bytes.strip_prefix(MAGIC).or_else(|| bytes.strip_prefix(MAGIC_LF)) {
            Some(blob) => os::unprotect(blob),
            None => Some(bytes),
        }
    }

    pub fn seal(json: Vec<u8>) -> Vec<u8> {
        match os::protect(&json) {
            Some(blob) => [MAGIC, &blob].concat(),
            None => json,
        }
    }

    #[cfg(windows)]
    mod os {
        use std::ffi::c_void;

        #[repr(C)]
        struct Blob {
            len: u32,
            data: *mut u8,
        }

        #[link(name = "crypt32")]
        unsafe extern "system" {
            fn CryptProtectData(
                i: *const Blob,
                d: *const u16,
                e: *const Blob,
                r: *mut c_void,
                p: *mut c_void,
                f: u32,
                o: *mut Blob,
            ) -> i32;
            fn CryptUnprotectData(
                i: *const Blob,
                d: *mut *mut u16,
                e: *const Blob,
                r: *mut c_void,
                p: *mut c_void,
                f: u32,
                o: *mut Blob,
            ) -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LocalFree(h: *mut c_void) -> *mut c_void;
        }

        const UI_FORBIDDEN: u32 = 0x1;

        fn run(input: &[u8], f: impl FnOnce(*const Blob, *mut Blob) -> i32) -> Option<Vec<u8>> {
            let inb = Blob {
                len: u32::try_from(input.len()).ok()?,
                data: input.as_ptr().cast_mut(),
            };
            let mut out = Blob {
                len: 0,
                data: std::ptr::null_mut(),
            };
            if f(&inb, &mut out) == 0 || out.data.is_null() {
                return None;
            }
            // SAFETY: on success the API returns `len` bytes at `data`, allocated with LocalAlloc.
            let v = unsafe { std::slice::from_raw_parts(out.data, out.len as usize) }.to_vec();
            unsafe { LocalFree(out.data.cast()) };
            Some(v)
        }

        pub fn protect(b: &[u8]) -> Option<Vec<u8>> {
            let n = std::ptr::null_mut();
            // SAFETY: pointers are valid for the call; optional params are null.
            run(b, |i, o| unsafe {
                CryptProtectData(i, std::ptr::null(), std::ptr::null(), n, n, UI_FORBIDDEN, o)
            })
        }

        pub fn unprotect(b: &[u8]) -> Option<Vec<u8>> {
            let n = std::ptr::null_mut();
            // SAFETY: as above.
            run(b, |i, o| unsafe {
                CryptUnprotectData(i, std::ptr::null_mut(), std::ptr::null(), n, n, UI_FORBIDDEN, o)
            })
        }
    }

    #[cfg(not(windows))]
    mod os {
        pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
            None
        }
        pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
            None
        }
    }
}

/// Writes `buf` to `path`, owner-only (0600) on Unix.
pub(crate) fn write_private(path: &std::path::Path, buf: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(buf)
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
            .and_then(|p| std::fs::read(p).ok())
            .and_then(seal::open)
            .and_then(|json| cookie_store::serde::json::load(json.as_slice()).ok())
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
        if !self.dirty.load(Ordering::Acquire) {
            return;
        }
        let mut buf = Vec::new();
        if let Ok(store) = self.store.lock() {
            if cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut buf).is_err() {
                return;
            }
        } else {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if write_private(&tmp, &seal::seal(buf)).is_err() {
            return;
        }
        if std::fs::rename(&tmp, path).is_ok() {
            // Only clear dirty after the data actually reached disk.
            self.dirty.store(false, Ordering::Release);
        }
    }
}

impl reqwest::cookie::CookieStore for PersistentCookieJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &http::HeaderValue>, url: &Url) {
        let mut cookies = headers
            .filter_map(|v| v.to_str().ok())
            .filter_map(|s| cookie_store::RawCookie::parse(s.to_owned()).ok())
            .peekable();
        if cookies.peek().is_none() {
            return;
        }
        if let Ok(mut store) = self.store.lock() {
            store.store_response_cookies(cookies, url);
            self.dirty.store(true, Ordering::Release);
        }
    }

    fn cookies(&self, url: &Url) -> Option<http::HeaderValue> {
        let store = self.store.lock().ok()?;
        // Drop only the offending pair instead of the whole header when one
        // value contains characters illegal in a HeaderValue.
        let mut pairs: Vec<String> = Vec::new();
        for (name, value) in store.get_request_values(url) {
            let pair = format!("{name}={value}");
            if http::HeaderValue::from_str(&pair).is_ok() {
                pairs.push(pair);
            }
        }
        if pairs.is_empty() {
            return None;
        }
        let mut header = pairs.join("; ");
        // Last-resort sanitization: strip non-visible-ASCII rather than dropping all cookies.
        if http::HeaderValue::from_str(&header).is_err() {
            header = header
                .chars()
                .filter(|c| matches!(*c, '\x20'..='\x7E') && *c != '\x7F')
                .collect();
        }
        http::HeaderValue::from_str(&header).ok()
    }
}

pub struct CleanNetProvider {
    inner: Arc<BlitzNetProvider>,
    pub blocked_count: Arc<AtomicUsize>,
    pub masked_failures: Arc<AtomicUsize>,
    pub filter_enabled: AtomicBool,
    pub cookies: Arc<PersistentCookieJar>,
}

impl CleanNetProvider {
    pub fn new(waker: Option<Arc<dyn NetWaker>>, cookies: Arc<PersistentCookieJar>) -> Self {
        Self {
            inner: Arc::new(BlitzNetProvider::with_cookie_provider(waker, Arc::clone(&cookies))),
            cookies,
            blocked_count: Arc::new(AtomicUsize::new(0)),
            masked_failures: Arc::new(AtomicUsize::new(0)),
            filter_enabled: AtomicBool::new(true),
        }
    }

    pub fn is_filter_enabled(&self) -> bool {
        self.filter_enabled.load(Ordering::Relaxed)
    }

    pub fn toggle_filter(&self) {
        self.filter_enabled.fetch_xor(true, Ordering::Relaxed);
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

    pub fn masked_failure_total(&self) -> usize {
        self.masked_failures.load(Ordering::Relaxed)
    }
}

/// Ensures subresource handlers ALWAYS resolve (even on HTTP 4xx/5xx, timeout, or tracker block),
/// preventing `blitz-dom`'s `pending_critical_resources` from permanently blocking page paint.
struct SafeSubresourceHandler {
    url: String,
    inner: Option<Box<dyn NetHandler>>,
    masked_failures: Arc<AtomicUsize>,
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
            // A subresource that never resolved (error/timeout/tracker-block):
            // counted so the UI can show degraded loads instead of failing silently.
            self.masked_failures.fetch_add(1, Ordering::Relaxed);
            h.bytes(std::mem::take(&mut self.url), Bytes::new());
        }
    }
}

impl NetProvider for CleanNetProvider {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url_str = request.url.to_string();
        // Subresource `file://` reads are confined to our own app-data dir (cached
        // favicons). Anything else is a local-file inclusion attempt by page markup.
        // Top-level `file://` navigation still goes through `fetch_document`.
        if request.url.scheme() == "file" && !is_app_data_file_url(&request.url) {
            handler.bytes(url_str, Bytes::new());
            return;
        }
        if self.is_filter_enabled() && is_blocked_url(&request.url) {
            self.blocked_count.fetch_add(1, Ordering::Relaxed);
            // Resolve cleanly with empty bytes so critical resource counters never hang.
            handler.bytes(url_str, Bytes::new());
            return;
        }
        self.inner.fetch(
            doc_id,
            request,
            Box::new(SafeSubresourceHandler {
                url: url_str,
                inner: Some(handler),
                masked_failures: Arc::clone(&self.masked_failures),
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
        if let Some(fail_cb) = self.on_fail.take()
            && !self.signal.aborted()
        {
            fail_cb("네트워크 연결 실패, HTTP 오류 또는 타임아웃입니다.".to_string());
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
        assert!(
            reloaded
                .cookies(&Url::parse("https://other.example.org/").unwrap())
                .is_none()
        );
        // Login tokens are not readable on disk on Windows (DPAPI); a legacy plaintext file still loads.
        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(cfg!(windows), !String::from_utf8_lossy(&on_disk).contains("user=abc"));
        // Files sealed by builds whose magic ended in a bare LF (LF checkout) still open.
        if cfg!(windows) {
            let lf = [b"RCBDPAPI1\n".as_slice(), &on_disk[b"RCBDPAPI1\r\n".len()..]].concat();
            assert!(String::from_utf8_lossy(&seal::open(lf).unwrap()).contains("user=abc"));
        }
        std::fs::write(&path, seal::open(on_disk).unwrap()).unwrap();
        assert!(PersistentCookieJar::load(Some(path.clone())).cookies(&url).is_some());
        // Empty Set-Cookie iteration must not mark the jar dirty.
        let jar2 = PersistentCookieJar::load(Some(path.clone()));
        jar2.set_cookies(&mut std::iter::empty(), &url);
        assert!(!jar2.dirty.load(Ordering::Acquire));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn confines_file_subresources_to_app_data() {
        let base = crate::bookmarks::app_data_dir().expect("app data dir");
        let inside = url::Url::from_file_path(base.join("favicons2").join("x.png")).unwrap();
        assert!(is_app_data_file_url(&inside));
        let outside = url::Url::parse("file:///C:/Windows/win.ini").unwrap();
        assert!(!is_app_data_file_url(&outside));
        let cookies = url::Url::from_file_path(base.join("cookies.json")).unwrap();
        assert!(!is_app_data_file_url(&cookies));
        let escape = url::Url::parse(&format!(
            "file://{}",
            base.join("..").join("other").to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
        assert!(!is_app_data_file_url(&escape));
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
