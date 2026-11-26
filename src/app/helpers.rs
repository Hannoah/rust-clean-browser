//! Small free functions: window styling, URL input, key/pointer event construction.

use blitz_html::HtmlDocument;
use blitz_traits::events::{
    BlitzKeyEvent, BlitzPointerEvent, BlitzPointerId, BlitzWheelDelta, KeyState, MouseEventButton, MouseEventButtons,
    PointerCoords, PointerDetails,
};
use keyboard_types::{Code, Key, Location, Modifiers as KbtModifiers};
use std::time::Instant;
use url::Url;
use winit::cursor::CursorIcon;
use winit::event::{ButtonSource, MouseButton, MouseScrollDelta};
use winit::window::{ResizeDirection, WindowAttributes};
use wry::WebView;

/// Width (logical px) of the invisible resize frame around the frameless window.
pub(super) const RESIZE_EDGE: u32 = 5;

/// Text-editing / shortcut modifier: Cmd on macOS, Ctrl elsewhere (matches Blitz).
#[cfg(target_os = "macos")]
pub(super) const ACTION_MOD: KbtModifiers = KbtModifiers::META;
#[cfg(not(target_os = "macos"))]
pub(super) const ACTION_MOD: KbtModifiers = KbtModifiers::CONTROL;

/// True if the platform's primary shortcut modifier is held (Cmd on macOS, Ctrl elsewhere).
pub(super) fn primary_modifier(state: winit::keyboard::ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        state.meta_key()
    } else {
        state.control_key()
    }
}

/// Gives the main window keyboard focus (`activate`: also bring it to the front).
///
/// winit's `focus_window` only activates the window. Once a child WebView had focus (or was
/// destroyed while focused), Windows delivers keys to an active window without a focused
/// control as `WM_SYSCHAR`, and every keystroke plays the error "ding". `SetFocus` fixes that.
pub(super) fn focus_main_window(win: &dyn winit::window::Window, activate: bool) {
    if activate {
        win.focus_window();
    }
    #[cfg(target_os = "windows")]
    {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetFocus(hwnd: isize) -> isize;
            fn GetForegroundWindow() -> isize;
        }
        if let Ok(handle) = win.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            let hwnd = h.hwnd.get();
            // Never steal focus from another app when we weren't asked to activate.
            unsafe {
                if activate || GetForegroundWindow() == hwnd {
                    SetFocus(hwnd);
                }
            }
        }
    }
}

/// True when the main winit window itself currently owns OS keyboard focus (not a child WebView2
/// HWND or another application). Used so clicking `#url-bar` after interacting with a JS-mode
/// WebView2 page is reliably treated as an unfocused first click (select-all).
pub(super) fn main_window_has_keyboard_focus(win: &dyn winit::window::Window) -> bool {
    #[cfg(target_os = "windows")]
    {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetFocus() -> isize;
        }
        if let Ok(handle) = win.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            return unsafe { GetFocus() == h.hwnd.get() };
        }
        true
    }
    #[cfg(not(target_os = "windows"))]
    {
        win.has_focus()
    }
}

/// Dangerous executable / script extensions that must never auto-download or auto-launch.
pub(super) fn is_dangerous_download_ext(filename: &str) -> bool {
    let trimmed = filename.trim().trim_end_matches('.');
    let Some((_, ext)) = trimmed.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe"
            | "msi"
            | "msp"
            | "bat"
            | "cmd"
            | "com"
            | "pif"
            | "scr"
            | "ps1"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "hta"
            | "lnk"
            | "reg"
            | "dll"
            | "cpl"
            | "jar"
            | "app"
            | "command"
            | "sh"
            // Packaged apps, disk images (bypass Mark-of-the-Web), and other shell-launchable formats.
            | "msix"
            | "msixbundle"
            | "appx"
            | "appxbundle"
            | "appinstaller"
            | "iso"
            | "img"
            | "vhd"
            | "vhdx"
            | "chm"
            | "url"
            | "scf"
            | "xll"
            | "psm1"
            | "psd1"
            | "settingcontent-ms"
            | "application"
            | "gadget"
            | "inf"
            | "sct"
            | "wsc"
    )
}

/// Best-effort human-readable filename from a download path or URL.
pub(super) fn download_display_name(path: Option<&std::path::Path>, url: &str) -> String {
    if let Some(name) = path
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
    if let Ok(parsed) = Url::parse(url)
        && let Some(seg) = parsed.path_segments().and_then(|mut s| s.next_back())
        && !seg.is_empty()
    {
        return url::form_urlencoded::parse(format!("k={seg}").as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| seg.to_string());
    }
    "download".to_string()
}

