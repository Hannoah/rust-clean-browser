//! Crisp cross-platform inline `<svg>` icon library (1.65px geometric stroke, uniform visual weight).
//! Uses `stroke="currentColor"` (resolved by Blitz against each element's computed CSS `color`),
//! eliminating Windows-vs-macOS font glyph inconsistencies and emoji font bloat.

use std::fmt::Write;

#[derive(Clone, Copy)]
pub enum Icon {
    ArrowLeft,
    ArrowRight,
    Reload,
    Stop,
    Home,
    Plus,
    Close,
    Minimize,
    Maximize,
    Restore,
    Lock,
    AlertTriangle,
    Compass,
    Search,
    StarOutline,
    StarFilled,
    BookOpen,
    Zap,
    Code,
    ShieldCheck,
    ShieldOff,
    Bookmark,
    Folder,
    History,
    Cpu,
    Command,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    Trash,
    Sun,
    Moon,
    Monitor,
    ExternalLink,
    Check,
    PanelTop,
    Pencil,
}

impl Icon {
    fn body(self) -> &'static str {
        match self {
            Icon::ArrowLeft => r#"<path d="M10.5 3.5L5.5 8l5 4.5"/>"#,
            Icon::ArrowRight => r#"<path d="M5.5 3.5L10.5 8l-5 4.5"/>"#,
            Icon::Reload => {
                r#"<path d="M13 8a5 5 0 1 1-1.35-3.42"/><path d="M13 2.5v2.75h-2.75"/>"#
            }
            Icon::Stop => r#"<path d="M4 4l8 8M12 4l-8 8"/>"#,
            Icon::Home => {
                r#"<path d="M2.5 7.2L8 2.5l5.5 4.7V13a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1V7.2z"/><path d="M6.2 14V9.5h3.6V14"/>"#
            }
            Icon::Plus => r#"<path d="M8 3v10M3 8h10"/>"#,
            Icon::Close => r#"<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>"#,
            Icon::Minimize => r#"<path d="M3.5 8.5h9"/>"#,
            Icon::Maximize => r#"<rect x="3.5" y="3.5" width="9" height="9" rx="1.2"/>"#,
            Icon::Restore => {
                r#"<rect x="3.5" y="5.2" width="7.3" height="7.3" rx="1"/><path d="M5.5 5.2V3.8a.8.8 0 0 1 .8-.8h6a.8.8 0 0 1 .8.8v6a.8.8 0 0 1-.8.8h-1.4"/>"#
            }
            Icon::Lock => {
                r#"<rect x="3.5" y="7" width="9" height="6.5" rx="1.6"/><path d="M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7"/>"#
            }
            Icon::AlertTriangle => {
                r#"<path d="M7.15 2.8a1 1 0 0 1 1.7 0l5 8.5A1 1 0 0 1 13 12.8H3a1 1 0 0 1-.85-1.5l5-8.5z"/><path d="M8 6.2v3"/><circle cx="8" cy="11" r=".55" fill="currentColor" stroke="none"/>"#
            }
            Icon::Compass => {
                r#"<circle cx="8" cy="8" r="5.5"/><polygon points="10.4,5.6 9.1,9.1 5.6,10.4 6.9,6.9" fill="currentColor" stroke="none"/>"#
            }
            Icon::Search => r#"<circle cx="7" cy="7" r="4.3"/><path d="M10.2 10.2L13.5 13.5"/>"#,
            Icon::StarOutline => {
                r#"<path d="M8 2.2l1.75 3.55 3.92.57-2.83 2.76.67 3.9L8 11.14l-3.51 1.84.67-3.9-2.83-2.76 3.92-.57L8 2.2z"/>"#
            }
            Icon::StarFilled => {
                r#"<path d="M8 2.2l1.75 3.55 3.92.57-2.83 2.76.67 3.9L8 11.14l-3.51 1.84.67-3.9-2.83-2.76 3.92-.57L8 2.2z" fill="currentColor"/>"#
            }
            Icon::BookOpen => {
                r#"<path d="M8 4.2C6.8 3.2 4.8 2.8 2.5 3.2v9.2c2.3-.4 4.3 0 5.5 1 1.2-1 3.2-1.4 5.5-1V3.2c-2.3-.4-4.3 0-5.5 1z"/><path d="M8 4.2v9.2"/>"#
            }
            Icon::Pencil => r#"<path d="M11.2 2.6l2.2 2.2-7.6 7.6-3 .8.8-3 7.6-7.6z"/><path d="M9.8 4l2.2 2.2"/>"#,
            Icon::Zap => r#"<path d="M9 2L4 9h4l-1 5 5-7H8l1-5z"/>"#,
            Icon::Code => r#"<path d="M5.5 4.8L2.3 8l3.2 3.2M10.5 4.8L13.7 8l-3.2 3.2M9 3.2L7 12.8"/>"#,
            Icon::ShieldCheck => {
                r#"<path d="M8 2.2l4.8 1.9v3.6c0 3.1-2 5.4-4.8 6.3-2.8-.9-4.8-3.2-4.8-6.3V4.1L8 2.2z"/><path d="M5.8 8l1.6 1.6 3-3.1"/>"#
            }
            Icon::ShieldOff => {
                r#"<path d="M8 2.2l4.8 1.9v3.6c0 1.4-.4 2.7-1.1 3.7M3.2 5.2v2.5c0 3.1 2 5.4 4.8 6.3 1-.3 1.8-.8 2.5-1.5M2.5 2.5l11 11"/>"#
            }
            Icon::Bookmark => {
                r#"<path d="M4 2.8a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v10.8l-4-2.5-4 2.5V2.8z"/>"#
            }
            Icon::Folder => {
                r#"<path d="M2.2 4.2a1 1 0 0 1 1-1h3.3l1.3 1.5h5a1 1 0 0 1 1 1v6.2a1 1 0 0 1-1 1H3.2a1 1 0 0 1-1-1V4.2z"/>"#
            }
            Icon::History => {
                r#"<path d="M2.7 8a5.3 5.3 0 1 0 1.3-3.5L2.5 6"/><path d="M2.5 3v3h3"/><path d="M8 5.2v3l2 1.2"/>"#
            }
            Icon::Cpu => {
                r#"<rect x="4.5" y="4.5" width="7" height="7" rx="1.1"/><path d="M6.5 2v2.5M9.5 2v2.5M6.5 11.5V14M9.5 11.5V14M2 6.5h2.5M2 9.5h2.5M11.5 6.5H14M11.5 9.5H14"/>"#
            }
            Icon::Command => {
                r#"<path d="M5.5 5.5h5v5h-5z"/><path d="M5.5 5.5V4a1.5 1.5 0 1 0-1.5 1.5h1.5zm5 0V4A1.5 1.5 0 1 1 12 5.5h-1.5zm-5 5V12A1.5 1.5 0 1 1 4 10.5h1.5zm5 0V12A1.5 1.5 0 1 0 12 10.5h-1.5z"/>"#
            }
            Icon::ChevronRight => r#"<path d="M6 3.5l4.5 4.5L6 12.5"/>"#,
            Icon::ChevronUp => r#"<path d="M3.5 10L8 5.5l4.5 4.5"/>"#,
            Icon::ChevronDown => r#"<path d="M3.5 6L8 10.5 12.5 6"/>"#,
            Icon::Trash => {
                r#"<path d="M3.2 4.5h9.6M6 4.5V3.1a.8.8 0 0 1 .8-.8h2.4a.8.8 0 0 1 .8.8v1.4M4.8 4.5l.6 8.3a1 1 0 0 0 1 .9h3.2a1 1 0 0 0 1-.9l.6-8.3"/>"#
            }
            Icon::Sun => {
                r#"<circle cx="8" cy="8" r="2.6"/><path d="M8 2v1.4M8 12.6V14M2 8h1.4M12.6 8H14M3.8 3.8l1 1M11.2 11.2l1 1M12.2 3.8l-1 1M4.8 11.2l-1 1"/>"#
            }
            Icon::Moon => r#"<path d="M12.8 9.9A5.2 5.2 0 0 1 6.1 3.2a5.2 5.2 0 1 0 6.7 6.7z"/>"#,
            Icon::Monitor => {
                r#"<rect x="2.2" y="3" width="11.6" height="8" rx="1.2"/><path d="M5.5 13.5h5M8 11v2.5"/>"#
            }
            Icon::ExternalLink => {
                r#"<path d="M9.5 2.5h4v4M13.5 2.5L7.5 8.5M12 8.8v3.7a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3.7"/>"#
            }
            Icon::Check => r#"<path d="M3.5 8.2l3 3 6-6.2"/>"#,
            Icon::PanelTop => {
                r#"<rect x="2.2" y="2.8" width="11.6" height="10.4" rx="1.4"/><path d="M2.2 6.2h11.6"/>"#
            }
        }
    }
}

