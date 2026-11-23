//! Site favicons, cached on disk as `<app data>/favicons2/<key>.png|.svg` (raster icons are
//! shrunk to 64px; SVG is kept as-is and rendered by Blitz). UI code only renders `<img>` for
//! cached keys and falls back to the monogram badge otherwise.
//!
//! The icon is always the one the service declares (`<link rel=icon>`), like Chrome does:
//! - rendered pages hand over their links (`begin_fetch_known`), which also refreshes stale icons;
//! - unvisited URLs (bookmarks) fetch the page first to read its links (`begin_fetch`);
//! - `/favicon.ico` is only the last resort.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use url::Url;

const ICON_PX: u32 = 64;
/// Hosts without a usable icon are not retried for this long (`<key>.none` marker files).
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(3 * 24 * 3600);
/// Hosts serving several products with different icons under one domain: the first path
/// segment becomes part of the cache key (`/wiki` = Confluence, `/jira` = Jira, ...).
// ponytail: explicit list; add hosts here as mismatches show up.
const MULTI_APP_HOSTS: &[&str] = &["atlassian.net", "docs.google.com"];

/// One `<link>` element (from page HTML, or from a WebView via JSON).
#[derive(Debug, Default, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub sizes: String,
    #[serde(default, rename = "type")]
    pub ty: String,
}

#[derive(Debug)]
pub enum Job {
    /// Fetch the page and read its declared icon (see `discover`).
    Page { key: String, url: Url },
    /// Fetch an icon. `fallback` is tried if it fails; `guess` jobs mark the key as icon-less on failure.
    Icon {
        key: String,
        url: Url,
        fallback: Option<Url>,
        guess: bool,
    },
}

/// What to do after a fetch: done (`changed`: the cached icon changed), or run another job.
#[derive(Debug)]
// ponytail: one short-lived value per fetch; Box<Job> is not worth the churn.
#[allow(clippy::large_enum_variant)]
pub enum Next {
    Done { changed: bool },
    Then(Job),
}

struct State {
    dir: Option<PathBuf>,
    /// Keys with an icon file on disk -> file extension.
    ready: HashMap<String, &'static str>,
    /// Keys whose icon fetch failed recently (persisted as `<key>.none`).
    failed: HashSet<String>,
    /// Keys (or `key icon-url` pairs) already fetched this session; avoids retry storms.
    attempted: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        let base = crate::bookmarks::app_data_dir();
        // The v1 cache held guessed `/favicon.ico` files keyed by host only: drop it.
        if let Some(old) = base.as_ref().map(|d| d.join("favicons")) {
            let _ = std::fs::remove_dir_all(old);
        }
        let dir = base.map(|d| d.join("favicons2"));
        let mut ready = HashMap::new();
        let mut failed = HashSet::new();
        for entry in dir
            .as_ref()
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some((key, ext)) = name.rsplit_once('.') else {
                continue;
            };
            match ext {
                "png" => drop(ready.insert(key.to_owned(), "png")),
                "svg" => drop(ready.insert(key.to_owned(), "svg")),
                "none" => {
                    let fresh = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().unwrap_or_default() < RETRY_AFTER);
                    if fresh {
                        failed.insert(key.to_owned());
                    } else {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
                _ => {}
            }
        }
        Mutex::new(State {
            dir,
            ready,
            failed,
            attempted: HashSet::new(),
        })
    })
}

/// Cache key for a page URL: lowercase host without `www.` (plus the first path segment for
/// `MULTI_APP_HOSTS`), restricted to filename-safe characters.
fn cache_key(page: &Url) -> Option<String> {
    if !matches!(page.scheme(), "http" | "https") {
        return None;
    }
    let host = page.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return None;
    }
    let multi = MULTI_APP_HOSTS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")));
    let segment = page
        .path_segments()
        .and_then(|mut s| s.next())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    Some(match segment {
        Some(seg) if multi => format!("{host}~{seg}"),
        _ => host.to_string(),
    })
}

/// `file://` URL of the cached icon for this page, if any.
pub fn cached_icon_url(page_url: &str) -> Option<String> {
    let key = cache_key(&Url::parse(page_url).ok()?)?;
    let st = state().lock().ok()?;
    let ext = st.ready.get(&key)?;
    let path = st.dir.as_ref()?.join(format!("{key}.{ext}"));
    Url::from_file_path(path).ok().map(String::from)
}