/// Opens a completed non-executable downloaded file with the OS default application.
pub(super) fn open_downloaded_file(path: &std::path::Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && is_dangerous_download_ext(name)
    {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: isize,
                lp_operation: *const u16,
                lp_file: *const u16,
                lp_parameters: *const u16,
                lp_directory: *const u16,
                n_show_cmd: i32,
            ) -> isize;
        }
        let op: Vec<u16> = "open\0".encode_utf16().collect();
        let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let res = unsafe { ShellExecuteW(0, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        res > 32
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn().is_ok()
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().is_ok()
    }
}

/// Reveals a downloaded file in the OS file manager (Explorer / Finder), selecting it.
pub(super) fn reveal_in_file_manager(path: &std::path::Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        if path.exists() {
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(path.as_os_str());
            return std::process::Command::new("explorer.exe").arg(arg).spawn().is_ok();
        }
        if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
            return std::process::Command::new("explorer.exe").arg(parent).spawn().is_ok();
        }
        open_downloads_folder()
    }
    #[cfg(target_os = "macos")]
    {
        if path.exists() {
            std::process::Command::new("open").arg("-R").arg(path).spawn().is_ok()
        } else {
            open_downloads_folder()
        }
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let dir = path.parent().filter(|p| p.is_dir());
        if let Some(d) = dir {
            std::process::Command::new("xdg-open").arg(d).spawn().is_ok()
        } else {
            open_downloads_folder()
        }
    }
}

/// Opens the user's Downloads folder in the OS file manager.
pub(super) fn open_downloads_folder() -> bool {
    let dir = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| std::path::PathBuf::from(h).join("Downloads"));
    let Some(dir) = dir.filter(|d| d.is_dir()) else {
        return false;
    };
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe").arg(dir).spawn().is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(dir).spawn().is_ok()
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(dir).spawn().is_ok()
    }
}

/// Lets cached `file://` favicons decode before a freshly built UI document is shown, so
/// icons don't flicker on every chrome rebuild. Bounded to 25 ms, and skips icon URLs that
/// previously failed to decode so corrupt cached files never stall the UI thread.
pub(super) fn wait_for_images(doc: &mut HtmlDocument) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static BROKEN_SRCS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let broken = BROKEN_SRCS.get_or_init(|| Mutex::new(HashSet::new()));

    let deadline = Instant::now() + std::time::Duration::from_millis(25);
    loop {
        doc.handle_messages();
        let pending_srcs: Vec<String> = {
            let guard = broken.lock().ok();
            doc.tree()
                .iter()
                .filter_map(|(_, node)| {
                    let el = node.element_data()?;
                    if &*el.name.local != "img" || el.image_data().is_some() {
                        return None;
                    }
                    let src = el.attr(blitz_dom::LocalName::from("src")).unwrap_or_default();
                    if guard.as_ref().is_some_and(|set| set.contains(src)) {
                        return None;
                    }
                    Some(src.to_string())
                })
                .collect()
        };
        if pending_srcs.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            if let Ok(mut guard) = broken.lock() {
                // Bounded: forgetting old entries only costs one more 25 ms wait per icon.
                if guard.len() > 1024 {
                    guard.clear();
                }
                for src in pending_srcs {
                    if !src.is_empty() {
                        guard.insert(src);
                    }
                }
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Screen position (physical px) of the main window's client-area origin.
///
/// On Windows this asks the OS directly (`ClientToScreen`), which stays correct after
/// Win+Arrow snapping, DPI changes and invisible resize borders; `surface_position()` can
/// lag behind in those cases and made popups open in the wrong place.
pub(super) fn client_origin_on_screen(win: &dyn winit::window::Window) -> winit::dpi::PhysicalPosition<i32> {
    #[cfg(target_os = "windows")]
    {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn ClientToScreen(hwnd: isize, point: *mut Point) -> i32;
        }
        if let Ok(handle) = win.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            let mut p = Point { x: 0, y: 0 };
            if unsafe { ClientToScreen(h.hwnd.get(), &mut p) } != 0 {
                return winit::dpi::PhysicalPosition::new(p.x, p.y);
            }
        }
    }
    win.surface_position()
}

