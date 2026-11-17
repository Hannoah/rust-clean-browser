//! Site favicons: fetched once per host, shrunk to a 64px PNG and cached on disk
//! (`<app data>/favicons/<host>.png`). UI code only renders `<img>` for hosts already cached,
//! and falls back to the monogram badge otherwise.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use url::Url;

const ICON_PX: u32 = 64;

struct State {
    dir: Option<PathBuf>,
    /// Hosts with an icon file on disk.
    ready: HashSet<String>,
    /// Hosts already fetched (or being fetched) this session; avoids retry storms.
    attempted: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        let dir = crate::bookmarks::app_data_dir().map(|d| d.join("favicons"));
        let ready = dir
            .as_ref()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str()?.strip_suffix(".png").map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Mutex::new(State {
            dir,
            ready,
            attempted: HashSet::new(),
        })
    })
}

/// Cache key for a page URL: its lowercase host, restricted to filename-safe characters.
fn host_key(page_url: &str) -> Option<String> {
    let url = Url::parse(page_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        .then_some(host)
}

/// `file://` URL of the cached icon for this page's host, if any.
pub fn cached_icon_url(page_url: &str) -> Option<String> {
    let host = host_key(page_url)?;
    let st = state().lock().ok()?;
    if !st.ready.contains(&host) {
        return None;
    }
    let path = st.dir.as_ref()?.join(format!("{host}.png"));
    Url::from_file_path(path).ok().map(String::from)
}

/// Decides whether to fetch an icon for `page_url`. Returns `(host, icon_url)` the first time
/// a host without a cached icon is seen. `declared` is the page's `<link rel=icon>` href.
pub fn begin_fetch(page_url: &str, declared: Option<&str>) -> Option<(String, Url)> {
    let host = host_key(page_url)?;
    {
        let mut st = state().lock().ok()?;
        if st.ready.contains(&host) || !st.attempted.insert(host.clone()) {
            return None;
        }
    }
    let page = Url::parse(page_url).ok()?;
    let icon = declared
        .filter(|href| !href.to_ascii_lowercase().contains(".svg") && !href.starts_with("data:"))
        .and_then(|href| page.join(href).ok())
        .or_else(|| page.join("/favicon.ico").ok())?;
    Some((host, icon))
}

/// Decodes (ICO/PNG/JPEG/GIF/WebP/BMP), resizes and stores an icon. Returns true on success.
pub fn store(host: &str, bytes: &[u8]) -> bool {
    let Ok(img) = image::load_from_memory(bytes) else {
        return false;
    };
    let img = if img.width() > ICON_PX || img.height() > ICON_PX {
        img.resize(ICON_PX, ICON_PX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut png = std::io::Cursor::new(Vec::new());
    if img.write_to(&mut png, image::ImageFormat::Png).is_err() {
        return false;
    }
    let Ok(mut st) = state().lock() else {
        return false;
    };
    let Some(dir) = st.dir.clone() else {
        return false;
    };
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::write(dir.join(format!("{host}.png")), png.into_inner()).is_err() {
        return false;
    }
    st.ready.insert(host.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_keys_and_icon_urls() {
        assert_eq!(host_key("https://WWW.Example.com/a?b").as_deref(), Some("www.example.com"));
        assert_eq!(host_key("rust://home"), None);
        assert_eq!(host_key("file:///C:/x.html"), None);

        let (host, icon) = begin_fetch("https://icons-a.test/page", Some("/static/fav.png")).unwrap();
        assert_eq!(host, "icons-a.test");
        assert_eq!(icon.as_str(), "https://icons-a.test/static/fav.png");
        assert!(begin_fetch("https://icons-a.test/other", None).is_none(), "deduplicated per host");

        let (_, icon) = begin_fetch("https://icons-b.test/x", Some("logo.svg")).unwrap();
        assert_eq!(icon.as_str(), "https://icons-b.test/favicon.ico");
    }

    #[test]
    fn decodes_and_stores_png_icon() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(128, 128, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let host = format!("store-test-{}.test", std::process::id());
        assert!(store(&host, png.get_ref()));
        let url = cached_icon_url(&format!("https://{host}/")).expect("cached");
        let path = Url::parse(&url).unwrap().to_file_path().unwrap();
        let saved = image::open(&path).unwrap();
        assert_eq!((saved.width(), saved.height()), (64, 64));
        let _ = std::fs::remove_file(path);
        assert!(!store("bad.test", b"not an image"));
    }
}