/// For a URL whose content we don't have (e.g. a bookmark): the first time a key without a
/// cached icon is seen, fetch the page to find its declared icon.
pub fn begin_fetch(page_url: &str) -> Option<Job> {
    let url = Url::parse(page_url).ok()?;
    let key = cache_key(&url)?;
    let mut st = state().lock().ok()?;
    if st.ready.contains_key(&key) || st.failed.contains(&key) || !st.attempted.insert(key.clone()) {
        return None;
    }
    Some(Job::Page { key, url })
}

/// For a rendered page whose `<link>`s we know: fetch its declared icon once per session
/// (refreshing whatever is cached), or guess `/favicon.ico` if it declares none.
pub fn begin_fetch_known(page_url: &str, links: &[Link]) -> Option<Job> {
    let page = Url::parse(page_url).ok()?;
    let key = cache_key(&page)?;
    let ico = page.join("/favicon.ico").ok()?;
    let mut st = state().lock().ok()?;
    let have_icon = st.ready.contains_key(&key);
    match pick_icon(links, &page) {
        Some(icon) => {
            if !st.attempted.insert(format!("{key} {icon}")) {
                return None;
            }
            st.attempted.insert(key.clone());
            let fallback = (!have_icon && icon != ico).then_some(ico);
            Some(Job::Icon {
                key,
                url: icon,
                fallback,
                guess: !have_icon,
            })
        }
        None => {
            if have_icon || st.failed.contains(&key) || !st.attempted.insert(key.clone()) {
                return None;
            }
            Some(Job::Icon {
                key,
                url: ico,
                fallback: None,
                guess: true,
            })
        }
    }
}

/// Result of a `Job::Page` fetch: pick the page's declared icon, else `/favicon.ico`.
/// `bytes` is `None` when the page could not be fetched.
pub fn page_fetched(key: String, requested: &Url, final_url: &str, bytes: Option<&[u8]>) -> Job {
    let ico = requested.join("/favicon.ico").expect("http(s) URL");
    let declared = bytes.zip(Url::parse(final_url).ok()).and_then(|(bytes, page)| {
        // A sign-in redirect lands on another site whose icon isn't this service's.
        if !same_site(requested, &page) {
            return None;
        }
        let html = String::from_utf8_lossy(&bytes[..bytes.len().min(512 * 1024)]);
        pick_icon(&links_in_html(&html), &page)
    });
    match declared {
        Some(url) => {
            let fallback = (url != ico).then_some(ico);
            Job::Icon {
                key,
                url,
                fallback,
                guess: true,
            }
        }
        None => Job::Icon {
            key,
            url: ico,
            fallback: None,
            guess: true,
        },
    }
}

/// Result of a `Job::Icon` fetch (`bytes`: `None` on network/HTTP failure).
pub fn icon_fetched(key: String, fallback: Option<Url>, guess: bool, bytes: Option<&[u8]>) -> Next {
    if let Some(changed) = bytes.and_then(|b| store(&key, b)) {
        return Next::Done { changed };
    }
    match fallback {
        Some(url) => Next::Then(Job::Icon {
            key,
            url,
            fallback: None,
            guess,
        }),
        None => {
            if guess {
                mark_failed(&key);
            }
            Next::Done { changed: false }
        }
    }
}

// ponytail: compares the last two host labels; wrong for `co.kr`-style suffixes, where it is
// merely permissive. Use a public-suffix list if that ever matters.
fn same_site(a: &Url, b: &Url) -> bool {
    fn site(u: &Url) -> String {
        let host = u.host_str().unwrap_or("");
        let labels: Vec<&str> = host.rsplitn(3, '.').take(2).collect();
        labels.join(".")
    }
    site(a) == site(b)
}

/// Best icon among `links`, resolved against `page`: `rel=icon` over `apple-touch-icon`;
/// 32–96px raster first, then SVG / unsized, then larger, then 16px. Ties keep document order.
pub fn pick_icon(links: &[Link], page: &Url) -> Option<Url> {
    let mut best: Option<(u32, Url)> = None;
    for link in links {
        let rel = link.rel.to_ascii_lowercase();
        let is_icon = rel.split_ascii_whitespace().any(|r| r == "icon");
        let touch = rel.split_ascii_whitespace().any(|r| r.starts_with("apple-touch-icon"));
        let href = link.href.trim().replace("&amp;", "&");
        if !(is_icon || touch) || href.is_empty() {
            continue;
        }
        // Only allow http(s) icon URLs; data:/javascript:/blob: are never fetched.
        let h_lower = href.trim_start().to_ascii_lowercase();
        if h_lower.starts_with("data:")
            || h_lower.starts_with("javascript:")
            || h_lower.starts_with("blob:")
            || h_lower.starts_with("about:")
        {
            continue;
        }
        let Ok(url) = page.join(&href) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let svg = link.ty.contains("svg") || url.path().to_ascii_lowercase().ends_with(".svg");
        let size = link
            .sizes
            .split_ascii_whitespace()
            .filter_map(|s| s.to_ascii_lowercase().split_once('x')?.0.parse::<u32>().ok())
            .max();
        let fit = match size {
            _ if svg => 3,
            Some(n) if (32..=96).contains(&n) => 4,
            Some(n) if n > 96 => 2,
            Some(_) => 1,
            None => 3,
        };
        let score = if is_icon { 10 } else { 0 } + fit;
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, url));
        }
    }
    best.map(|(_, url)| url)
}