/// Returns whether the primary mouse button is currently held down at the OS level.
#[cfg(target_os = "windows")]
pub(super) fn is_primary_mouse_button_down() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(vkey: i32) -> i16;
        fn GetSystemMetrics(index: i32) -> i32;
    }
    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    const SM_SWAPBUTTON: i32 = 23;
    unsafe {
        let vkey = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        GetAsyncKeyState(vkey) < 0
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn is_primary_mouse_button_down() -> bool {
    true
}

/// The JS-mode WebView is destroyed when evicted from the parked LRU pool, which would drop its
/// session cookies (i.e. log the user out). Promote them to 30-day persistent cookies first so
/// logins survive tab eviction and restarts.
pub(super) fn persist_session_cookies(wv: &WebView) {
    let Ok(cookies) = wv.cookies() else {
        return;
    };
    for cookie in cookies {
        if cookie.expires_datetime().is_none() && cookie.max_age().is_none() {
            let mut kept = cookie.clone();
            kept.set_max_age(wry::cookie::time::Duration::days(30));
            let _ = wv.set_cookie(&kept);
        }
    }
}

/// Freezes a hidden background WebView (`TrySuspend` + `MemoryUsageTargetLevel::Low` on Windows)
/// so keeping up to 5 parked JS tabs uses minimal working-set memory and 0% CPU.
pub(super) fn suspend_webview(wv: &WebView) {
    #[cfg(target_os = "windows")]
    {
        use wry::{MemoryUsageLevel, WebViewExtWindows};
        let _ = wv.set_memory_usage_level(MemoryUsageLevel::Low);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = wv;
}

/// Wakes a parked WebView back to normal execution and memory target level before showing it.
pub(super) fn resume_webview(wv: &WebView) {
    #[cfg(target_os = "windows")]
    {
        use wry::{MemoryUsageLevel, WebViewExtWindows};
        let _ = wv.set_memory_usage_level(MemoryUsageLevel::Normal);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = wv;
}

/// Should `window.open` get a real popup window (keeps `window.opener` / `window.close()`) instead
/// of a new tab? Sign-in popups talk back to their opener, so a detached tab ends up blank.
pub(super) fn wants_native_popup(url: &str, has_window_features: bool) -> bool {
    const AUTH_HOSTS: &[&str] = &[
        "accounts.google.com",
        "login.microsoftonline.com",
        "login.live.com",
        "appleid.apple.com",
        "kauth.kakao.com",
        "nid.naver.com",
        "api.twitter.com",
        "discord.com",
    ];
    if has_window_features || url == "about:blank" {
        return true;
    }
    let Some(host) = Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)) else {
        return false;
    };
    AUTH_HOSTS.contains(&host.as_str())
        || (host == "github.com" && url.contains("/login/oauth"))
        || (host.ends_with("facebook.com") && url.contains("/dialog/"))
}

/// Hosts whose sign-in / app flows only work with a real JS engine and the Windows account SSO
/// (Microsoft Entra Conditional Access checks device compliance via WebView2's OS-account SSO).
pub(super) fn requires_js_engine(url: &str) -> bool {
    // PDFs are shown by WebView2's built-in viewer (search, zoom, thumbnails, print, annotate).
    if crate::pdfjs::is_pdf_url(url) {
        return true;
    }
    const JS_HOSTS: &[&str] = &[
        "login.microsoftonline.com",
        "login.microsoft.com",
        "login.live.com",
        "microsoft365.com",
        "office.com",
        "cloud.microsoft",
        "copilot.microsoft.com",
        "sharepoint.com",
        "teams.microsoft.com",
        "outlook.office.com",
        "accounts.google.com",
        // UA-sniffs and redirects Blitz to /unsupported_browser; JS-only SPA anyway.
        "highspot.com",
        // GitHub uses <react-app>/<react-partial> custom elements, Primer @layer CSS, and
        // multi-theme stylesheets that require JS hydration to render code/file trees properly.
        "github.com",
        "gist.github.com",
        // Naver services (www, mail, nid, cafe, blog, pay) share login cookies in WebView2;
        // fetching mail.naver.com via Blitz first lacks WebView2 cookies and bounces to nidlogin.
        "naver.com",
        // Google Search no longer serves results without JavaScript.
        "www.google.com",
    ];
    let Some(host) = Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)) else {
        return false;
    };
    JS_HOSTS.iter().any(|d| {
        host.strip_suffix(d)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

pub(super) fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
        ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
    }
}

pub(super) fn synthetic_key(key: Key, code: Code, modifiers: KbtModifiers) -> BlitzKeyEvent {
    BlitzKeyEvent {
        key,
        code,
        modifiers,
        location: Location::Standard,
        is_auto_repeating: false,
        is_composing: false,
        state: KeyState::Pressed,
        text: None,
    }
}