pub fn svg(icon: Icon, size: u32) -> String {
    svg_colored(icon, size, "currentColor")
}

pub fn svg_colored(icon: Icon, size: u32, stroke_color: &str) -> String {
    let mut s = String::with_capacity(220);
    let _ = write!(
        s,
        r#"<svg class="svg-ico" width="{size}" height="{size}" viewBox="0 0 16 16" fill="none" stroke="{stroke_color}" stroke-width="1.55" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
        icon.body()
    );
    s
}

/// Brand emblem used on `rust://home` (vector shield + stylized compass needle).
pub fn brand_emblem_svg(dark_mode: bool) -> &'static str {
    if dark_mode {
        r##"<svg width="44" height="44" viewBox="0 0 44 44" fill="none"><rect x="2" y="2" width="40" height="40" rx="13" fill="#1b3863" stroke="#7ab3ff" stroke-opacity="0.35" stroke-width="1.5"/><path d="M22 9l10 4v7.5c0 6.5-4.3 11.4-10 13.5-5.7-2.1-10-7-10-13.5V13l10-4z" fill="#7ab3ff" fill-opacity="0.16" stroke="#7ab3ff" stroke-width="1.8" stroke-linejoin="round"/><path d="M25.2 16.8l-2 5.4-5.4 2 2-5.4 5.4-2z" fill="#7ab3ff"/></svg>"##
    } else {
        r##"<svg width="44" height="44" viewBox="0 0 44 44" fill="none"><rect x="2" y="2" width="40" height="40" rx="13" fill="#dbeafe" stroke="#1b6ef3" stroke-opacity="0.24" stroke-width="1.5"/><path d="M22 9l10 4v7.5c0 6.5-4.3 11.4-10 13.5-5.7-2.1-10-7-10-13.5V13l10-4z" fill="#1b6ef3" fill-opacity="0.14" stroke="#1b6ef3" stroke-width="1.8" stroke-linejoin="round"/><path d="M25.2 16.8l-2 5.4-5.4 2 2-5.4 5.4-2z" fill="#1b6ef3"/></svg>"##
    }
}

/// Error page emblem illustration.
pub fn error_emblem_svg(dark_mode: bool) -> &'static str {
    if dark_mode {
        r##"<svg width="52" height="52" viewBox="0 0 52 52" fill="none"><rect x="2" y="2" width="48" height="48" rx="15" fill="#3f1618" stroke="#f87171" stroke-opacity="0.35" stroke-width="1.5"/><circle cx="26" cy="26" r="11" stroke="#f87171" stroke-width="2"/><path d="M26 20v7" stroke="#f87171" stroke-width="2.2" stroke-linecap="round"/><circle cx="26" cy="31.5" r="1.3" fill="#f87171"/></svg>"##
    } else {
        r##"<svg width="52" height="52" viewBox="0 0 52 52" fill="none"><rect x="2" y="2" width="48" height="48" rx="15" fill="#fee2e2" stroke="#dc2626" stroke-opacity="0.22" stroke-width="1.5"/><circle cx="26" cy="26" r="11" stroke="#dc2626" stroke-width="2"/><path d="M26 20v7" stroke="#dc2626" stroke-width="2.2" stroke-linecap="round"/><circle cx="26" cy="31.5" r="1.3" fill="#dc2626"/></svg>"##
    }
}