/// All `<link ...>` tags in `html` (attribute values are not entity-decoded beyond `&amp;`).
pub fn links_in_html(html: &str) -> Vec<Link> {
    // ASCII lowercasing keeps byte offsets, so indices into `lower` are valid for `html`.
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<link") {
        let start = pos + i + 5;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        pos = end;
        if !html[start..].starts_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let mut link = Link::default();
        for (name, value) in parse_attrs(&html[start..end]) {
            match name.as_str() {
                "rel" => link.rel = value,
                "href" => link.href = value,
                "sizes" => link.sizes = value,
                "type" => link.ty = value,
                _ => {}
            }
        }
        out.push(link);
    }
    out
}

/// `name=value` pairs of a tag body (names lowercased; values quoted, unquoted or empty).
fn parse_attrs(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = tag;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        let name_len = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        if name_len == 0 {
            return out;
        }
        let name = rest[..name_len].to_ascii_lowercase();
        rest = rest[name_len..].trim_start();
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (v, remaining) = match after.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let close = body.find(q).unwrap_or(body.len());
                    (&body[..close], body.get(close + 1..).unwrap_or(""))
                }
                _ => {
                    let end = after.find(|c: char| c.is_ascii_whitespace()).unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            value = v.to_string();
            rest = remaining;
        }
        out.push((name, value));
    }
}

/// Remembers (on disk) that `key` has no usable icon, so it isn't refetched every launch.
fn mark_failed(key: &str) {
    let Ok(mut st) = state().lock() else {
        return;
    };
    if st.ready.contains_key(key) {
        return;
    }
    if let Some(dir) = st.dir.clone() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{key}.none")), b"");
    }
    st.failed.insert(key.to_string());
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    (head.starts_with("<svg")
        || head.starts_with("<?xml")
        || head.starts_with("<!--")
        || head.starts_with("<!doctype svg"))
        && head.contains("<svg")
}

/// Maximum icon download accepted (decompression-bomb guard).
const MAX_ICON_BYTES: usize = 512 * 1024;

