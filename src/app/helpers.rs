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
/// icons don't flicker on every chrome rebuild. Bounded to 40 ms.
pub(super) fn wait_for_images(doc: &mut HtmlDocument) {
    let deadline = Instant::now() + std::time::Duration::from_millis(40);
    loop {
        doc.handle_messages();
        let pending = doc.tree().iter().any(|(_, node)| {
            node.element_data()
                .is_some_and(|el| &*el.name.local == "img" && el.image_data().is_none())
        });
        if !pending || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The JS-mode WebView is destroyed on tab switch to free memory, which would also drop its
/// session cookies (i.e. log the user out). Promote them to 30-day persistent cookies first so
/// logins survive tab switches and restarts.
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
                .with_corner_preference(CornerPreference::RoundSmall),
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
    if trimmed.starts_with("rust://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
    {
        return trimmed.to_string();
    }
    if !trimmed.contains(char::is_whitespace)
        && (trimmed.contains('.') || trimmed.starts_with("localhost"))
    {
        return format!("https://{trimmed}");
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