pub(super) fn pointer_event(
    id: BlitzPointerId,
    is_primary: bool,
    coords: PointerCoords,
    button: MouseEventButton,
    buttons: MouseEventButtons,
    mods: KbtModifiers,
    details: PointerDetails,
) -> BlitzPointerEvent {
    BlitzPointerEvent {
        id,
        is_primary,
        coords,
        button,
        buttons,
        mods,
        details,
        element: Default::default(),
        active_pointers: Default::default(),
    }
}

pub(super) fn mouse_button(button: &ButtonSource) -> MouseEventButton {
    match button {
        ButtonSource::Mouse(MouseButton::Left) => MouseEventButton::Main,
        ButtonSource::Mouse(MouseButton::Right) => MouseEventButton::Secondary,
        ButtonSource::Mouse(_) => MouseEventButton::Auxiliary,
        _ => MouseEventButton::Main,
    }
}

pub(super) fn wheel_delta(delta: MouseScrollDelta) -> BlitzWheelDelta {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => BlitzWheelDelta::Lines(x as f64, y as f64),
        MouseScrollDelta::PixelDelta(p) => BlitzWheelDelta::Pixels(p.x, p.y),
        _ => BlitzWheelDelta::Pixels(0.0, 0.0),
    }
}

/// Pointer coordinates for a document drawn at the window's top-left (no zoom / offset).
pub(super) fn plain_coords(x: f32, y: f32, scroll_x: f32, scroll_y: f32) -> PointerCoords {
    PointerCoords {
        screen_x: x,
        screen_y: y,
        client_x: x,
        client_y: y,
        page_x: x + scroll_x,
        page_y: y + scroll_y,
    }
}

/// 64x64 RGBA app icon generated by `assets/make_icon.py`.
pub(super) fn app_icon() -> Option<winit::icon::Icon> {
    const RGBA: &[u8] = include_bytes!("../../assets/icon_64.rgba");
    winit::icon::RgbaIcon::new(RGBA.to_vec(), 64, 64).ok().map(Into::into)
}

/// Windows: frameless (our own title bar/buttons) with Windows 11 shadow + rounded corners.
/// macOS: native traffic lights over a transparent, full-size-content title bar.
/// Other platforms: native decorations.
pub(super) fn main_window_style(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::{CornerPreference, WindowAttributesWindows};
        attrs.with_decorations(false).with_platform_attributes(Box::new(
            WindowAttributesWindows::default()
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round),
        ))
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowAttributesMacOS;
        attrs.with_platform_attributes(Box::new(
            WindowAttributesMacOS::default()
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true),
        ))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        attrs
    }
}

pub(super) fn popup_window_style(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::{CornerPreference, WindowAttributesWindows};
        attrs.with_platform_attributes(Box::new(
            WindowAttributesWindows::default()
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round),
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        attrs
    }
}

/// A CLI/forwarded argument -> URL: existing local files become `file://`, the rest is normalized.
pub(super) fn arg_to_url(arg: &str) -> String {
    match std::path::absolute(arg).ok().filter(|p| p.is_file()) {
        Some(abs) => url::Url::from_file_path(abs).map_or_else(|_| arg.to_string(), String::from),
        None => normalize_url_input(arg),
    }
}

