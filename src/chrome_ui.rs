use crate::bookmarks::{BookmarkItem, BookmarkNode, BookmarkRootKind, BookmarkStore, count_urls, parse_folder_spec};
use crate::cleaner::html_escape;
use crate::history::{History, HistoryItem, SuggestKind, Suggestion};
use crate::icons::{Icon, brand_emblem_svg, error_emblem_svg, svg};
use crate::prefs::ThemePreference;
use crate::tabs::Tab;
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// Tab strip doubles as the (frameless) window title bar.
pub const TITLEBAR_HEIGHT: u32 = 40;
pub const TOOLBAR_HEIGHT: u32 = 46;
pub const BMBAR_HEIGHT: u32 = 32;
pub const POPUP_WIDTH: u32 = 336;
pub const POPUP_MAX_HEIGHT: u32 = 460;
pub const PALETTE_WIDTH: u32 = 560;
pub const PALETTE_MAX_HEIGHT: u32 = 480;
pub const FIND_BAR_WIDTH: u32 = 388;
pub const FIND_BAR_HEIGHT: u32 = 48;

pub const fn chrome_height(show_bookmarks_bar: bool) -> u32 {
    TITLEBAR_HEIGHT + TOOLBAR_HEIGHT + if show_bookmarks_bar { BMBAR_HEIGHT } else { 0 }
}

/// Design tokens shared by every built-in page and popup, following Fluent 2 + Material 3
/// tonal surface elevation and dark-mode contrast ergonomics.
const THEME_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --frame: #e3e8f0;
  --frame-hover: #edf0f6;
  --surface: #ffffff;
  --surface-2: #f6f8fb;
  --surface-3: #edf1f7;
  --omni: #eef2f7;
  --omni-hover: #e5ebf3;
  --border: rgba(15, 23, 42, 0.11);
  --divider: rgba(15, 23, 42, 0.08);
  --tab-specular: rgba(255, 255, 255, 0.9);
  --text: #16181d;
  --text-2: #525a69;
  --text-3: #838d9e;
  --accent: #1b6ef3;
  --accent-bg: #dbeafe;
  --accent-text: #0f3a8a;
  --accent-ring: rgba(27, 110, 243, 0.22);
  --green: #0f8a5f;
  --green-bg: #dcfce7;
  --green-text: #065f46;
  --amber: #b45309;
  --amber-bg: #fef3c7;
  --amber-text: #78350f;
  --hover: rgba(15, 23, 42, 0.055);
  --press: rgba(15, 23, 42, 0.11);
  --danger: #dc2626;
  --danger-bg: #fee2e2;
  --shadow-sm: 0 1px 2px rgba(15, 23, 42, 0.06);
  --shadow-md: 0 6px 20px rgba(15, 23, 42, 0.09), 0 1px 3px rgba(15, 23, 42, 0.05);
}
@media (prefers-color-scheme: dark) {
  :root {
    --frame: #121418;
    --frame-hover: #1c1f26;
    --surface: #1e2127;
    --surface-2: #17191e;
    --surface-3: #262a33;
    --omni: #13151a;
    --omni-hover: #181b21;
    --border: rgba(255, 255, 255, 0.10);
    --divider: rgba(255, 255, 255, 0.07);
    --tab-specular: rgba(255, 255, 255, 0.09);
    --text: #e9edf5;
    --text-2: #9da6b5;
    --text-3: #6c7585;
    --accent: #7ab3ff;
    --accent-bg: #1b3863;
    --accent-text: #d6e6ff;
    --accent-ring: rgba(122, 179, 255, 0.25);
    --green: #34d399;
    --green-bg: #0d3326;
    --green-text: #a7f3d0;
    --amber: #fbbf24;
    --amber-bg: #3b2a0d;
    --amber-text: #fde68a;
    --hover: rgba(255, 255, 255, 0.075);
    --press: rgba(255, 255, 255, 0.14);
    --danger: #f87171;
    --danger-bg: #3f1618;
    --shadow-sm: 0 1px 3px rgba(0, 0, 0, 0.35);
    --shadow-md: 0 10px 28px rgba(0, 0, 0, 0.45), 0 2px 6px rgba(0, 0, 0, 0.30);
  }
}
* { box-sizing: border-box; margin: 0; padding: 0; user-select: none; }
body {
  font-family: "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", -apple-system, "SF Pro Text", "Apple SD Gothic Neo", "Noto Sans KR", "Malgun Gothic", sans-serif;
  color: var(--text);
  cursor: default;
  line-height: 1.45;
}
a, a * { cursor: pointer; }
input { font-family: inherit; user-select: text; cursor: text; }
.svg-ico { display: block; flex-shrink: 0; }
.num { font-variant-numeric: tabular-nums; }
.fav { display: inline-flex; align-items: center; justify-content: center; width: 16px; height: 16px; border-radius: 4px; color: #fff; font-size: 10px; font-weight: 700; flex-shrink: 0; }
img.fav { border-radius: 3px; }
.fav.glyph { color: var(--text-2); background: transparent; }
kbd {
  display: inline-flex; align-items: center; justify-content: center;
  padding: 1px 6px; border-radius: 5px;
  font-family: inherit; font-size: 10.5px; font-weight: 600;
  color: var(--text-2); background: var(--surface-3);
  border: 1px solid var(--border);
  box-shadow: inset 0 -1px 0 var(--border);
}
a:focus-visible, .ib:focus-visible, .tile:focus-visible, .btn:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
@media (prefers-reduced-motion: reduce) {
  * { animation-duration: 0.01ms !important; transition-duration: 0.01ms !important; }
}
"#;

const CHROME_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--frame); user-select: none; }
.chrome { display: flex; flex-direction: column; width: 100%; height: 100%; }

