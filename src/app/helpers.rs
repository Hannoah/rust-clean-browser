//! Small free functions: window styling, URL input, key/pointer event construction.

use blitz_html::HtmlDocument;
use blitz_traits::events::{
    BlitzKeyEvent, BlitzPointerEvent, BlitzPointerId, BlitzWheelDelta, KeyState, MouseEventButton,
    MouseEventButtons, PointerCoords, PointerDetails,
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
        if let Ok(handle) = win.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
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
        if let Ok(handle) = win.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let mut p = Point { x: 0, y: 0 };
                if unsafe { ClientToScreen(h.hwnd.get(), &mut p) } != 0 {
                    return winit::dpi::PhysicalPosition::new(p.x, p.y);
                }
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

/// Hosts whose sign-in / app flows only work with a real JS engine and the Windows account SSO
/// (Microsoft Entra Conditional Access checks device compliance via WebView2's OS-account SSO).
pub(super) fn requires_js_engine(url: &str) -> bool {
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

pub(super) fn normalize_url_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "rust://home".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    for scheme in ["rust://", "http://", "https://", "file://"] {
        if lower.starts_with(scheme) {
            return format!("{scheme}{}", &trimmed[scheme.len()..]);
        }
    }
    if lower.starts_with("about:")
        || lower.starts_with("mailto:")
        || lower.starts_with("javascript:")
        || lower.starts_with("data:")
    {
        return trimmed.to_string();
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
        assert_eq!(normalize_url_input("HTTPS://Example.COM/Path"), "https://Example.COM/Path");
        assert_eq!(normalize_url_input("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url_input("127.0.0.1:8080/api"), "http://127.0.0.1:8080/api");
        assert_eq!(normalize_url_input("mailto:a@b.com"), "mailto:a@b.com");
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
}