pub(super) fn normalize_url_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "rust://home".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    for scheme in ["http://", "https://", "file://"] {
        if lower.starts_with(scheme) {
            return format!("{scheme}{}", &trimmed[scheme.len()..]);
        }
    }
    if lower.starts_with("rust://") {
        // Internal routes are matched case-sensitively; normalize the whole URL.
        return lower;
    }
    // Dangerous or external-handler schemes never navigate directly:
    // route them to web search instead of fetching them as documents.
    // (about:/mailto:/javascript:/data: passthrough was an XSS/navigation bug.)
    for scheme in ["about:", "mailto:", "javascript:", "data:", "tel:", "sms:"] {
        if lower.starts_with(scheme) {
            let encoded: String = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect();
            return format!("https://www.google.com/search?q={encoded}");
        }
    }
    if !trimmed.contains(char::is_whitespace) {
        let host_part = trimmed.split(['/', '?', '#']).next().unwrap_or(trimmed);
        let host_no_port = host_part.split(':').next().unwrap_or(host_part);
        if host_no_port.eq_ignore_ascii_case("localhost") || host_no_port == "127.0.0.1" {
            return format!("http://{trimmed}");
        }
        if trimmed.contains('.') {
            return format!("https://{trimmed}");
        }
    }
    let encoded: String = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect();
    format!("https://www.google.com/search?q={encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::DocumentConfig;
    use blitz_traits::shell::{ColorScheme, Viewport};

    #[test]
    fn pdf_urls_use_js_engine_and_popups_are_detected() {
        assert!(requires_js_engine(
            "https://example.com/files/Report.PDF?download=1#page=3"
        ));
        assert!(requires_js_engine("file:///C:/docs/a.pdf"));
        assert!(!requires_js_engine("https://example.com/pdf-guide.html"));
        assert!(wants_native_popup(
            "https://accounts.google.com/o/oauth2/auth?x=1",
            false
        ));
        assert!(wants_native_popup("about:blank", false));
        assert!(wants_native_popup("https://site.example/x", true));
        assert!(!wants_native_popup("https://site.example/x", false));
    }

    #[test]
    fn sign_in_hosts_use_js_engine() {
        assert!(requires_js_engine("https://m365.cloud.microsoft/chat"));
        assert!(requires_js_engine("https://login.microsoftonline.com/common/oauth2"));
        assert!(requires_js_engine("https://contoso.sharepoint.com/sites/x"));
        assert!(requires_js_engine("https://www.google.com/search?q=x"));
        assert!(requires_js_engine("https://github.com/meta/astryx"));
        assert!(!requires_js_engine("https://mail.google.com/"));
        assert!(!requires_js_engine("https://news.ycombinator.com/"));
        assert!(!requires_js_engine("https://notoffice.com/"));
        assert!(!requires_js_engine("rust://home"));
    }

    #[test]
    fn normalizes_urls_and_search_queries() {
        assert_eq!(normalize_url_input(""), "rust://home");
        assert_eq!(normalize_url_input("rust://bookmarks"), "rust://bookmarks");
        assert_eq!(normalize_url_input("example.com"), "https://example.com");
        assert_eq!(normalize_url_input("https://rust-lang.org"), "https://rust-lang.org");
        assert_eq!(
            normalize_url_input("HTTPS://Example.COM/Path"),
            "https://Example.COM/Path"
        );
        assert_eq!(normalize_url_input("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url_input("127.0.0.1:8080/api"), "http://127.0.0.1:8080/api");
        assert_eq!(
            normalize_url_input("mailto:a@b.com"),
            "https://www.google.com/search?q=mailto%3Aa%40b.com"
        );
        assert_eq!(normalize_url_input("RUST://BOOKMARKS"), "rust://bookmarks");
        assert_eq!(
            normalize_url_input("javascript:alert(1)"),
            "https://www.google.com/search?q=javascript%3Aalert%281%29"
        );
        assert_eq!(
            normalize_url_input("rust browser engine"),
            "https://www.google.com/search?q=rust+browser+engine"
        );
    }

    /// Blitz uses wheel-style deltas: a negative `scroll_viewport_by` dy moves the page down,
    /// which `keyboard_scroll` relies on.
    #[test]
    fn negative_viewport_delta_scrolls_down() {
        let mut doc = HtmlDocument::from_html(
            r#"<html><body><div style="height:5000px">tall</div></body></html>"#,
            DocumentConfig {
                viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        doc.scroll_viewport_by(0.0, -120.0);
        assert!(doc.viewport_scroll().y > 100.0, "{:?}", doc.viewport_scroll());
    }

    #[test]
    fn dangerous_download_extensions_are_blocked_while_documents_are_allowed() {
        assert!(is_dangerous_download_ext("installer.exe"));
        assert!(is_dangerous_download_ext("SETUP.MSI"));
        assert!(is_dangerous_download_ext("payload.bat."));
        assert!(is_dangerous_download_ext("script.ps1"));
        assert!(is_dangerous_download_ext("macro.vbs"));
        assert!(is_dangerous_download_ext("shortcut.lnk"));
        assert!(is_dangerous_download_ext("App.MSIX"));
        assert!(is_dangerous_download_ext("bundle.iso"));
        assert!(is_dangerous_download_ext("help.chm"));
        assert!(!is_dangerous_download_ext("report.pdf"));
        assert!(!is_dangerous_download_ext("export.xlsx"));
        assert!(!is_dangerous_download_ext("data.csv"));
        assert!(!is_dangerous_download_ext("archive.zip"));
        assert!(!is_dangerous_download_ext("photo.png"));
        assert!(!is_dangerous_download_ext("README"));
        assert_eq!(
            download_display_name(None, "https://example.com/files/quarterly%20report.pdf?dl=1"),
            "quarterly report.pdf"
        );
    }
}