/* ── Title bar & Tab strip ── */
.titlebar {
  display: flex; align-items: flex-end; height: 40px;
  padding-left: 8px; background: var(--frame);
}
.tabs {
  display: flex; align-items: flex-end; flex: 0 1 auto;
  min-width: 0; overflow: hidden; height: 40px; padding-top: 6px;
}
.tab {
  position: relative; display: flex; align-items: center; gap: 7px;
  width: 214px; flex: 0 1 auto; min-width: 68px; height: 34px;
  padding: 0 8px 0 11px; border-radius: 10px 10px 0 0;
  color: var(--text-2); font-size: 12px;
  transition: background 110ms ease, color 110ms ease;
}
.tab:hover { background: var(--frame-hover); color: var(--text); }
.tab.active {
  background: var(--surface); color: var(--text); font-weight: 600;
  box-shadow: inset 0 1px 0 var(--tab-specular), 0 -1px 4px rgba(0, 0, 0, 0.04);
}
.tab.sleeping { color: var(--text-3); }
.tab.sleeping .fav { opacity: 0.65; }
.tab.sleeping:hover { color: var(--text-2); }
.tab.sleeping:hover .fav { opacity: 0.92; }
.tab-sep {
  width: 1px; height: 15px; margin: 0 1px 9px;
  background: var(--border); flex-shrink: 0;
}
.tab:hover + .tab-sep { opacity: 0; }
@keyframes tab-spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
.fav-spin {
  display: inline-flex; align-items: center; justify-content: center;
  width: 16px; height: 16px; flex-shrink: 0;
  animation: tab-spin 0.8s linear infinite;
}
.tab-title { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; }
.tab-dot {
  width: 6px; height: 6px; border-radius: 3px; flex-shrink: 0;
}
.tab-dot.sleep { background: var(--text-3); opacity: 0.55; }
.tab-badge {
  font-size: 9.5px; font-weight: 700; padding: 1px 4px;
  border-radius: 4px; flex-shrink: 0; letter-spacing: 0.02em;
}
.tab-badge.js { background: var(--accent-bg); color: var(--accent-text); }
.tab-badge.reader { background: var(--green-bg); color: var(--green); }
.tab-close {
  display: flex; align-items: center; justify-content: center;
  width: 18px; height: 18px; border-radius: 9px;
  color: var(--text-2); flex-shrink: 0; opacity: 0.35;
}
.tab.active .tab-close, .tab:hover .tab-close { opacity: 1; }
.tab-close:hover { background: var(--press); color: var(--text); }
.newtab {
  display: flex; align-items: center; justify-content: center;
  width: 28px; height: 28px; margin: 0 0 3px 5px;
  border-radius: 14px; color: var(--text-2); flex-shrink: 0;
}
.newtab:hover { background: var(--hover); color: var(--text); }
.drag { flex: 1 1 0; min-width: 28px; height: 40px; }
.title-tools {
  display: flex; align-items: center; gap: 4px;
  height: 40px; padding-right: 6px; flex-shrink: 0;
}
.tchip {
  display: inline-flex; align-items: center; gap: 5px;
  height: 26px; padding: 0 9px; border-radius: 13px;
  font-size: 11px; font-weight: 500; color: var(--text-2);
  background: transparent;
}
.tchip:hover { background: var(--hover); color: var(--text); }
.tchip.icon-only { width: 28px; padding: 0; justify-content: center; }
.winctl { display: flex; align-self: flex-start; height: 40px; flex-shrink: 0; }
.wbtn {
  display: flex; align-items: center; justify-content: center;
  width: 46px; height: 40px; color: var(--text-2);
}
.wbtn:hover { background: var(--hover); color: var(--text); }
.wbtn.close:hover { background: var(--danger); color: #ffffff; }

/* ── Main Toolbar ── */
.toolbar {
  position: relative; display: flex; align-items: center; gap: 4px;
  height: 46px; padding: 0 10px; background: var(--surface);
  border-bottom: 1px solid var(--divider);
}
.toolbar.no-bm { border-bottom-color: var(--border); }
.nav-group { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.ib {
  position: relative; display: flex; align-items: center; justify-content: center;
  width: 32px; height: 32px; border-radius: 16px;
  color: var(--text-2); flex-shrink: 0;
}
.ib:hover { background: var(--hover); color: var(--text); }
.ib.off { color: var(--text-3); opacity: 0.42; }
.ib.off:hover { background: transparent; color: var(--text-3); }
.ib.on { color: var(--accent); background: var(--accent-bg); }
.ib.star-on { color: #f59e0b; }

/* ── Elevated Omnibox Pill ── */
.omni {
  display: flex; align-items: center; gap: 6px;
  flex: 1; min-width: 160px; height: 34px; margin: 0 6px;
  padding: 0 5px 0 8px; border-radius: 17px;
  border: 1.5px solid transparent; background: var(--omni);
  transition: background 120ms ease, border-color 120ms ease;
}
.omni:hover { background: var(--omni-hover); }
.omni:focus-within {
  background: var(--surface); border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-ring);
}
.omni-lead {
  display: flex; align-items: center; justify-content: center;
  width: 22px; height: 22px; border-radius: 11px; flex-shrink: 0;
  color: var(--text-2);
}
.omni-lead.secure { color: var(--green); }
.omni-lead.warn { color: var(--amber); }
.omni-lead.internal { color: var(--accent); }
#url-bar {
  flex: 1; min-width: 40px; height: 100%; border: none;
  background: transparent; color: var(--text); font-size: 13.5px; outline: none;
}
.omni-end { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.omni-btn {
  display: flex; align-items: center; justify-content: center;
  width: 26px; height: 26px; border-radius: 13px;
  color: var(--text-2); flex-shrink: 0;
}
.omni-btn:hover { background: var(--hover); color: var(--text); }
.omni-btn.on { color: var(--accent); background: var(--accent-bg); }
.omni-btn.star-on { color: #f59e0b; }
.zoom-pill {
  display: inline-flex; align-items: center; height: 22px; padding: 0 8px;
  border-radius: 11px; background: var(--accent-bg); color: var(--accent-text);
  font-size: 11px; font-weight: 600; white-space: nowrap;
}

/* ── Right Utility Pills ── */
.util-group { display: flex; align-items: center; gap: 5px; flex-shrink: 0; }
.pill {
  display: inline-flex; align-items: center; gap: 5px;
  height: 28px; padding: 0 10px; border-radius: 14px;
  background: var(--omni); border: 1px solid var(--divider);
  font-size: 11.5px; font-weight: 600; color: var(--text-2);
  white-space: nowrap; flex-shrink: 0;
}
.pill:hover { background: var(--omni-hover); color: var(--text); border-color: var(--border); }
.pill.clean { background: var(--green-bg); color: var(--green-text); border-color: transparent; }
.pill.js { background: var(--accent-bg); color: var(--accent-text); border-color: transparent; }
.pill.shield-on { color: var(--green); }
.pill.toast {
  background: var(--accent-bg); color: var(--accent-text);
  border-color: transparent; font-weight: 500;
}
.vsep { width: 1px; height: 18px; margin: 0 2px; background: var(--divider); flex-shrink: 0; }

/* ── Animated 2px Loading Progress Bar ── */
.progress-track {
  position: absolute; left: 0; right: 0; bottom: 0; height: 2px;
  overflow: hidden; background: transparent;
}
@keyframes progress-slide {
  0% { transform: translateX(-45%); }
  100% { transform: translateX(130%); }
}
.progress-bar {
  width: 45%; height: 100%; border-radius: 2px;
  background: linear-gradient(90deg, transparent, var(--accent), transparent);
  animation: progress-slide 1.15s ease-in-out infinite;
}

/* ── Bookmark Bar ── */
.bmbar {
  display: flex; align-items: center; gap: 4px; height: 32px;
  padding: 0 10px; background: var(--surface);
  border-bottom: 1px solid var(--border); overflow: hidden;
}
.bm-items { display: flex; align-items: center; gap: 2px; flex: 1; min-width: 0; overflow: hidden; }
.chip {
  display: inline-flex; align-items: center; gap: 6px;
  height: 24px; padding: 0 9px; border-radius: 12px;
  color: var(--text); font-size: 12px; white-space: nowrap; flex-shrink: 0;
}
.chip:hover { background: var(--hover); }
.chip.drop { background: var(--accent-bg); color: var(--accent-text); font-weight: 600; }
.chip .fav { width: 14px; height: 14px; font-size: 9px; }
.chip .fico { color: var(--text-2); display: inline-flex; }
.chip.drop .fico { color: var(--accent-text); }
.chip.muted { color: var(--text-2); font-size: 11.5px; }
.bm-right { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.bm-empty { font-size: 11.5px; color: var(--text-3); padding-left: 6px; }
@media (max-width: 780px) {
  .ib { width: 28px; height: 28px; }
  .omni { min-width: 96px; margin: 0 3px; }
  .pill { padding: 0 8px; font-size: 11px; }
  .hide-narrow { display: none; }
}
"#;

const CUSTOM_WINDOW_CONTROLS: bool = cfg!(target_os = "windows") || cfg!(target_os = "linux");
const TRAFFIC_LIGHT_INSET: u32 = if cfg!(target_os = "macos") { 78 } else { 8 };

#[cfg(test)]
fn substitute_icon_entities(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(pos) = rest.find("&#xE") {
        out.push_str(&rest[..pos]);
        let entity = &rest[pos..];
        let code = entity.get(3..7).unwrap_or("");
        if entity.as_bytes().get(7) == Some(&b';') {
            let sub = match code {
                "E72B" => "←",
                "E72A" => "→",
                "E72C" | "E895" => "↻",
                "E711" | "E8BB" => "✕",
                "E80F" => "⌂",
                "E710" => "+",
                "E921" => "–",
                "E922" => "□",
                "E923" => "❐",
                "E734" => "☆",
                "E735" => "★",
                "E72E" | "E721" => "",
                "E946" => "ⓘ",
                "E8B7" => "▸",
                "E736" => "¶",
                "E943" => "{}",
                "EA18" => "◈",
                "E728" => "☰",
                "E76C" => "›",
                "E783" => "⚠",
                _ => "•",
            };
            out.push_str(sub);
            rest = &entity[8..];
        } else {
            out.push('&');
            rest = &entity[1..];
        }
    }
    out.push_str(rest);
    out
}

pub struct ChromeRenderState<'a> {
    pub tabs: &'a [Tab],
    pub active_idx: usize,
    pub bookmarks: &'a BookmarkStore,
    pub can_back: bool,
    pub can_forward: bool,
    pub working_set_mb: f64,
    pub blocked_count: usize,
    pub filter_enabled: bool,
    pub maximized: bool,
    pub dark_mode: bool,
    pub show_bookmarks_bar: bool,
    pub theme_pref: ThemePreference,
    pub status_note: Option<&'a str>,
    /// Text the user is typing in the URL bar, kept across chrome rebuilds.
    pub url_override: Option<&'a str>,
    /// Folder spec currently hovered during a bookmark drag-and-drop.
    pub drop_target_folder: Option<&'a str>,
}

fn spinner_html(dark_mode: bool) -> String {
    let accent = if dark_mode { "#7ab3ff" } else { "#1b6ef3" };
    format!(
        r#"<span class="fav-spin"><svg width="14" height="14" viewBox="0 0 16 16" fill="none"><circle cx="8" cy="8" r="5.5" fill="none" stroke="{accent}" stroke-opacity="0.22" stroke-width="2" /><path d="M 8 2.5 A 5.5 5.5 0 0 1 11.89 11.89" fill="none" stroke="{accent}" stroke-width="2" stroke-linecap="round" /></svg></span>"#
    )
}

fn favicon_html(title: &str, url: &str) -> String {
    if url.starts_with("rust://") {
        let ic = if url.starts_with("rust://bookmarks") {
            Icon::Bookmark
        } else if url.starts_with("rust://history") {
            Icon::History
        } else {
            Icon::Compass
        };
        return format!(r#"<span class="fav glyph">{}</span>"#, svg(ic, 14));
    }
    if let Some(src) = crate::favicon::cached_icon_url(url) {
        return format!(r#"<img class="fav img" src="{}" />"#, html_escape(&src));
    }
    let (initial, color) = monogram_badge(title, url);
    format!(r#"<span class="fav" style="background:{color}">{initial}</span>"#)
}

pub fn render_chrome_html(state: &ChromeRenderState<'_>) -> String {
    let active = state.tabs.get(state.active_idx).or_else(|| state.tabs.first());
    let Some(active) = active else {
        return "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body></body></html>".to_string();
    };
    let sleeping_count = state
        .tabs
        .iter()
        .enumerate()
        .filter(|(i, t)| *i != state.active_idx && !t.is_loaded() && !t.js_mode)
        .count();

    let mut h = String::with_capacity(24 * 1024);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(CHROME_CSS);
    h.push_str("</style></head><body><div class=\"chrome\">");

    // ── 1. Title bar: tabs + new tab + drag spacer + theme/palette + window controls ──
    let _ = write!(
        h,
        r#"<div class="titlebar" data-drag="1" style="padding-left:{TRAFFIC_LIGHT_INSET}px"><div class="tabs" data-drag="1">"#
    );
    let close_svg = svg(Icon::Close, 11);
    for (idx, tab) in state.tabs.iter().enumerate() {
        let is_active = idx == state.active_idx;
        let next_active = idx + 1 == state.active_idx;
        let is_sleeping = !is_active && !tab.is_loaded() && !tab.js_mode;
        let cls = match (is_active, is_sleeping) {
            (true, _) => "tab active",
            (false, true) => "tab sleeping",
            (false, false) => "tab",
        };
        let fav = if tab.is_loading {
            spinner_html(state.dark_mode)
        } else {
            favicon_html(&tab.title, &tab.url)
        };
        let mode_badge = if tab.js_mode {
            r#"<span class="tab-badge js">JS</span>"#
        } else if tab.reader_mode {
            r#"<span class="tab-badge reader">R</span>"#
        } else if is_sleeping {
            r#"<span class="tab-dot sleep"></span>"#
        } else {
            ""
        };
        let bm_attr = if tab.url.starts_with("rust://") {
            String::new()
        } else {
            format!(
                r#" data-bm-src="url:{}" data-bm-title="{}""#,
                html_escape(&tab.url),
                html_escape(&truncate_chars(&tab.title, 40)),
            )
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="tab:{idx}"{bm_attr}>{fav}<span class="tab-title">{title}</span>{mode_badge}<span class="tab-close" data-action="close-tab:{idx}">{close_svg}</span></div>"#,
            title = html_escape(&truncate_chars(&tab.title, 26)),
        );
        if !is_active && !next_active && idx + 1 < state.tabs.len() {
            h.push_str(r#"<div class="tab-sep"></div>"#);
        }
    }
    let plus_svg = svg(Icon::Plus, 14);
    let theme_icon = match state.theme_pref {
        ThemePreference::System => Icon::Monitor,
        ThemePreference::Light => Icon::Sun,
        ThemePreference::Dark => Icon::Moon,
    };
    let palette_label = if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl+K" };
    let _ = write!(
        h,
        r#"</div><div class="newtab" data-action="new-tab">{plus_svg}</div><div class="drag" data-drag="1"></div>
<div class="title-tools">
  <div class="tchip icon-only" data-action="cycle-theme">{theme_svg}</div>
  <div class="tchip" data-action="open-palette">{cmd_svg}<span>{palette_label}</span></div>
</div>"#,
        theme_svg = svg(theme_icon, 13),
        cmd_svg = svg(Icon::Command, 12),
    );
    if CUSTOM_WINDOW_CONTROLS {
        let max_svg = svg(if state.maximized { Icon::Restore } else { Icon::Maximize }, 11);
        let _ = write!(
            h,
            r#"<div class="winctl"><div class="wbtn" data-action="win-min">{min}</div><div class="wbtn" data-action="win-max">{max_svg}</div><div class="wbtn close" data-action="win-close">{close}</div></div>"#,
            min = svg(Icon::Minimize, 11),
            close = svg(Icon::Close, 11),
        );
    }
    h.push_str("</div>");

    // ── 2. Main Toolbar ──
    let can_back = state.can_back;
    let can_forward = state.can_forward;
    let is_bookmarked = state.bookmarks.contains_url(&active.url);
    let idn_suspicious = is_suspicious_host(&active.url);
    let (lead_cls, lead_svg) = if idn_suspicious {
        ("omni-lead warn", svg(Icon::AlertTriangle, 13))
    } else if active.url.starts_with("https://") {
        ("omni-lead secure", svg(Icon::Lock, 13))
    } else if active.url.starts_with("rust://") {
        ("omni-lead internal", svg(Icon::Compass, 14))
    } else if active.url.starts_with("http://") {
        ("omni-lead warn", svg(Icon::AlertTriangle, 13))
    } else {
        ("omni-lead", svg(Icon::Search, 13))
    };
    let url_val = match state.url_override {
        Some(t) => t,
        None if active.url == "rust://home" => "",
        None => active.url.as_str(),
    };
    let focus_key = if cfg!(target_os = "macos") { "⌘L" } else { "Ctrl+L" };
    let toolbar_cls = if state.show_bookmarks_bar {
        "toolbar"
    } else {
        "toolbar no-bm"
    };
    let _ = write!(
        h,
        r#"<div class="{toolbar_cls}">
<div class="nav-group">
  <div class="ib{back_off}" data-action="back">{back}</div>
  <div class="ib{fwd_off}" data-action="forward">{fwd}</div>
  <div class="ib" data-action="reload">{reload}</div>
  <div class="ib" data-action="nav:rust://home">{home}</div>
</div>
<div id="omni-box" class="omni">
  <span class="{lead_cls}"{lead_tip}>{lead_svg}</span>
  <input id="url-bar" type="text" value="{url_val}" placeholder="검색어 또는 URL 입력 ({focus_key} · {palette_label} 명령)" />
  <div class="omni-end">"#,
        back_off = if can_back { "" } else { " off" },
        fwd_off = if can_forward { "" } else { " off" },
        back = svg(Icon::ArrowLeft, 15),
        fwd = svg(Icon::ArrowRight, 15),
        reload = svg(if active.is_loading { Icon::Stop } else { Icon::Reload }, 14),
        home = svg(Icon::Home, 14),
        url_val = html_escape(url_val),
        lead_tip = if idn_suspicious {
            " title=\"도메인에 서로 다른 문자 체계가 섞여 있습니다. 피싱 사이트일 수 있으니 주의하세요.\""
        } else {
            ""
        },
    );

    if (active.zoom - 1.0).abs() > 0.001 {
        let _ = write!(
            h,
            r#"<div class="zoom-pill num" data-action="zoom-reset">{:.0}%</div>"#,
            active.zoom * 100.0
        );
    }
    if !active.url.starts_with("rust://") {
        let r_cls = if active.reader_mode { "omni-btn on" } else { "omni-btn" };
        let _ = write!(
            h,
            r#"<div class="{r_cls}" data-action="toggle-reader">{}</div>"#,
            svg(Icon::BookOpen, 14)
        );
    }
    let star_cls = if is_bookmarked { "omni-btn star-on" } else { "omni-btn" };
    let star_svg = if is_bookmarked {
        svg(Icon::StarFilled, 14)
    } else {
        svg(Icon::StarOutline, 14)
    };
    let _ = write!(
        h,
        r#"<div class="{star_cls}" data-action="toggle-bookmark">{star_svg}</div>
  </div>
</div>
<div class="util-group">"#
    );

    if let Some(note) = state.status_note {
        let _ = write!(
            h,
            r#"<div class="pill toast hide-narrow" data-action="dismiss-toast">{}<span>{}</span></div>"#,
            svg(Icon::Check, 12),
            html_escape(&truncate_chars(note, 26)),
        );
    }

    // PDF tabs: one-click switch between WebView2's viewer and the pdf.js annotator.
    let pdf_btn = if active.is_pdf() {
        let (cls, label) = if active.editing_pdf() {
            ("pill js", "필기 끄기")
        } else {
            ("pill", "필기")
        };
        format!(
            r#"<div class="{cls}" data-action="pdf-editor">{}<span>{label}</span></div>"#,
            svg(Icon::Pencil, 12)
        )
    } else {
        String::new()
    };
    let (engine_cls, engine_svg, engine_label) = if active.js_mode {
        ("pill js", svg(Icon::Code, 12), "JS")
    } else {
        ("pill clean", svg(Icon::Zap, 12), "Clean")
    };
    let (shield_cls, shield_svg, shield_label) = if state.filter_enabled {
        (
            "pill shield-on num",
            svg(Icon::ShieldCheck, 13),
            state.blocked_count.to_string(),
        )
    } else {
        ("pill", svg(Icon::ShieldOff, 13), "OFF".to_string())
    };
    let ram_label = if sleeping_count > 0 {
        format!(
            r#"{:.0} MB<span class="hide-narrow"> · {sleeping_count}절전</span>"#,
            state.working_set_mb
        )
    } else {
        format!("{:.0} MB", state.working_set_mb)
    };

    let _ = write!(
        h,
        r#"{pdf_btn}<div class="{engine_cls}" data-action="toggle-js">{engine_svg}<span>{engine_label}</span></div>
  <div class="{shield_cls}" data-action="toggle-filter">{shield_svg}<span>{shield_label}</span></div>
  <div class="pill num" data-action="trim-memory">{cpu_svg}<span>{ram_label}</span></div>
  <div class="vsep"></div>
  <div class="ib" data-action="open-find">{find_svg}</div>
  <div class="ib" data-action="nav:rust://history">{hist_svg}</div>
  <div class="ib" data-action="nav:rust://bookmarks">{bm_svg}</div>
</div>"#,
        cpu_svg = svg(Icon::Cpu, 13),
        find_svg = svg(Icon::Search, 14),
        hist_svg = svg(Icon::History, 14),
        bm_svg = svg(Icon::Bookmark, 14),
    );

    if active.is_loading {
        h.push_str(r#"<div class="progress-track"><div class="progress-bar"></div></div>"#);
    }
    h.push_str("</div>");

    // ── 3. Collapsible Bookmark Bar ──
    if state.show_bookmarks_bar {
        h.push_str(r#"<div class="bmbar"><div class="bm-items" data-bm-folder="bar">"#);
        const MAX_CHIPS: usize = 16;
        let mut rendered = 0usize;
        for (idx, item) in state.bookmarks.custom.iter().enumerate().take(MAX_CHIPS) {
            push_url_chip(&mut h, item, &format!("custom-{idx}"));
            rendered += 1;
        }
        let mut shown_bar = 0usize;
        let folder_svg = svg(Icon::Folder, 13);
        for (i, node) in state.bookmarks.bar.iter().enumerate() {
            if rendered >= MAX_CHIPS {
                break;
            }
            shown_bar = i + 1;
            rendered += 1;
            match node {
                BookmarkNode::Url(item) => push_url_chip(&mut h, item, &format!("bar-{i}")),
                BookmarkNode::Folder(folder) => {
                    let fkey = format!("bar-{i}");
                    let drop_cls = if state.drop_target_folder == Some(fkey.as_str()) {
                        "chip drop"
                    } else {
                        "chip"
                    };
                    let _ = write!(
                        h,
                        r#"<div class="{drop_cls}" data-action="bm-popup:bar:{i}" data-bm-folder="{fkey}"><span class="fico">{folder_svg}</span><span>{}</span></div>"#,
                        html_escape(&truncate_chars(&folder.title, 16)),
                    );
                }
            }
        }
        if rendered == 0 {
            h.push_str(
                r#"<span class="bm-empty">북마크를 추가하려면 주소창의 ☆를 누르세요 · Ctrl+Shift+B 숨기기</span>"#,
            );
        }
        h.push_str(r#"</div><div class="bm-right">"#);
        if state.bookmarks.bar.len() > shown_bar {
            let _ = write!(
                h,
                r#"<div class="chip num" data-action="bm-popup:overflow:{shown_bar}"><span class="fico">{}</span><span>{}</span></div>"#,
                svg(Icon::ChevronRight, 12),
                state.bookmarks.bar.len() - shown_bar
            );
        }
        if !state.bookmarks.other.is_empty() {
            let other_cls = if state.drop_target_folder == Some("other") {
                "chip drop"
            } else {
                "chip"
            };
            let _ = write!(
                h,
                r#"<div class="{other_cls}" data-action="bm-popup:other" data-bm-folder="other"><span class="fico">{folder_svg}</span><span>기타 북마크</span></div>"#
            );
        }
        let _ = write!(
            h,
            r#"<div class="chip muted" data-action="toggle-bm-bar">{}</div></div></div>"#,
            svg(Icon::PanelTop, 12),
        );
    }

    h.push_str("</div></body></html>");
    h
}

fn push_url_chip(h: &mut String, item: &BookmarkItem, src_spec: &str) {
    let short_title = html_escape(&truncate_chars(&item.title, 18));
    let _ = write!(
        h,
        r#"<div class="chip" data-action="nav:{url}" data-bm-src="{src}" data-bm-title="{short_title}">{fav}<span>{short_title}</span></div>"#,
        url = html_escape(&item.url),
        src = html_escape(src_spec),
        fav = favicon_html(&item.title, &item.url),
    );
}

/// Ideal logical popup height for bookmark folder popups (32px rows + header + footer).
pub fn estimate_popup_height(item_count: usize, can_go_back: bool) -> u32 {
    let rows_h = (item_count.max(1) as u32) * 32 + 16;
    let header_h = if can_go_back { 38 } else { 32 };
    let footer_h = 34;
    (rows_h + header_h + footer_h).clamp(112, POPUP_MAX_HEIGHT)
}

const POPUP_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.shell { display: flex; flex-direction: column; width: 100%; height: 100%; border: 1px solid var(--border); background: var(--surface); }
.hdr { display: flex; align-items: center; gap: 8px; height: 36px; padding: 0 10px; border-bottom: 1px solid var(--divider); background: var(--surface-2); flex-shrink: 0; }
.back { display: flex; align-items: center; justify-content: center; width: 24px; height: 24px; border-radius: 12px; color: var(--text-2); }
.back:hover { background: var(--hover); color: var(--text); }
.hdr-ico { color: var(--text-2); display: inline-flex; }
.hdr-title { flex: 1; font-size: 12px; font-weight: 600; color: var(--text); white-space: nowrap; overflow: hidden; }
.hdr-cnt { font-size: 11px; color: var(--text-3); padding: 1px 7px; border-radius: 999px; background: var(--surface-3); }
.list { flex: 1; min-height: 0; overflow-y: auto; padding: 6px; }
.row { display: flex; align-items: center; gap: 10px; height: 32px; padding: 0 10px; border-radius: 8px; font-size: 12.5px; color: var(--text); }
.row:hover { background: var(--hover); }
.row.sel { background: var(--accent-bg); color: var(--accent-text); font-weight: 500; }
.fico { color: var(--text-2); display: inline-flex; flex-shrink: 0; }
.title { flex: 1; white-space: nowrap; overflow: hidden; }
.arrow { color: var(--text-3); display: inline-flex; }
.empty { padding: 18px; text-align: center; color: var(--text-3); font-size: 12px; }
.ftr { display: flex; align-items: center; justify-content: space-between; height: 34px; padding: 0 12px; border-top: 1px solid var(--divider); background: var(--surface-2); font-size: 11.5px; color: var(--text-2); flex-shrink: 0; }
.ftr:hover { background: var(--hover); color: var(--text); }
"#;

pub fn render_bookmark_popup_html(
    title: &str,
    nodes: &[BookmarkNode],
    can_go_back: bool,
    index_offset: usize,
    selected: Option<usize>,
) -> String {
    let mut h = String::with_capacity(4096 + nodes.len() * 280);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(POPUP_CSS);
    h.push_str(r#"</style></head><body><div class="shell"><div class="hdr">"#);
    if can_go_back {
        let _ = write!(
            h,
            r#"<div class="back" data-action="popup-back">{}</div>"#,
            svg(Icon::ArrowLeft, 13),
        );
    } else {
        let _ = write!(h, r#"<span class="hdr-ico">{}</span>"#, svg(Icon::Folder, 14));
    }
    let _ = write!(
        h,
        r#"<div class="hdr-title">{}</div><span class="hdr-cnt num">{}</span></div><div class="list">"#,
        html_escape(title),
        nodes.len(),
    );

    let folder_svg = svg(Icon::Folder, 14);
    let chev_svg = svg(Icon::ChevronRight, 13);
    for (idx, node) in nodes.iter().enumerate() {
        let abs_idx = index_offset + idx;
        let sel = if selected == Some(idx) { " sel" } else { "" };
        match node {
            BookmarkNode::Folder(folder) => {
                let _ = write!(
                    h,
                    r#"<div class="row{sel}" data-action="popup-enter:{abs_idx}"><span class="fico">{folder_svg}</span><span class="title">{}</span><span class="arrow">{chev_svg}</span></div>"#,
                    html_escape(&truncate_chars(&folder.title, 30)),
                );
            }
            BookmarkNode::Url(item) => {
                let _ = write!(
                    h,
                    r#"<div class="row{sel}" data-action="popup-open:{url}">{fav}<span class="title">{name}</span></div>"#,
                    url = html_escape(&item.url),
                    fav = favicon_html(&item.title, &item.url),
                    name = html_escape(&truncate_chars(&item.title, 34)),
                );
            }
        }
    }
    if nodes.is_empty() {
        h.push_str(r#"<div class="empty">비어 있는 폴더입니다.</div>"#);
    }
    let _ = write!(
        h,
        r#"</div><div class="ftr" data-action="popup-manager"><span>북마크 관리자 열기</span>{}</div></div></body></html>"#,
        svg(Icon::ExternalLink, 12),
    );
    h
}

/// Address-bar suggestion dropdown height: 38px rows + padding + 30px footer hint.
pub fn suggest_popup_height(rows: usize) -> u32 {
    // Clamp like estimate_popup_height so a full suggestion list can't overflow the screen.
    let rows = rows.min(7);
    ((rows as u32) * 38 + 12 + 30).clamp(80, 480)
}

const SUGGEST_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.shell { display: flex; flex-direction: column; height: 100vh; border: 1px solid var(--border); background: var(--surface); }
.list { flex: 1; padding: 6px; overflow: hidden; }
.row {
  position: relative; display: flex; align-items: center; gap: 10px;
  height: 38px; padding: 0 12px; border-radius: 9px;
  font-size: 13px; white-space: nowrap; overflow: hidden;
}
.row:hover { background: var(--hover); }
.row.sel { background: var(--accent-bg); color: var(--accent-text); }
.sico { width: 16px; display: inline-flex; align-items: center; justify-content: center; color: var(--text-2); flex-shrink: 0; }
.row.sel .sico { color: var(--accent); }
.main { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; overflow: hidden; }
.title { flex: 0 1 auto; min-width: 0; overflow: hidden; font-weight: 500; }
.url { flex: 1 1 0; min-width: 0; overflow: hidden; color: var(--text-3); font-size: 12px; }
.row.sel .url { color: var(--accent-text); opacity: 0.78; }
.hl { font-weight: 700; color: var(--accent); }
.row.sel .hl { color: var(--accent-text); text-decoration: underline; }
.badge {
  font-size: 10.5px; font-weight: 600; padding: 2px 7px;
  border-radius: 999px; background: var(--surface-3); color: var(--text-2); flex-shrink: 0;
}
.badge.tab { background: var(--green-bg); color: var(--green-text); }
.badge.bm { background: var(--amber-bg); color: var(--amber-text); }
.row.sel .badge { background: var(--surface); color: var(--accent-text); }
.ftr {
  display: flex; align-items: center; justify-content: space-between;
  height: 30px; padding: 0 12px; border-top: 1px solid var(--divider);
  background: var(--surface-2); font-size: 11px; color: var(--text-3); flex-shrink: 0;
}
"#;

/// Suggestion rows for the address bar; `selected` is highlighted (Enter opens it).
pub fn render_suggest_popup_html(rows: &[Suggestion], selected: usize, query: &str) -> String {
    let mut h = String::with_capacity(3072 + rows.len() * 440);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(SUGGEST_CSS);
    h.push_str(r#"</style></head><body><div class="shell"><div class="list">"#);
    for (i, row) in rows.iter().enumerate() {
        let sel = if i == selected { " sel" } else { "" };
        let (icon, title_html, detail_html, badge_cls, badge_lbl) = match row.kind {
            SuggestKind::Typed if row.url.starts_with("https://www.google.com/search?") => (
                format!(r#"<span class="sico">{}</span>"#, svg(Icon::Search, 14)),
                html_escape(&truncate_chars(&row.title, 56)),
                "Google에서 검색".to_string(),
                "badge",
                "검색",
            ),
            SuggestKind::Typed => (
                format!(r#"<span class="sico">{}</span>"#, svg(Icon::ArrowRight, 14)),
                html_escape(&truncate_chars(&row.url, 64)),
                "직접 이동".to_string(),
                "badge",
                "이동",
            ),
            SuggestKind::OpenTab => {
                let title = if row.title.trim().is_empty() {
                    &row.url
                } else {
                    &row.title
                };
                (
                    favicon_html(&row.title, &row.url),
                    highlight_escaped(&truncate_chars(title, 46), query),
                    highlight_escaped(&truncate_chars(strip_url_scheme(&row.url), 64), query),
                    "badge tab",
                    "열린 탭",
                )
            }
            SuggestKind::Bookmark => {
                let title = if row.title.trim().is_empty() {
                    &row.url
                } else {
                    &row.title
                };
                (
                    favicon_html(&row.title, &row.url),
                    highlight_escaped(&truncate_chars(title, 46), query),
                    highlight_escaped(&truncate_chars(strip_url_scheme(&row.url), 64), query),
                    "badge bm",
                    "북마크",
                )
            }
            SuggestKind::History => {
                let title = if row.title.trim().is_empty() {
                    &row.url
                } else {
                    &row.title
                };
                (
                    favicon_html(&row.title, &row.url),
                    highlight_escaped(&truncate_chars(title, 46), query),
                    highlight_escaped(&truncate_chars(strip_url_scheme(&row.url), 64), query),
                    "badge",
                    "방문 기록",
                )
            }
        };
        let _ = write!(
            h,
            r#"<div class="row{sel}" data-action="popup-open:{url}">{icon}<div class="main"><span class="title">{title_html}</span><span class="url">— {detail_html}</span></div><span class="{badge_cls}">{badge_lbl}</span></div>"#,
            url = html_escape(&row.url),
        );
    }
    h.push_str(
        r#"</div><div class="ftr"><span><kbd>↑</kbd> <kbd>↓</kbd> 탐색 · <kbd>Enter</kbd> 열기 · <kbd>Alt+Enter</kbd> 새 탭</span><span><kbd>Esc</kbd> 닫기</span></div></div></body></html>"#,
    );
    h
}

// ── Command Palette (`Ctrl+K`) ──

#[derive(Debug, Clone)]
pub struct PaletteEntry {
    pub action: String,
    pub category: &'static str,
    pub title: String,
    pub subtitle: String,
    pub shortcut: &'static str,
    pub icon_kind: PaletteIcon,
}

#[derive(Debug, Clone, Copy)]
pub enum PaletteIcon {
    Tab,
    Command,
    Bookmark,
    History,
    Search,
    Theme,
    Shield,
    Zap,
    Book,
    Memory,
}

const PALETTE_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.shell { display: flex; flex-direction: column; width: 100%; height: 100%; border: 1px solid var(--border); background: var(--surface); }
.search-row {
  display: flex; align-items: center; gap: 10px; height: 50px;
  padding: 0 14px; border-bottom: 1px solid var(--divider); background: var(--surface); flex-shrink: 0;
}
.search-ico { color: var(--accent); display: inline-flex; flex-shrink: 0; }
#palette-input {
  flex: 1; height: 34px; border: none; background: transparent;
  color: var(--text); font-size: 14.5px; outline: none;
}
.list { flex: 1; min-height: 0; overflow-y: auto; padding: 6px; }
.cat { padding: 7px 10px 4px; font-size: 11px; font-weight: 600; color: var(--text-3); letter-spacing: 0.03em; }
.row {
  display: flex; align-items: center; gap: 10px; height: 38px;
  padding: 0 10px; border-radius: 9px; font-size: 13px; color: var(--text);
}
.row:hover { background: var(--hover); }
.row.sel { background: var(--accent-bg); color: var(--accent-text); }
.pico {
  width: 24px; height: 24px; border-radius: 7px;
  display: inline-flex; align-items: center; justify-content: center;
  background: var(--surface-3); color: var(--text-2); flex-shrink: 0;
}
.row.sel .pico { background: var(--surface); color: var(--accent); }
.pmain { display: flex; align-items: baseline; gap: 8px; flex: 1; min-width: 0; overflow: hidden; }
.ptitle { font-weight: 500; white-space: nowrap; overflow: hidden; flex-shrink: 0; max-width: 58%; }
.psub { font-size: 11.5px; color: var(--text-3); white-space: nowrap; overflow: hidden; flex: 1; }
.row.sel .psub { color: var(--accent-text); opacity: 0.78; }
.hl { font-weight: 700; color: var(--accent); }
.empty { padding: 28px; text-align: center; font-size: 13px; color: var(--text-3); }
.ftr {
  display: flex; align-items: center; justify-content: space-between; height: 32px;
  padding: 0 14px; border-top: 1px solid var(--divider); background: var(--surface-2);
  font-size: 11px; color: var(--text-3); flex-shrink: 0;
}
"#;

pub fn palette_popup_height(item_count: usize, category_count: usize) -> u32 {
    let rows = (item_count.max(1) as u32) * 38;
    let cats = (category_count as u32) * 26;
    (50 + rows + cats + 14 + 32).clamp(160, PALETTE_MAX_HEIGHT)
}

pub fn render_command_palette_shell_html(query: &str, list_html: &str) -> String {
    let mut h = String::with_capacity(4096 + list_html.len());
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(PALETTE_CSS);
    let _ = write!(
        h,
        r#"</style></head><body><div class="shell">
<div class="search-row"><span class="search-ico">{search}</span><input id="palette-input" type="text" value="{q}" placeholder="열린 탭, 명령, 북마크, 방문 기록 검색..." /><kbd>Esc</kbd></div>
<div id="palette-list" class="list">{list_html}</div>
<div class="ftr"><span><kbd>↑</kbd> <kbd>↓</kbd> 이동 · <kbd>Enter</kbd> 실행</span><span>명령 팔레트</span></div>
</div></body></html>"#,
        search = svg(Icon::Command, 16),
        q = html_escape(query),
    );
    h
}

pub fn render_command_palette_list_html(items: &[PaletteEntry], selected: usize, query: &str) -> String {
    if items.is_empty() {
        return r#"<div class="empty">일치하는 탭, 명령 또는 북마크가 없습니다.</div>"#.to_string();
    }
    let mut h = String::with_capacity(items.len() * 320);
    let mut last_cat = "";
    for (idx, item) in items.iter().enumerate() {
        if item.category != last_cat {
            last_cat = item.category;
            let _ = write!(h, r#"<div class="cat">{}</div>"#, html_escape(last_cat));
        }
        let sel = if idx == selected { " sel" } else { "" };
        let ico = match item.icon_kind {
            PaletteIcon::Tab => svg(Icon::PanelTop, 13),
            PaletteIcon::Command => svg(Icon::Command, 13),
            PaletteIcon::Bookmark => svg(Icon::Bookmark, 13),
            PaletteIcon::History => svg(Icon::History, 13),
            PaletteIcon::Search => svg(Icon::Search, 13),
            PaletteIcon::Theme => svg(Icon::Sun, 13),
            PaletteIcon::Shield => svg(Icon::ShieldCheck, 13),
            PaletteIcon::Zap => svg(Icon::Zap, 13),
            PaletteIcon::Book => svg(Icon::BookOpen, 13),
            PaletteIcon::Memory => svg(Icon::Cpu, 13),
        };
        let kbd = if item.shortcut.is_empty() {
            String::new()
        } else {
            format!("<kbd>{}</kbd>", html_escape(item.shortcut))
        };
        let _ = write!(
            h,
            r#"<div class="row{sel}"{sel_id} data-action="palette-exec:{idx}"><span class="pico">{ico}</span><div class="pmain"><span class="ptitle">{title}</span><span class="psub">{sub}</span></div>{kbd}</div>"#,
            sel_id = if idx == selected { r#" id="palette-sel""# } else { "" },
            title = highlight_escaped(&truncate_chars(&item.title, 38), query),
            sub = html_escape(&truncate_chars(&item.subtitle, 44)),
        );
    }
    h
}

// ── Find in Page Bar (`Ctrl+F`) ──

const FIND_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.find-shell {
  display: flex; align-items: center; gap: 6px; width: 100%; height: 100%;
  padding: 0 8px 0 12px; border: 1px solid var(--border); background: var(--surface);
}
.find-ico { color: var(--text-2); display: inline-flex; flex-shrink: 0; }
#find-input {
  flex: 1; min-width: 80px; height: 30px; border: none; background: transparent;
  color: var(--text); font-size: 13px; outline: none;
}
#find-count {
  font-size: 11.5px; color: var(--text-2); padding: 2px 7px; border-radius: 6px;
  background: var(--surface-3); white-space: nowrap; flex-shrink: 0;
}
#find-count.none { color: var(--danger); background: var(--danger-bg); }
.fbtn {
  display: flex; align-items: center; justify-content: center;
  width: 28px; height: 28px; border-radius: 8px; color: var(--text-2); flex-shrink: 0;
}
.fbtn:hover { background: var(--hover); color: var(--text); }
"#;

pub fn render_find_bar_html(query: &str, count_label: &str, no_match: bool) -> String {
    let mut h = String::with_capacity(2048);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(FIND_CSS);
    let cnt_cls = if no_match { "num none" } else { "num" };
    let _ = write!(
        h,
        r#"</style></head><body><div class="find-shell">
<span class="find-ico">{search}</span>
<input id="find-input" type="text" value="{q}" placeholder="페이지 내 찾기..." />
<span id="find-count" class="{cnt_cls}">{cnt}</span>
<div class="fbtn" data-action="find-prev">{up}</div>
<div class="fbtn" data-action="find-next">{down}</div>
<div class="fbtn" data-action="find-close">{close}</div>
</div></body></html>"#,
        search = svg(Icon::Search, 14),
        q = html_escape(query),
        cnt = html_escape(count_label),
        up = svg(Icon::ChevronUp, 14),
        down = svg(Icon::ChevronDown, 14),
        close = svg(Icon::Close, 13),
    );
    h
}

// ── Bottom-Left Link Hover Preview Pill (Blitz Content) ──

pub fn render_link_preview_html(url: &str) -> String {
    let secure = url.starts_with("https://");
    let ico = if secure {
        svg(Icon::Lock, 12)
    } else {
        svg(Icon::ExternalLink, 12)
    };
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><style>{THEME_CSS}
html, body {{ width: 100%; height: 100%; overflow: hidden; background: transparent; padding: 4px 6px; }}
.pill {{
  display: inline-flex; align-items: center; gap: 6px;
  height: 26px; max-width: 100%; padding: 0 11px;
  border-radius: 8px; background: var(--surface); border: 1px solid var(--border);
  box-shadow: var(--shadow-sm); font-size: 11.5px; color: var(--text-2);
  white-space: nowrap; overflow: hidden;
}}
.url {{ overflow: hidden; color: var(--text); }}
.hint {{ color: var(--text-3); font-size: 10.5px; margin-left: 4px; flex-shrink: 0; }}
</style></head><body><div class="pill">{ico}<span class="url">{u}</span><span class="hint">{hint}</span></div></body></html>"#,
        u = html_escape(&truncate_chars(url, 72)),
        hint = if cfg!(target_os = "macos") {
            "⌘+클릭 새 탭"
        } else {
            "Ctrl+클릭 새 탭"
        },
    )
}

// ── Built-in Page: Home Bento Dashboard (`rust://home`) ──

const HOME_CSS: &str = r#"
body { background: var(--surface-2); padding: 48px 28px 64px; }
.wrap { max-width: 920px; margin: 0 auto; }
.hero { display: flex; flex-direction: column; align-items: center; margin-bottom: 26px; text-align: center; }
.hero-brand { display: flex; align-items: center; gap: 12px; margin-bottom: 8px; }
.brand-name { font-size: 26px; font-weight: 700; letter-spacing: -0.025em; }
.brand-name span { color: var(--accent); }
.subtitle { font-size: 13px; color: var(--text-2); }
.search-wrap {
  display: flex; align-items: center; gap: 10px;
  width: 100%; max-width: 660px; height: 50px; margin: 0 auto 10px; padding: 0 14px 0 18px;
  border-radius: 25px; border: 1.5px solid var(--border);
  background: var(--surface); box-shadow: var(--shadow-sm);
}
.search-wrap:focus-within { border-color: var(--accent); box-shadow: 0 0 0 4px var(--accent-ring); }
.search-ico { color: var(--text-3); display: inline-flex; flex-shrink: 0; }
#home-search {
  flex: 1; height: 100%; border: none; background: transparent;
  color: var(--text); font-size: 15.5px; outline: none;
}
.search-kbds { display: flex; align-items: center; gap: 5px; flex-shrink: 0; }
.hint { text-align: center; font-size: 12px; color: var(--text-3); margin-bottom: 32px; }

.section-hdr { display: flex; align-items: center; justify-content: space-between; margin-bottom: 10px; }
.section-t { font-size: 12px; font-weight: 600; color: var(--text-2); letter-spacing: 0.02em; }
.section-link { font-size: 12px; color: var(--accent); text-decoration: none; font-weight: 500; }
.section-link:hover { text-decoration: underline; }

.dial-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(156px, 1fr)); gap: 10px; margin-bottom: 24px; }
.tile {
  display: flex; flex-direction: column; align-items: center; gap: 8px;
  padding: 14px 8px 12px; border-radius: 14px;
  background: var(--surface); border: 1px solid var(--divider);
  text-decoration: none; color: var(--text); box-shadow: var(--shadow-sm);
}
.tile:hover { border-color: var(--accent); background: var(--surface); }
.tile-ic {
  width: 42px; height: 42px; display: flex; align-items: center; justify-content: center;
  border-radius: 14px; color: #fff; font-size: 17px; font-weight: 700;
}
.tile-ic.img { background: var(--surface-2); border: 1px solid var(--divider); }
.tile-ic img { width: 22px; height: 22px; border-radius: 4px; }
.tile-t { font-size: 12.5px; font-weight: 600; text-align: center; white-space: nowrap; overflow: hidden; max-width: 100%; }
.tile-d { font-size: 11px; color: var(--text-3); white-space: nowrap; overflow: hidden; max-width: 100%; }

.bento { display: grid; grid-template-columns: 1.05fr 1fr; gap: 14px; margin-bottom: 24px; }
@media (max-width: 740px) {
  .bento { grid-template-columns: 1fr; }
}
.bcard {
  padding: 18px 20px; border-radius: 16px;
  background: var(--surface); border: 1px solid var(--border); box-shadow: var(--shadow-sm);
}
.bcard-hdr { display: flex; align-items: center; justify-content: space-between; margin-bottom: 14px; }
.bcard-title { display: flex; align-items: center; gap: 8px; font-size: 14px; font-weight: 600; }
.bcard-ico { color: var(--accent); display: inline-flex; }
.stat-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; margin-bottom: 14px; }
.stat-box { padding: 11px 13px; border-radius: 12px; background: var(--surface-2); border: 1px solid var(--divider); }
.stat-k { font-size: 11.5px; color: var(--text-2); margin-bottom: 3px; }
.stat-v { font-size: 19px; font-weight: 700; letter-spacing: -0.02em; }
.stat-v.green { color: var(--green); }
.mem-meter { padding: 10px 13px; border-radius: 12px; background: var(--surface-2); border: 1px solid var(--divider); }
.mem-top { display: flex; justify-content: space-between; font-size: 11.5px; color: var(--text-2); margin-bottom: 7px; }
.mem-track { height: 7px; border-radius: 4px; background: var(--surface-3); overflow: hidden; }
.mem-fill { height: 100%; border-radius: 4px; background: var(--green); }

.hist-list { display: flex; flex-direction: column; gap: 4px; }
.hrow {
  display: flex; align-items: center; gap: 9px; padding: 7px 9px;
  border-radius: 10px; text-decoration: none; color: var(--text); font-size: 12.5px;
}
.hrow:hover { background: var(--hover); }
.hrow-t { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; font-weight: 500; }
.hrow-time { font-size: 11px; color: var(--text-3); flex-shrink: 0; }
.hempty { padding: 28px 12px; text-align: center; font-size: 12.5px; color: var(--text-3); }

.keys {
  display: flex; flex-wrap: wrap; justify-content: center; gap: 10px 16px;
  padding: 12px 16px; border-radius: 14px; background: var(--surface); border: 1px solid var(--divider);
  font-size: 12px; color: var(--text-2);
}
.key-item { display: inline-flex; align-items: center; gap: 5px; }
"#;

// ponytail: positional args are all plain stats; bundle into a struct if a 9th shows up.
#[allow(clippy::too_many_arguments)]
pub fn render_home_page(
    bookmarks: &BookmarkStore,
    history: &History,
    working_set_mb: f64,
    tab_count: usize,
    unloaded_count: usize,
    blocked_count: usize,
    degraded_count: usize,
    dark_mode: bool,
) -> String {
    let mut tiles = String::new();
    let urls = bookmarks
        .custom
        .iter()
        .chain(bookmarks.bar.iter().filter_map(|n| match n {
            BookmarkNode::Url(item) => Some(item),
            _ => None,
        }))
        .take(10);
    for item in urls {
        let icon = match crate::favicon::cached_icon_url(&item.url) {
            Some(src) => format!(
                r#"<span class="tile-ic img"><img src="{}" /></span>"#,
                html_escape(&src)
            ),
            None => {
                let (initial, color) = monogram_badge(&item.title, &item.url);
                format!(r#"<span class="tile-ic" style="background:{color}">{initial}</span>"#)
            }
        };
        let _ = write!(
            tiles,
            r#"<a class="tile" href="{url}" title="{full}">{icon}<span class="tile-t">{title}</span><span class="tile-d">{domain}</span></a>"#,
            url = html_escape(&item.url),
            full = html_escape(&format!("{} — {}", item.title, item.url)),
            title = html_escape(&truncate_chars(&item.title, 14)),
            domain = html_escape(&short_host(&item.url)),
        );
    }
    if tiles.is_empty() {
        for (title, url) in [
            ("Wikipedia", "https://en.wikipedia.org/"),
            ("DuckDuckGo", "https://html.duckduckgo.com/html/"),
            ("Rust Book", "https://doc.rust-lang.org/book/"),
            ("Hacker News", "https://news.ycombinator.com/"),
            ("Rust Lang", "https://www.rust-lang.org/"),
        ] {
            let (initial, color) = monogram_badge(title, url);
            let _ = write!(
                tiles,
                r#"<a class="tile" href="{url}" title="{full}"><span class="tile-ic" style="background:{color}">{initial}</span><span class="tile-t">{title}</span><span class="tile-d">{domain}</span></a>"#,
                full = html_escape(&format!("{title} — {url}")),
                domain = html_escape(&short_host(url)),
            );
        }
    }

    let recent_items = history.recent(5);
    let mut recent_html = String::new();
    if recent_items.is_empty() {
        recent_html.push_str(r#"<div class="hempty">최근 방문한 페이지가 여기에 표시됩니다.</div>"#);
    } else {
        for item in &recent_items {
            let label = if item.title.trim().is_empty() {
                &item.url
            } else {
                &item.title
            };
            let _ = write!(
                recent_html,
                r#"<a class="hrow" href="{url}">{fav}<span class="hrow-t">{title}</span><span class="hrow-time num">{ago}</span></a>"#,
                url = html_escape(&item.url),
                fav = favicon_html(&item.title, &item.url),
                title = html_escape(&truncate_chars(label, 34)),
                ago = format_relative_time(item.last),
            );
        }
    }

    // Memory bar relative to a 450 MB conventional browser baseline.
    let mem_pct = ((working_set_mb / 450.0) * 100.0).clamp(4.0, 100.0);
    let cmd_k = if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl+K" };

    let mut h = String::with_capacity(14 * 1024);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>새 탭</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(HOME_CSS);
    let _ = write!(
        h,
        r#"</style></head><body><div class="wrap">
<div class="hero">
  <div class="hero-brand">{emblem}<div class="brand-name">Rust <span>Clean</span> Browser</div></div>
  <div class="subtitle">순수 Rust Blitz 엔진 · 트래커 제로 · 비활성 탭 즉시 메모리 회수</div>
</div>
<div class="search-wrap">
  <span class="search-ico">{search_ico}</span>
  <input id="home-search" type="text" placeholder="Google 검색 또는 웹 주소 입력" />
  <div class="search-kbds"><kbd>Enter</kbd></div>
</div>
<div class="hint">주소창 <kbd>Ctrl+L</kbd> · 명령 팔레트 <kbd>{cmd_k}</kbd> · 페이지 내 찾기 <kbd>Ctrl+F</kbd></div>

<div class="section-hdr">
  <span class="section-t">빠른 실행</span>
  <a class="section-link" href="rust://bookmarks">북마크 관리 →</a>
</div>
<div class="dial-grid">{tiles}</div>

<div class="bento">
  <div class="bcard">
    <div class="bcard-hdr">
      <div class="bcard-title"><span class="bcard-ico">{cpu_ico}</span><span>메모리 및 프라이버시</span></div>
      <span class="section-t num">북마크 {bm_count}</span>
    </div>
    <div class="stat-grid">
      <div class="stat-box"><div class="stat-k">현재 메모리</div><div class="stat-v green num">{working_set_mb:.0} MB</div></div>
      <div class="stat-box"><div class="stat-k">절전 / 전체 탭</div><div class="stat-v num">{unloaded_count} / {tab_count}</div></div>
      <div class="stat-box"><div class="stat-k">차단된 트래커</div><div class="stat-v num">{blocked_count}</div></div>
      <div class="stat-box"><div class="stat-k">방문 기록</div><div class="stat-v num">{hist_count}</div></div>
      <div class="stat-box"><div class="stat-k">불완전 로드</div><div class="stat-v num">{degraded_count}</div></div>
    </div>
    <div class="mem-meter">
      <div class="mem-top"><span>일반 브라우저(450 MB) 대비</span><span class="num">{working_set_mb:.0} MB 사용 중</span></div>
      <div class="mem-track"><div class="mem-fill" style="width:{mem_pct:.0}%"></div></div>
    </div>
  </div>
  <div class="bcard">
    <div class="bcard-hdr">
      <div class="bcard-title"><span class="bcard-ico">{hist_ico}</span><span>최근 방문 기록</span></div>
      <a class="section-link" href="rust://history">전체 보기 →</a>
    </div>
    <div class="hist-list">{recent_html}</div>
  </div>
</div>

<div class="keys">
  <span class="key-item"><kbd>Ctrl+T</kbd> 새 탭</span>
  <span class="key-item"><kbd>Ctrl+W</kbd> 닫기</span>
  <span class="key-item"><kbd>Ctrl+Shift+T</kbd> 복원</span>
  <span class="key-item"><kbd>Ctrl+K</kbd> 명령 팔레트</span>
  <span class="key-item"><kbd>Ctrl+F</kbd> 찾기</span>
  <span class="key-item"><kbd>Alt+R</kbd> 리더 모드</span>
  <span class="key-item"><kbd>Ctrl+H</kbd> 기록</span>
  <span class="key-item"><kbd>Ctrl+Shift+B</kbd> 북마크바</span>
</div>
</div></body></html>"#,
        emblem = brand_emblem_svg(dark_mode),
        search_ico = svg(Icon::Search, 18),
        cpu_ico = svg(Icon::Cpu, 16),
        hist_ico = svg(Icon::History, 16),
        bm_count = bookmarks.total_count(),
        hist_count = history.len(),
        degraded_count = degraded_count,
    );
    if cfg!(target_os = "macos") {
        h.replace("Ctrl+", "⌘")
            .replace("⌘Tab", "Ctrl+Tab")
            .replace("⌘클릭", "⌘+클릭")
    } else {
        h
    }
}

// ── Built-in Page: Bookmark Manager (`rust://bookmarks`) ──

const BOOKMARKS_CSS: &str = r#"
body { background: var(--surface-2); }
.top {
  display: flex; align-items: center; justify-content: space-between; gap: 16px;
  padding: 12px 24px; background: var(--surface); border-bottom: 1px solid var(--border);
}
.brand { display: flex; align-items: center; gap: 9px; font-size: 16px; font-weight: 700; }
.brand-ico { color: var(--accent); display: inline-flex; }
.search { display: flex; align-items: center; gap: 8px; flex: 1; max-width: 560px; }
#bm-search {
  flex: 1; height: 36px; padding: 0 16px; border-radius: 18px;
  border: 1.5px solid var(--border); background: var(--omni);
  color: var(--text); font-size: 13px; outline: none;
}
#bm-search:focus { background: var(--surface); border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-ring); }
.btn {
  display: inline-flex; align-items: center; gap: 6px; height: 34px; padding: 0 14px;
  border-radius: 17px; border: 1px solid var(--border); background: var(--surface);
  color: var(--text-2); font-size: 12.5px; font-weight: 500; text-decoration: none; white-space: nowrap;
}
.btn:hover { background: var(--hover); color: var(--text); }
.layout { display: flex; min-height: 600px; max-width: 1240px; margin: 0 auto; }
.side { width: 256px; flex-shrink: 0; padding: 14px 10px; }
.sb {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  padding: 8px 12px; border-radius: 10px; text-decoration: none;
  color: var(--text); font-size: 13px; margin-bottom: 2px;
}
.sb:hover { background: var(--hover); }
.sb.active, .sb.drop { background: var(--accent-bg); color: var(--accent-text); font-weight: 600; }
.sb-l { display: flex; align-items: center; gap: 8px; min-width: 0; overflow: hidden; }
.sb-n { font-size: 11px; color: var(--text-3); padding: 1px 7px; border-radius: 999px; background: var(--surface-3); }
.sb.active .sb-n { background: var(--surface); color: var(--accent-text); }
.main { flex: 1; padding: 20px 28px 56px; min-width: 0; }
.ptitle { margin: 0 0 14px; font-size: 18px; font-weight: 700; }
.folders { display: flex; flex-wrap: wrap; gap: 9px; margin-bottom: 16px; }
.fcard {
  display: flex; align-items: center; gap: 8px; padding: 10px 14px;
  border-radius: 12px; background: var(--surface); border: 1px solid var(--border);
  text-decoration: none; color: var(--text); font-size: 13px; font-weight: 500;
  box-shadow: var(--shadow-sm);
}
.fcard:hover { border-color: var(--accent); background: var(--hover); }
.fcard.drop { background: var(--accent-bg); color: var(--accent-text); border-color: var(--accent); font-weight: 600; }
.fcnt { color: var(--text-3); font-size: 11px; }
.ico-f { color: var(--text-2); display: inline-flex; flex-shrink: 0; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 14px; overflow: hidden; box-shadow: var(--shadow-sm); }
.row-wrap {
  display: flex; align-items: center; border-bottom: 1px solid var(--divider);
}
.row-wrap:last-child { border-bottom: none; }
.row-wrap:hover { background: var(--hover); }
.row {
  display: flex; align-items: center; gap: 12px; flex: 1; min-width: 0;
  padding: 11px 16px; text-decoration: none; color: var(--text); font-size: 13px;
}
.rtitle { font-weight: 500; white-space: nowrap; overflow: hidden; }
.rdom { font-size: 11px; padding: 2px 8px; border-radius: 999px; background: var(--surface-3); color: var(--text-2); flex-shrink: 0; }
.rurl { color: var(--text-3); font-size: 11.5px; margin-left: auto; white-space: nowrap; overflow: hidden; max-width: 320px; }
.rdel {
  display: flex; align-items: center; justify-content: center;
  width: 34px; height: 34px; margin-right: 8px; border-radius: 9px;
  color: var(--text-3); text-decoration: none; flex-shrink: 0;
}
.rdel:hover { background: var(--danger-bg); color: var(--danger); }
.empty { padding: 36px; text-align: center; background: var(--surface); border: 1px solid var(--border); border-radius: 14px; color: var(--text-3); font-size: 13px; }
"#;

/// Chrome-style 2-column Bookmark Manager (`rust://bookmarks`, `?folder=...`, `?q=...`).
pub fn render_bookmarks_page(bookmarks: &BookmarkStore, current_url: &str, drop_target_folder: Option<&str>) -> String {
    let query = parse_query_param(current_url, "q");
    let folder_param = parse_query_param(current_url, "folder");

    let active_key = if query.is_some() {
        "__search__".to_string()
    } else if let Some(f) = folder_param {
        f
    } else if !bookmarks.bar.is_empty() {
        "bar".to_string()
    } else if !bookmarks.other.is_empty() {
        "other".to_string()
    } else {
        "custom".to_string()
    };

    let mut side = String::new();
    let sb_cls = |key: &str| {
        if drop_target_folder == Some(key) {
            "sb drop"
        } else if active_key == key {
            "sb active"
        } else {
            "sb"
        }
    };
    let folder_svg = svg(Icon::Folder, 14);
    let star_svg = svg(Icon::StarOutline, 14);
    let _ = write!(
        side,
        r#"<a class="{}" data-bm-folder="custom" href="rust://bookmarks?folder=custom"><span class="sb-l"><span class="ico-f">{star_svg}</span><span>이 브라우저에서 추가</span></span><span class="sb-n num">{}</span></a>"#,
        sb_cls("custom"),
        bookmarks.custom.len(),
    );
    let _ = write!(
        side,
        r#"<a class="{}" data-bm-folder="bar" href="rust://bookmarks?folder=bar"><span class="sb-l"><span class="ico-f">{folder_svg}</span><span>북마크바</span></span><span class="sb-n num">{}</span></a>"#,
        sb_cls("bar"),
        count_urls(&bookmarks.bar),
    );
    render_sidebar_folders(&bookmarks.bar, "bar", 1, &active_key, drop_target_folder, &mut side);
    if !bookmarks.other.is_empty() {
        let _ = write!(
            side,
            r#"<a class="{}" data-bm-folder="other" href="rust://bookmarks?folder=other"><span class="sb-l"><span class="ico-f">{folder_svg}</span><span>기타 북마크</span></span><span class="sb-n num">{}</span></a>"#,
            sb_cls("other"),
            count_urls(&bookmarks.other),
        );
        render_sidebar_folders(&bookmarks.other, "other", 1, &active_key, drop_target_folder, &mut side);
    }

    let mut folders = String::new();
    let mut rows = String::new();
    let heading = if let Some(q) = &query {
        let matches = bookmarks.search(q);
        for item in &matches {
            let src = format!("url:{}", item.url);
            append_manager_url_row(&mut rows, item, &src);
        }
        format!("검색: \"{q}\" ({}개)", matches.len())
    } else {
        let (kind, path) = parse_folder_spec(&active_key);
        let (title, nodes) = bookmarks
            .resolve_folder(kind, &path)
            .unwrap_or_else(|| ("북마크바".to_string(), (&bookmarks.bar[..]).into()));
        for (i, node) in nodes.iter().enumerate() {
            match node {
                BookmarkNode::Folder(f) => {
                    let next_key = if path.is_empty() {
                        format!("{}-{}", kind_prefix(kind), i)
                    } else {
                        format!("{active_key}-{i}")
                    };
                    let fcard_cls = if drop_target_folder == Some(next_key.as_str()) {
                        "fcard drop"
                    } else {
                        "fcard"
                    };
                    let _ = write!(
                        folders,
                        r#"<a class="{fcard_cls}" data-bm-folder="{next_key}" href="rust://bookmarks?folder={next_key}"><span class="ico-f">{folder_svg}</span><span>{name}</span><span class="fcnt num">{}</span></a>"#,
                        count_urls(&f.children),
                        name = html_escape(&f.title),
                    );
                }
                BookmarkNode::Url(item) => {
                    let item_key = if path.is_empty() {
                        format!("{}-{}", kind_prefix(kind), i)
                    } else {
                        format!("{active_key}-{i}")
                    };
                    append_manager_url_row(&mut rows, item, &item_key);
                }
            }
        }
        title
    };

    let mut body = String::new();
    if folders.is_empty() && rows.is_empty() {
        body.push_str(r#"<div class="empty">표시할 북마크가 없습니다.</div>"#);
    } else {
        if !folders.is_empty() {
            let _ = write!(body, r#"<div class="folders">{folders}</div>"#);
        }
        if !rows.is_empty() {
            let _ = write!(body, r#"<div class="card">{rows}</div>"#);
        }
    }

    let mut h = String::with_capacity(10 * 1024 + side.len() + body.len());
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>북마크</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(BOOKMARKS_CSS);
    let _ = write!(
        h,
        r#"</style></head><body>
<div class="top"><div class="brand"><span class="brand-ico">{bm_ico}</span><span class="num">북마크 ({total})</span></div>
<div class="search"><input id="bm-search" type="text" value="{search_val}" placeholder="북마크 제목 또는 URL 검색 (Enter)" /><a class="btn" href="rust://bookmarks">전체</a><a class="btn" href="rust://bookmarks?sync=1">{sync_ico}<span>Chrome 동기화</span></a></div></div>
<div class="layout"><div class="side">{side}</div><div class="main"><h2 class="ptitle">{heading}</h2>{body}</div></div>
</body></html>"#,
        bm_ico = svg(Icon::Bookmark, 18),
        sync_ico = svg(Icon::Reload, 13),
        total = bookmarks.total_count(),
        search_val = html_escape(query.as_deref().unwrap_or("")),
        heading = html_escape(&heading),
    );
    h
}

fn render_sidebar_folders(
    nodes: &[BookmarkNode],
    key_prefix: &str,
    depth: usize,
    active_key: &str,
    drop_target_folder: Option<&str>,
    out: &mut String,
) {
    if depth > 4 {
        return;
    }
    let pad = 12 + depth * 14;
    let folder_svg = svg(Icon::Folder, 14);
    for (i, node) in nodes.iter().enumerate() {
        if let BookmarkNode::Folder(folder) = node {
            let key = format!("{key_prefix}-{i}");
            let cls = if drop_target_folder == Some(key.as_str()) {
                "sb drop"
            } else if active_key == key {
                "sb active"
            } else {
                "sb"
            };
            let _ = write!(
                out,
                r#"<a class="{cls}" style="padding-left:{pad}px" data-bm-folder="{key}" href="rust://bookmarks?folder={key}"><span class="sb-l"><span class="ico-f">{folder_svg}</span><span>{name}</span></span><span class="sb-n num">{}</span></a>"#,
                count_urls(&folder.children),
                name = html_escape(&truncate_chars(&folder.title, 18)),
            );
            render_sidebar_folders(&folder.children, &key, depth + 1, active_key, drop_target_folder, out);
        }
    }
}

fn append_manager_url_row(out: &mut String, item: &BookmarkItem, src_spec: &str) {
    let title = html_escape(&truncate_chars(&item.title, 54));
    let del_q: String = url::form_urlencoded::byte_serialize(item.url.as_bytes()).collect();
    let _ = write!(
        out,
        r#"<div class="row-wrap"><a class="row" data-bm-src="{src}" data-bm-title="{title}" href="{url}">{fav}<span class="rtitle">{title}</span><span class="rdom">{domain}</span><span class="rurl">{url_short}</span></a><a class="rdel" href="rust://bookmarks?del={del_q}">{trash}</a></div>"#,
        src = html_escape(src_spec),
        url = html_escape(&item.url),
        fav = favicon_html(&item.title, &item.url),
        domain = html_escape(&short_host(&item.url)),
        url_short = html_escape(&truncate_chars(&item.url, 48)),
        trash = svg(Icon::Trash, 13),
    );
}

// ── Built-in Page: History Timeline (`rust://history`) ──

const HISTORY_CSS: &str = r#"
body { background: var(--surface-2); }
.top {
  display: flex; align-items: center; justify-content: space-between; gap: 16px;
  padding: 12px 24px; background: var(--surface); border-bottom: 1px solid var(--border);
}
.brand { display: flex; align-items: center; gap: 9px; font-size: 16px; font-weight: 700; }
.brand-ico { color: var(--accent); display: inline-flex; }
.search { display: flex; align-items: center; gap: 8px; flex: 1; max-width: 560px; }
#hist-search {
  flex: 1; height: 36px; padding: 0 16px; border-radius: 18px;
  border: 1.5px solid var(--border); background: var(--omni);
  color: var(--text); font-size: 13px; outline: none;
}
#hist-search:focus { background: var(--surface); border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-ring); }
.btn {
  display: inline-flex; align-items: center; gap: 6px; height: 34px; padding: 0 14px;
  border-radius: 17px; border: 1px solid var(--border); background: var(--surface);
  color: var(--text-2); font-size: 12.5px; font-weight: 500; text-decoration: none; white-space: nowrap;
}
.btn:hover { background: var(--hover); color: var(--text); }
.btn.danger:hover { background: var(--danger-bg); color: var(--danger); border-color: transparent; }
.wrap { max-width: 920px; margin: 0 auto; padding: 24px 24px 64px; }
.group-t { font-size: 12.5px; font-weight: 700; color: var(--text-2); margin: 18px 0 8px 4px; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 14px; overflow: hidden; box-shadow: var(--shadow-sm); margin-bottom: 12px; }
.row-wrap { display: flex; align-items: center; border-bottom: 1px solid var(--divider); }
.row-wrap:last-child { border-bottom: none; }
.row-wrap:hover { background: var(--hover); }
.row { display: flex; align-items: center; gap: 11px; flex: 1; min-width: 0; padding: 10px 16px; text-decoration: none; color: var(--text); font-size: 13px; }
.htitle { font-weight: 500; white-space: nowrap; overflow: hidden; max-width: 360px; }
.hurl { flex: 1; min-width: 0; font-size: 11.5px; color: var(--text-3); white-space: nowrap; overflow: hidden; }
.hcnt { font-size: 11px; padding: 2px 7px; border-radius: 999px; background: var(--surface-3); color: var(--text-2); flex-shrink: 0; }
.htime { font-size: 11.5px; color: var(--text-3); flex-shrink: 0; }
.rdel { display: flex; align-items: center; justify-content: center; width: 34px; height: 34px; margin-right: 8px; border-radius: 9px; color: var(--text-3); text-decoration: none; flex-shrink: 0; }
.rdel:hover { background: var(--danger-bg); color: var(--danger); }
.empty { padding: 48px; text-align: center; background: var(--surface); border: 1px solid var(--border); border-radius: 14px; color: var(--text-3); font-size: 13px; }
"#;

pub fn render_history_page(history: &History, current_url: &str) -> String {
    let query = parse_query_param(current_url, "q");
    let items = history.search_history(query.as_deref().unwrap_or(""), 120);
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());

    let mut groups: [(&'static str, Vec<&HistoryItem>); 4] = [
        ("오늘", Vec::new()),
        ("어제", Vec::new()),
        ("이번 주", Vec::new()),
        ("이전 기록", Vec::new()),
    ];
    for item in &items {
        let age = now_secs.saturating_sub(item.last);
        let idx = if age < 86_400 {
            0
        } else if age < 2 * 86_400 {
            1
        } else if age < 7 * 86_400 {
            2
        } else {
            3
        };
        groups[idx].1.push(item);
    }

    let mut body = String::new();
    let trash_svg = svg(Icon::Trash, 13);
    for (label, list) in groups {
        if list.is_empty() {
            continue;
        }
        let _ = write!(body, r#"<div class="group-t">{label}</div><div class="card">"#);
        for item in list {
            let title = if item.title.trim().is_empty() {
                &item.url
            } else {
                &item.title
            };
            let del_q: String = url::form_urlencoded::byte_serialize(item.url.as_bytes()).collect();
            let _ = write!(
                body,
                r#"<div class="row-wrap"><a class="row" href="{url}">{fav}<span class="htitle">{title}</span><span class="hurl">{url_short}</span><span class="hcnt num">{cnt}회</span><span class="htime num">{ago}</span></a><a class="rdel" href="rust://history?del={del_q}">{trash_svg}</a></div>"#,
                url = html_escape(&item.url),
                fav = favicon_html(&item.title, &item.url),
                title = html_escape(&truncate_chars(title, 44)),
                url_short = html_escape(&truncate_chars(strip_url_scheme(&item.url), 44)),
                cnt = item.count,
                ago = format_relative_time(item.last),
            );
        }
        body.push_str("</div>");
    }
    if body.is_empty() {
        body.push_str(r#"<div class="empty">표시할 방문 기록이 없습니다.</div>"#);
    }

    let mut h = String::with_capacity(6144 + body.len());
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>방문 기록</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(HISTORY_CSS);
    let _ = write!(
        h,
        r#"</style></head><body>
<div class="top">
  <div class="brand"><span class="brand-ico">{hist_ico}</span><span class="num">방문 기록 ({total})</span></div>
  <div class="search">
    <input id="hist-search" type="text" value="{search_val}" placeholder="방문 기록 제목 또는 URL 검색 (Enter)" />
    <a class="btn" href="rust://history">전체</a>
    <a class="btn danger" href="rust://history?clear=1">{trash_all}<span>기록 지우기</span></a>
  </div>
</div>
<div class="wrap">{body}</div>
</body></html>"#,
        hist_ico = svg(Icon::History, 18),
        trash_all = svg(Icon::Trash, 13),
        total = history.len(),
        search_val = html_escape(query.as_deref().unwrap_or("")),
    );
    h
}

fn kind_prefix(kind: BookmarkRootKind) -> &'static str {
    match kind {
        BookmarkRootKind::Bar => "bar",
        BookmarkRootKind::Other => "other",
        BookmarkRootKind::Custom => "custom",
    }
}

/// True when a host looks like an IDN homograph lure: punycode labels or mixed
/// Latin + Cyrillic/Greek/fullwidth characters (e.g. `аpple.com` in Cyrillic).
/// Pure non-Latin hosts (e.g. 한글 도메인) are NOT flagged.
pub(crate) fn is_suspicious_host(url: &str) -> bool {
    // Use the raw host text: `Url` normalizes IDNs to punycode, which would
    // flag every legitimate non-Latin domain (e.g. 한글.kr -> xn--...).
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = host_port.split(':').next().unwrap_or("");
    if host.is_empty() {
        return false;
    }
    if host
        .to_ascii_lowercase()
        .split('.')
        .any(|label| label.starts_with("xn--"))
    {
        return true;
    }
    let has_latin = host.chars().any(|c| c.is_ascii_alphabetic());
    let has_lookalike = host
        .chars()
        .any(|c| matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0370}'..='\u{03FF}' | '\u{FF00}'..='\u{FFEF}'));
    has_latin && has_lookalike
}

pub fn parse_query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    let query = query.split('#').next().unwrap_or(query);
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

// ── Built-in Page: Error Page ──

pub fn render_error_page(url: &str, message: &str, dark_mode: bool) -> String {
    let mut h = String::with_capacity(4096);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>오류</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(
        r#"body { background: var(--surface-2); padding: 80px 24px; }
.card {
  max-width: 600px; margin: 0 auto; padding: 34px 32px;
  border-radius: 18px; background: var(--surface); border: 1px solid var(--border);
  box-shadow: var(--shadow-md);
}
.ic { margin-bottom: 18px; }
h1 { font-size: 21px; font-weight: 700; margin-bottom: 10px; letter-spacing: -0.02em; }
p { color: var(--text-2); font-size: 14px; line-height: 1.6; margin-bottom: 12px; }
.url-chip {
  padding: 9px 12px; border-radius: 10px; background: var(--surface-2);
  border: 1px solid var(--divider); color: var(--text-2);
  font-size: 12.5px; word-break: break-all; margin-bottom: 22px;
}
.actions { display: flex; flex-wrap: wrap; gap: 10px; }
.btn {
  display: inline-flex; align-items: center; gap: 7px; height: 36px; padding: 0 18px;
  border-radius: 18px; font-size: 13px; font-weight: 600; text-decoration: none;
  border: 1px solid var(--border); color: var(--text); background: var(--surface);
}
.btn:hover { background: var(--hover); }
.btn.primary { background: var(--accent); border-color: var(--accent); color: #ffffff; }
.btn.primary:hover { opacity: 0.92; }
"#,
    );
    let open_js_q: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
    let _ = write!(
        h,
        r#"</style></head><body><div class="card">
  <div class="ic">{emblem}</div>
  <h1>페이지를 열 수 없습니다</h1>
  <p>{message}</p>
  <div class="url-chip">{url}</div>
  <div class="actions">
    <a class="btn primary" href="{url}">{reload_ico}<span>다시 시도</span></a>
    <a class="btn" href="rust://open-js?url={open_js_q}">{js_ico}<span>JS 모드로 열기</span></a>
    <a class="btn" href="rust://home">{home_ico}<span>새 탭으로</span></a>
  </div>
</div></body></html>"#,
        emblem = error_emblem_svg(dark_mode),
        url = html_escape(url),
        message = html_escape(message),
        reload_ico = svg(Icon::Reload, 14),
        js_ico = svg(Icon::Code, 14),
        home_ico = svg(Icon::Home, 14),
    );
    h
}

/// Highlights case-insensitive occurrences of `query` in `text` while HTML-escaping safely.
fn highlight_escaped(text: &str, query: &str) -> String {
    let q = query.trim();
    if q.is_empty() || text.is_empty() {
        return html_escape(text);
    }
    let lower_q = q.to_lowercase();
    if lower_q.is_empty() {
        return html_escape(text);
    }
    let lower_text = text.to_lowercase();
    let ranges: Vec<(usize, usize)> = if lower_text.len() == text.len() {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        while let Some(rel) = lower_text[cursor..].find(&lower_q) {
            let start = cursor + rel;
            let end = start + lower_q.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                out.push((start, end));
            }
            cursor = end.max(cursor + 1);
        }
        out
    } else {
        Vec::new()
    };

    let ranges = if !ranges.is_empty() || lower_text.len() == text.len() {
        ranges
    } else {
        // Unicode fallback when `to_lowercase()` changes byte lengths (e.g. `İ` or `ẞ`).
        let needle_chars: Vec<char> = lower_q.chars().collect();
        let char_starts: Vec<(usize, char)> = text
            .char_indices()
            .flat_map(|(b, ch)| ch.to_lowercase().map(move |lc| (b, lc)))
            .collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i + needle_chars.len() <= char_starts.len() {
            let matched = char_starts[i..i + needle_chars.len()]
                .iter()
                .zip(&needle_chars)
                .all(|((_, a), b)| a == b);
            if matched {
                let start = char_starts[i].0;
                let end = if i + needle_chars.len() < char_starts.len() {
                    char_starts[i + needle_chars.len()].0
                } else {
                    text.len()
                };
                if start < end && text.is_char_boundary(start) && text.is_char_boundary(end) {
                    out.push((start, end));
                }
                i += needle_chars.len();
            } else {
                i += 1;
            }
        }
        out
    };

    if ranges.is_empty() {
        return html_escape(text);
    }
    let mut out = String::with_capacity(text.len() + 32);
    let mut cursor = 0usize;
    for (start, end) in ranges {
        if start < cursor {
            continue;
        }
        out.push_str(&html_escape(&text[cursor..start]));
        out.push_str(r#"<span class="hl">"#);
        out.push_str(&html_escape(&text[start..end]));
        out.push_str("</span>");
        cursor = end;
    }
    out.push_str(&html_escape(&text[cursor..]));
    out
}

fn strip_url_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

fn format_relative_time(ts: u64) -> String {
    if ts == 0 {
        return "이전".to_string();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let diff = now.saturating_sub(ts);
    if diff < 60 {
        "방금 전".to_string()
    } else if diff < 3600 {
        format!("{}분 전", diff / 60)
    } else if diff < 86_400 {
        format!("{}시간 전", diff / 3600)
    } else {
        format!("{}일 전", diff / 86_400)
    }
}

pub(crate) fn short_host(url: &str) -> String {
    let host = strip_url_scheme(url).split('/').next().unwrap_or("");
    truncate_chars(host.strip_prefix("www.").unwrap_or(host), 18)
}

fn monogram_badge(title: &str, url: &str) -> (char, &'static str) {
    // Brand-cohesive badge hues: browser blue, shield teal/green, warm amber, slate.
    const PALETTE: &[&str] = &[
        "#1b6ef3", "#0f8a5f", "#0891b2", "#1d4ed8", "#0d9488", "#b45309", "#475569", "#0369a1",
    ];
    let host = short_host(url);
    let source = if !host.is_empty() { host.as_str() } else { title };
    let ch = source
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('•');
    let hash: usize = url
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
    (ch, PALETTE[hash % PALETTE.len()])
}

pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_idn_homographs_but_not_pure_hangul_hosts() {
        assert!(is_suspicious_host("https://xn--pple-43d.com/"));
        assert!(is_suspicious_host("https://\u{0430}pple.com/")); // Cyrillic а + latin
        assert!(!is_suspicious_host("https://www.rust-lang.org/"));
        assert!(!is_suspicious_host("https://한글.kr/"));
        assert!(!is_suspicious_host("rust://home"));
    }

    #[test]
    fn maps_fluent_icon_entities_to_unicode() {
        let html = "<b>&#xE72B;</b>&amp;&#xE8B7;&#xE72E;&#xEFFF;&#xE7";
        assert_eq!(substitute_icon_entities(html), "<b>←</b>&amp;▸•&#xE7");
    }

    #[test]
    fn chrome_html_reflects_state_and_escapes() {
        let mut tab = Tab::new(1, "https://a.test/?x=<b>".into());
        tab.title = "T&T".into();
        let tabs = vec![tab];
        let bm = BookmarkStore::default();
        let html = render_chrome_html(&ChromeRenderState {
            tabs: &tabs,
            active_idx: 0,
            bookmarks: &bm,
            can_back: false,
            can_forward: false,
            working_set_mb: 42.4,
            blocked_count: 7,
            filter_enabled: true,
            maximized: true,
            dark_mode: false,
            show_bookmarks_bar: true,
            theme_pref: ThemePreference::System,
            status_note: Some("메모리 정리됨"),
            url_override: Some("typed"),
            drop_target_folder: None,
        });
        assert!(html.contains(r#"value="typed""#));
        assert!(html.contains("T&amp;T"));
        assert!(html.contains("42 MB"));
        assert!(html.contains("Clean"));
        assert!(html.contains("메모리 정리됨"));
        assert!(html.contains("<svg"));
    }

    #[test]
    fn loading_tab_renders_animated_spinner_and_blitz_animates_it() {
        let mut tab = Tab::new(1, "https://a.test/".into());
        tab.is_loading = true;
        let tabs = vec![tab];
        let bm = BookmarkStore::default();
        let html = render_chrome_html(&ChromeRenderState {
            tabs: &tabs,
            active_idx: 0,
            bookmarks: &bm,
            can_back: false,
            can_forward: false,
            working_set_mb: 10.0,
            blocked_count: 0,
            filter_enabled: true,
            maximized: false,
            dark_mode: false,
            show_bookmarks_bar: true,
            theme_pref: ThemePreference::System,
            status_note: None,
            url_override: None,
            drop_target_folder: None,
        });
        assert!(html.contains(r#"class="fav-spin""#));
        assert!(html.contains("@keyframes tab-spin"));
        assert!(html.contains("progress-bar"));

        let mut doc = blitz_html::HtmlDocument::from_html(
            &html,
            blitz_dom::DocumentConfig {
                viewport: Some(blitz_traits::shell::Viewport::new(
                    800,
                    120,
                    1.0,
                    blitz_traits::shell::ColorScheme::Light,
                )),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        assert!(doc.is_animating(), "loading spinner must trigger Blitz animation ticks");
        doc.resolve(0.2);
        assert!(doc.is_animating());

        // Hovering over a tab title's anonymous inline root must report `Default` cursor, not `Text`.
        doc.set_hover_to(60.0, 22.0);
        assert_eq!(doc.get_cursor(), Some(winit::cursor::CursorIcon::Default));
    }

    #[test]
    fn highlights_escaped_query_without_xss() {
        let out = highlight_escaped("<script>Rust Browser</script>", "rust");
        assert!(out.contains(r#"<span class="hl">Rust</span>"#));
        assert!(!out.contains("<script>"));
    }

    #[test]
    fn home_and_history_render_cleanly_in_blitz() {
        let bm = BookmarkStore::default();
        let mut hist = History::load(None);
        hist.record("https://doc.rust-lang.org/book/", "The Rust Programming Language");
        let home = render_home_page(&bm, &hist, 38.0, 3, 2, 14, 1, true);
        assert!(home.contains("The Rust Programming Language"));
        assert!(home.contains("38 MB"));

        let hist_html = render_history_page(&hist, "rust://history");
        assert!(hist_html.contains("The Rust Programming Language"));

        let mut doc = blitz_html::HtmlDocument::from_html(
            &home,
            blitz_dom::DocumentConfig {
                viewport: Some(blitz_traits::shell::Viewport::new(
                    1024,
                    768,
                    1.0,
                    blitz_traits::shell::ColorScheme::Dark,
                )),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        assert!(doc.get_element_by_id("home-search").is_some());
    }

    #[test]
    fn palette_find_suggest_and_error_render_and_resolve_in_blitz() {
        let entries = vec![PaletteEntry {
            action: "new-tab".into(),
            category: "빠른 명령",
            title: "새 탭 열기".into(),
            subtitle: "대시보드".into(),
            shortcut: "Ctrl+T",
            icon_kind: PaletteIcon::Command,
        }];
        let list_html = render_command_palette_list_html(&entries, 0, "새 탭");
        let palette_html = render_command_palette_shell_html("새 탭", &list_html);
        assert!(palette_html.contains("id=\"palette-input\""));
        assert!(palette_html.contains("id=\"palette-sel\""));

        let mut pdoc = blitz_html::HtmlDocument::from_html(
            &palette_html,
            blitz_dom::DocumentConfig {
                viewport: Some(blitz_traits::shell::Viewport::new(
                    PALETTE_WIDTH,
                    240,
                    1.0,
                    blitz_traits::shell::ColorScheme::Dark,
                )),
                html_parser_provider: Some(std::sync::Arc::new(blitz_html::HtmlProvider)),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        pdoc.resolve(0.0);
        let list_id = pdoc.get_element_by_id("palette-list").expect("palette-list");
        pdoc.mutate()
            .set_inner_html(list_id, &render_command_palette_list_html(&[], 0, "none"));
        pdoc.resolve(0.1);
        assert!(pdoc.get_element_by_id("palette-sel").is_none());

        let find_html = render_find_bar_html("rust", "1 / 4", false);
        assert!(find_html.contains("1 / 4"));
        let suggest_html = render_suggest_popup_html(
            &[Suggestion {
                url: "https://rust-lang.org/".into(),
                title: "Rust".into(),
                kind: SuggestKind::OpenTab,
            }],
            0,
            "rust",
        );
        assert!(suggest_html.contains("열린 탭"));
        assert!(render_error_page("https://a.test/", "timeout", true).contains("rust://open-js"));
        assert!(render_link_preview_html("https://rust-lang.org/").contains("+클릭 새 탭"));
        assert_eq!(chrome_height(false), TITLEBAR_HEIGHT + TOOLBAR_HEIGHT);
        assert_eq!(chrome_height(true), TITLEBAR_HEIGHT + TOOLBAR_HEIGHT + BMBAR_HEIGHT);

        // Verify actual Taffy layout geometry of chrome elements (#omni-box and #url-bar).
        let chrome_html = render_chrome_html(&ChromeRenderState {
            tabs: &[Tab::new(1, "https://rust-lang.org/".into())],
            active_idx: 0,
            bookmarks: &BookmarkStore::default(),
            can_back: false,
            can_forward: false,
            working_set_mb: 32.0,
            blocked_count: 5,
            filter_enabled: true,
            maximized: false,
            dark_mode: true,
            show_bookmarks_bar: true,
            theme_pref: ThemePreference::Dark,
            status_note: None,
            url_override: None,
            drop_target_folder: None,
        });
        let mut cdoc = blitz_html::HtmlDocument::from_html(
            &chrome_html,
            blitz_dom::DocumentConfig {
                viewport: Some(blitz_traits::shell::Viewport::new(
                    1280,
                    chrome_height(true),
                    1.0,
                    blitz_traits::shell::ColorScheme::Dark,
                )),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        cdoc.resolve(0.0);
        let omni_id = cdoc.get_element_by_id("omni-box").expect("omni-box");
        let url_id = cdoc.get_element_by_id("url-bar").expect("url-bar");
        let omni_size = cdoc.get_node(omni_id).unwrap().final_layout().size;
        let url_size = cdoc.get_node(url_id).unwrap().final_layout().size;
        assert!(
            omni_size.width > 300.0 && omni_size.height >= 32.0,
            "omni={omni_size:?}"
        );
        assert!(url_size.width > 200.0 && url_size.height >= 24.0, "url={url_size:?}");
    }

    #[test]
    fn preserves_host_letters_and_unicode_highlights() {
        let suggest = render_suggest_popup_html(
            &[Suggestion {
                url: "https://threads.net/@rust".into(),
                title: "Threads".into(),
                kind: SuggestKind::History,
            }],
            0,
            "rust",
        );
        assert!(suggest.contains("— threads.net/@"), "{suggest}");
        assert!(!suggest.contains("— reads.net"), "{suggest}");

        let mut hist = History::load(None);
        hist.record("https://twitch.tv/rust", "Twitch");
        let hist_html = render_history_page(&hist, "rust://history");
        assert!(hist_html.contains("twitch.tv"), "{hist_html}");

        // Equal-byte-length net delta from `İ` (+1) and `ẞ` (-1) must not panic on non-char-boundaries.
        let hl = highlight_escaped("İẞ Rust test", "rust");
        assert!(hl.contains(r#"<span class="hl">Rust</span>"#), "{hl}");
    }
}