/// Stores an icon (sanitized SVG; ICO/PNG/JPEG/GIF/WebP/BMP resized to 64px PNG).
/// Returns `Some(changed)` on success, `None` if the bytes aren't a usable image.
fn store(key: &str, bytes: &[u8]) -> Option<bool> {
    if bytes.len() > MAX_ICON_BYTES {
        return None;
    }
    let (data, ext) = if looks_like_svg(bytes) {
        if bytes.len() > 128 * 1024 {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        (crate::cleaner::strip_scripts_and_bloat(text, true).into_bytes(), "svg")
    } else {
        let img = image::load_from_memory(bytes).ok()?;
        let img = if img.width() > ICON_PX || img.height() > ICON_PX {
            img.resize(ICON_PX, ICON_PX, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).ok()?;
        (png.into_inner(), "png")
    };
    let mut st = state().lock().ok()?;
    let dir = st.dir.clone()?;
    let path = dir.join(format!("{key}.{ext}"));
    if st.ready.get(key) == Some(&ext) && std::fs::read(&path).is_ok_and(|old| old == data) {
        return Some(false);
    }
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!("{key}.{ext}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &data).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    let other = if ext == "svg" { "png" } else { "svg" };
    let _ = std::fs::remove_file(dir.join(format!("{key}.{other}")));
    let _ = std::fs::remove_file(dir.join(format!("{key}.none")));
    st.failed.remove(key);
    st.ready.insert(key.to_string(), ext);
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn cache_keys_per_service() {
        assert_eq!(
            cache_key(&url("https://WWW.Example.com/a?b")).as_deref(),
            Some("example.com")
        );
        assert_eq!(
            cache_key(&url("https://x.atlassian.net/wiki/spaces/A")).as_deref(),
            Some("x.atlassian.net~wiki")
        );
        assert_eq!(
            cache_key(&url("https://x.atlassian.net/jira/")).as_deref(),
            Some("x.atlassian.net~jira")
        );
        assert_eq!(
            cache_key(&url("https://docs.google.com/spreadsheets/d/1")).as_deref(),
            Some("docs.google.com~spreadsheets")
        );
        assert_eq!(
            cache_key(&url("https://github.com/sassoftware")).as_deref(),
            Some("github.com")
        );
        assert_eq!(cache_key(&url("rust://home")), None);
        assert_eq!(cache_key(&url("file:///C:/x.html")), None);
    }

    #[test]
    fn parses_links_and_picks_the_declared_icon() {
        let html = r#"<head><LINK rel="stylesheet" href="a.css"><link rel=icon href="/fav16.png" sizes="16x16">
            <link rel="mask-icon" href="/mask.svg"><link rel='icon' type='image/png' sizes='32x32' href='/fav32.png?v=1&amp;x=2'/>
            <link rel="apple-touch-icon" href="/touch.png" sizes="180x180"><linkfoo rel=icon href=/no.png></head>"#;
        let links = links_in_html(html);
        assert_eq!(links.len(), 5);
        let page = url("https://a.test/dir/page");
        assert_eq!(
            pick_icon(&links, &page).unwrap().as_str(),
            "https://a.test/fav32.png?v=1&x=2"
        );
        // rel=icon wins over apple-touch-icon; SVG is accepted.
        let svg = links_in_html(r#"<link rel="apple-touch-icon" href="t.png"><link rel="icon" href="logo.svg">"#);
        assert_eq!(pick_icon(&svg, &page).unwrap().as_str(), "https://a.test/dir/logo.svg");
        assert!(
            pick_icon(
                &links_in_html(r#"<link rel="icon" href="data:image/png;base64,AA">"#),
                &page
            )
            .is_none()
        );
    }

    #[test]
    fn page_discovery_ignores_sign_in_redirects() {
        let html = br#"<link rel="icon" href="https://cdn.test/app.png">"#;
        let req = url("https://app.example.com/home");
        let Job::Icon {
            url: icon, fallback, ..
        } = page_fetched("k".into(), &req, "https://www.example.com/x", Some(html))
        else {
            panic!()
        };
        assert_eq!(icon.as_str(), "https://cdn.test/app.png");
        assert_eq!(fallback.unwrap().as_str(), "https://app.example.com/favicon.ico");
        let Job::Icon { url: icon, .. } = page_fetched("k".into(), &req, "https://login.other.test/", Some(html))
        else {
            panic!()
        };
        assert_eq!(icon.as_str(), "https://app.example.com/favicon.ico");
    }

    #[test]
    fn stores_png_and_svg_and_dedupes() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(128, 128, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let key = format!("store-test-{}.test", std::process::id());
        assert_eq!(store(&key, png.get_ref()), Some(true));
        assert_eq!(
            store(&key, png.get_ref()),
            Some(false),
            "identical icon is not a change"
        );
        let path = Url::parse(&cached_icon_url(&format!("https://{key}/")).unwrap())
            .unwrap()
            .to_file_path()
            .unwrap();
        let saved = image::open(&path).unwrap();
        assert_eq!((saved.width(), saved.height()), (64, 64));

        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)" viewBox="0 0 1 1"><script>alert(2)</script><rect width="1" height="1"/></svg>"#;
        assert_eq!(store(&key, svg), Some(true));
        let svg_url = cached_icon_url(&format!("https://{key}/")).unwrap();
        assert!(svg_url.ends_with(".svg") && !path.exists(), "svg replaces png");
        let svg_path = Url::parse(&svg_url).unwrap().to_file_path().unwrap();
        let saved_svg = std::fs::read_to_string(&svg_path).unwrap();
        assert!(!saved_svg.contains("<script") && !saved_svg.contains("onload") && saved_svg.contains("<rect"));
        let _ = std::fs::remove_file(svg_path);
        assert_eq!(store("bad.test", b"not an image"), None);
        assert!(looks_like_svg(
            br#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" ""><svg/>"#
        ));
    }

    #[test]
    fn failed_guesses_are_not_refetched() {
        let key = format!("fail-test-{}.test", std::process::id());
        let page = format!("https://{key}/");
        let job = begin_fetch(&page).unwrap();
        assert!(matches!(job, Job::Page { .. }));
        assert!(begin_fetch(&page).is_none(), "deduplicated per key");
        assert!(matches!(
            icon_fetched(key.clone(), None, true, None),
            Next::Done { changed: false }
        ));
        assert!(state().lock().unwrap().failed.contains(&key));
        if let Some(dir) = state().lock().unwrap().dir.clone() {
            let _ = std::fs::remove_file(dir.join(format!("{key}.none")));
        }
    }
}
