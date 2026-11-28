mod pages;
mod popups;
mod sidebar;

#[allow(unused_imports)]
pub(crate) use pages::is_suspicious_host;
pub use pages::{
    parse_query_param, render_bookmarks_page, render_error_page, render_history_page, render_home_page,
    render_welcome_page,
};
pub use popups::{
    PaletteEntry, PaletteIcon, downloads_popup_height, estimate_popup_height, palette_popup_height,
    render_bookmark_popup_html, render_command_palette_list_html, render_command_palette_shell_html,
    render_downloads_popup_html, render_find_bar_html, render_suggest_popup_html, suggest_popup_height,
};
pub use sidebar::{SidebarMotion, SidebarPicker, SidebarRenderState, chat_input_height, render_sidebar_html};

use crate::bookmarks::{BookmarkItem, BookmarkNode, BookmarkStore};
use crate::cleaner::html_escape;
use crate::icons::{Icon, svg};
use crate::prefs::ThemePreference;
use crate::tabs::Tab;
use pages::is_loopback_http;
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
pub const DOWNLOADS_POPUP_WIDTH: u32 = 388;
pub const DOWNLOADS_POPUP_MAX_HEIGHT: u32 = 440;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    InProgress,
    Completed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub path: Option<std::path::PathBuf>,
    pub status: DownloadStatus,
    pub size_bytes: Option<u64>,
    pub started_at: u64,
}

pub const fn chrome_height(show_bookmarks_bar: bool) -> u32 {
    TITLEBAR_HEIGHT + TOOLBAR_HEIGHT + if show_bookmarks_bar { BMBAR_HEIGHT } else { 0 }
}

/// Design tokens shared by every built-in page and popup — Neutral Paper & Obsidian
/// minimal monochrome system with crisp cobalt focus accents.
pub(super) const THEME_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --frame: #eff1f4;
  --frame-hover: rgba(0, 0, 0, 0.048);
  --surface: #ffffff;
  --surface-2: #f7f8fa;
  --surface-3: #eef0f4;
  --omni: #f1f3f6;
  --omni-hover: #e8ebef;
  --border: rgba(0, 0, 0, 0.075);
  --border-strong: rgba(0, 0, 0, 0.13);
  --divider: rgba(0, 0, 0, 0.055);
  --tab-specular: rgba(255, 255, 255, 0.96);
  --text: #14161a;
  --text-2: #5b616e;
  --text-3: #8b919e;
  --ink: #14161a;
  --ink-fg: #ffffff;
  --accent: #2563eb;
  --accent-hover: #1d4ed8;
  --accent-grad: linear-gradient(180deg, #2563eb 0%, #1d4ed8 100%);
  --accent-bg: rgba(37, 99, 235, 0.09);
  --accent-text: #1d4ed8;
  --accent-ring: rgba(37, 99, 235, 0.18);
  --green: #0d9468;
  --green-bg: rgba(13, 148, 104, 0.09);
  --green-text: #0b6e4e;
  --amber: #b45309;
  --amber-bg: rgba(217, 119, 6, 0.10);
  --amber-text: #92400e;
  --hover: rgba(0, 0, 0, 0.045);
  --press: rgba(0, 0, 0, 0.085);
  --danger: #dc2626;
  --danger-bg: rgba(220, 38, 38, 0.09);
  --shadow-xs: 0 1px 2px rgba(0, 0, 0, 0.04);
  --shadow-sm: 0 2px 6px rgba(0, 0, 0, 0.05), 0 1px 2px rgba(0, 0, 0, 0.03);
  --shadow-md: 0 10px 28px rgba(0, 0, 0, 0.08), 0 2px 6px rgba(0, 0, 0, 0.03);
}
@media (prefers-color-scheme: dark) {
  :root {
    --frame: #0e0f12;
    --frame-hover: rgba(255, 255, 255, 0.055);
    --surface: #18191d;
    --surface-2: #121317;
    --surface-3: #22242a;
    --omni: #111216;
    --omni-hover: #1c1e24;
    --border: rgba(255, 255, 255, 0.08);
    --border-strong: rgba(255, 255, 255, 0.14);
    --divider: rgba(255, 255, 255, 0.055);
    --tab-specular: rgba(255, 255, 255, 0.07);
    --text: #f1f2f5;
    --text-2: #9499a6;
    --text-3: #636875;
    --ink: #f1f2f5;
    --ink-fg: #111216;
    --accent: #60a5fa;
    --accent-hover: #7cb6fb;
    --accent-grad: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
    --accent-bg: rgba(96, 165, 250, 0.13);
    --accent-text: #bfdbfe;
    --accent-ring: rgba(96, 165, 250, 0.24);
    --green: #34d399;
    --green-bg: rgba(52, 211, 153, 0.11);
    --green-text: #86efac;
    --amber: #fbbf24;
    --amber-bg: rgba(251, 191, 36, 0.12);
    --amber-text: #fde68a;
    --hover: rgba(255, 255, 255, 0.055);
    --press: rgba(255, 255, 255, 0.105);
    --danger: #f87171;
    --danger-bg: rgba(248, 113, 113, 0.13);
    --shadow-xs: 0 1px 2px rgba(0, 0, 0, 0.30);
    --shadow-sm: 0 2px 6px rgba(0, 0, 0, 0.36), inset 0 1px 0 rgba(255, 255, 255, 0.04);
    --shadow-md: 0 12px 32px rgba(0, 0, 0, 0.52), 0 2px 6px rgba(0, 0, 0, 0.32);
  }
}
* { box-sizing: border-box; margin: 0; padding: 0; user-select: none; }
body {
  font-family: "KoPubWorldDotum", "KoPubWorld돋움체", "KoPubWorldDotum Medium", "KoPub Dotum", "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", -apple-system, "SF Pro Text", "Apple SD Gothic Neo", "Noto Sans KR", "Malgun Gothic", sans-serif;
  color: var(--text);
  cursor: default;
  line-height: 1.45;
}
a, a * { cursor: pointer; }
input { font-family: inherit; user-select: text; cursor: text; }
.svg-ico { display: block; flex-shrink: 0; pointer-events: none; }
.num { font-variant-numeric: tabular-nums; }
.fav { display: inline-flex; align-items: center; justify-content: center; width: 16px; height: 16px; border-radius: 4px; color: #fff; font-size: 10px; font-weight: 700; flex-shrink: 0; }
img.fav { border-radius: 3px; }
.fav.glyph { color: var(--text-2); background: transparent; }
kbd {
  display: inline-flex; align-items: center; justify-content: center;
  height: 20px; padding: 0 6px; border-radius: 5px;
  font-family: inherit; font-size: 10.5px; font-weight: 600; letter-spacing: 0.01em;
  color: var(--text-2); background: var(--surface-3);
  border: 1px solid var(--border);
  box-shadow: inset 0 -1px 0 var(--border);
}
a:focus-visible, .ib:focus-visible, .ai-btn:focus-visible, .tile:focus-visible, .btn:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
/* ── Motion: short, decelerating, consistent (fast 60ms · base 100–140ms) ── */
:root { --ease-out: cubic-bezier(0.22, 1, 0.36, 1); }
@keyframes rb-rise { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
@keyframes rb-drop { from { opacity: 0; transform: translateY(-6px) scale(0.98); } to { opacity: 1; transform: translateY(0) scale(1); } }
@keyframes rb-fade { from { opacity: 0; } to { opacity: 1; } }
@keyframes rb-slide-l { from { opacity: 0; transform: translateX(16px); } to { opacity: 1; transform: translateX(0); } }
@keyframes rb-tab-in { from { opacity: 0; transform: translateY(8px) scale(0.94); } to { opacity: 1; transform: translateY(0) scale(1); } }
@keyframes rb-pulse { 0%, 100% { opacity: 0.42; } 50% { opacity: 1; } }
.rb-in { animation: rb-rise 120ms var(--ease-out) both; }
/* Popup windows: content drops in once when the popup opens (not on every refresh). */
body.rb-enter > * { animation: rb-drop 100ms var(--ease-out) both; }
@media (prefers-reduced-motion: reduce) {
  * { animation-duration: 0.01ms !important; transition-duration: 0.01ms !important; }
}
"#;

/// Entrance-animation hooks for an element that appeared `elapsed_ms` ago. The UI is rebuilt from
/// HTML on every state change, so a negative `animation-delay` resumes the animation where it was
/// instead of restarting it; past `window_ms` the element renders plain.
pub fn enter_attrs(elapsed_ms: Option<u64>, window_ms: u64) -> (&'static str, String) {
    match elapsed_ms {
        Some(ms) if ms < window_ms => (" rb-in", format!(r#" style="animation-delay:-{ms}ms""#)),
        _ => ("", String::new()),
    }
}

/// Neutral page placeholder painted while a page loads into an empty content area.
/// `animated`: Clean-mode loads fade it in after 120 ms (fast loads never flash it) and pulse it.
/// The static variant is painted once under a WebView that hasn't drawn yet: the child window
/// clips our surface there, so it can't animate, and it must be visible from the first frame.
pub fn render_page_skeleton_html(animated: bool) -> String {
    let mut h = String::with_capacity(4096);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(
        r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); }
.sk-page {
  max-width: 760px; margin: 0 auto; padding: 44px 32px;
  display: flex; flex-direction: column; gap: 14px;
  animation: rb-fade 150ms ease-out 120ms both;
}
.b { border-radius: 8px; background: var(--surface-3); animation: rb-pulse 1.2s ease-in-out infinite; }
.t { height: 30px; width: 62%; }
.m { height: 12px; width: 34%; margin-bottom: 12px; }
.img { height: 200px; width: 100%; border-radius: 14px; margin: 4px 0 10px; animation-delay: 80ms; }
.l { height: 13px; animation-delay: 160ms; }
.gap { height: 10px; }
</style>"#,
    );
    if !animated {
        h.push_str("<style>.sk-page, .b { animation: none !important; } .b { opacity: 0.75; }</style>");
    }
    h.push_str(
        r#"</head><body><div class="sk-page"><div class="b t"></div><div class="b m"></div><div class="b img"></div>"#,
    );
    for (i, w) in [100, 97, 92, 95, 71, 0, 98, 94, 86, 62].iter().enumerate() {
        if *w == 0 {
            h.push_str(r#"<div class="gap"></div>"#);
        } else {
            let _ = write!(
                h,
                r#"<div class="b l" style="width:{w}%;animation-delay:{}ms"></div>"#,
                120 + i * 40
            );
        }
    }
    h.push_str("</div></body></html>");
    h
}

const CHROME_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--frame); user-select: none; }
.chrome { display: flex; flex-direction: column; width: 100%; height: 100%; }

/* ── Title bar & Flush Modern Tab Strip ── */
.titlebar {
  display: flex; align-items: center; height: 40px;
  padding-left: 8px; background: var(--frame);
}
.tabs {
  display: flex; align-items: flex-end; gap: 2px; flex: 0 1 auto;
  min-width: 0; overflow: hidden; height: 40px;
}
.tab {
  position: relative; display: flex; align-items: center; gap: 7px;
  width: 204px; flex: 0 1 auto; min-width: 68px; height: 32px; margin-bottom: 4px;
  padding: 0 8px 0 10px; border-radius: 9px;
  border: 1px solid transparent;
  color: var(--text-2); font-size: 12px;
  transition: background 60ms ease, color 60ms ease;
}
.tab:hover { background: var(--frame-hover); color: var(--text); }
.tab:active { background: var(--press); }
.tab.active {
  height: 34px; margin-bottom: 0;
  border-radius: 10px 10px 0 0;
  background: var(--surface); border-color: var(--border); border-bottom-color: transparent;
  color: var(--text); font-weight: 600;
  box-shadow: inset 0 1px 0 var(--tab-specular);
}
.tab.sleeping { color: var(--text-3); }
.tab.sleeping .fav { opacity: 0.52; }
.tab.sleeping:hover { color: var(--text-2); }
.tab.sleeping:hover .fav { opacity: 0.88; }
.tab.detaching {
  opacity: 0.45;
  outline: 1.5px dashed var(--accent);
  outline-offset: -2px;
}
.tab-sep {
  width: 1px; height: 13px; margin: 0 1px 13px;
  background: var(--divider); flex-shrink: 0;
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
  width: 5px; height: 5px; border-radius: 3px; flex-shrink: 0;
}
.tab-dot.sleep { background: var(--text-3); opacity: 0.55; }
.tab-badge {
  font-size: 9.5px; font-weight: 700; padding: 1px 5px;
  border-radius: 999px; flex-shrink: 0; letter-spacing: 0.02em;
}
.tab-badge.js { background: var(--surface-3); color: var(--text-2); }
.tab-badge.reader { background: var(--green-bg); color: var(--green-text); }
.tab-badge.live { background: var(--danger-bg); color: var(--danger); }
.tab-close {
  display: flex; align-items: center; justify-content: center;
  width: 18px; height: 18px; border-radius: 999px;
  color: var(--text-2); flex-shrink: 0; opacity: 0;
}
.tab.active .tab-close { opacity: 0.55; }
.tab:hover .tab-close, .tab-close:hover { opacity: 1; }
.tab-close:hover { background: var(--press); color: var(--text); }
.newtab {
  display: flex; align-items: center; justify-content: center;
  width: 26px; height: 26px; margin-left: 4px;
  border-radius: 999px; color: var(--text-2); flex-shrink: 0;
}
.newtab:hover { background: var(--frame-hover); color: var(--text); }
.newtab:active { background: var(--press); }
.drag { flex: 1 1 0; min-width: 28px; height: 40px; }
.title-tools {
  display: flex; align-items: center; gap: 2px;
  height: 40px; padding-right: 6px; flex-shrink: 0;
}
.tchip {
  display: inline-flex; align-items: center; gap: 4px;
  height: 26px; padding: 0 8px; border-radius: 7px;
  font-size: 11px; font-weight: 500; color: var(--text-3);
  background: transparent;
}
.tchip:hover { background: var(--frame-hover); color: var(--text); }
.tchip:active { background: var(--press); }
.tchip.icon-only { width: 28px; padding: 0; justify-content: center; }
.winctl { display: flex; align-self: flex-start; height: 40px; flex-shrink: 0; }
.wbtn {
  display: flex; align-items: center; justify-content: center;
  width: 44px; height: 40px; color: var(--text-2);
}
.wbtn:hover { background: var(--frame-hover); color: var(--text); }
.wbtn:active { background: var(--press); }
.wbtn.close:hover { background: #e5484d; color: #ffffff; }

/* ── Main Toolbar ── */
.toolbar {
  position: relative; display: flex; align-items: center; gap: 6px;
  height: 46px; padding: 0 10px; background: var(--surface);
  border-bottom: 1px solid var(--divider);
}
.toolbar.no-bm { border-bottom-color: var(--border); }
.nav-group { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.ib {
  position: relative; display: flex; align-items: center; justify-content: center;
  width: 29px; height: 29px; border-radius: 999px;
  color: var(--text-2); flex-shrink: 0;
}
.ib:hover { background: var(--hover); color: var(--text); }
.ib:active { background: var(--press); }
.ib.off { color: var(--text-3); opacity: 0.35; }
.ib.off:hover, .ib.off:active { background: transparent; color: var(--text-3); }
.ib.on { color: var(--accent); background: var(--accent-bg); }
.ib.dl-done { color: var(--green); background: var(--green-bg); }
.ib.star-on { color: #f59e0b; }
.dl-dot {
  position: absolute; top: 4px; right: 4px; width: 6px; height: 6px;
  border-radius: 50%; border: 1.5px solid var(--surface);
}
.dl-dot.active { background: var(--accent); }
.dl-dot.done { background: var(--green); }

/* ── Pill Omnibox ── */
.omni {
  display: flex; align-items: center; gap: 6px;
  flex: 1; min-width: 160px; height: 34px; margin: 0 6px;
  padding: 0 6px 0 8px; border-radius: 999px;
  border: 1px solid transparent; background: var(--omni);
  transition: background 80ms ease, border-color 80ms ease, box-shadow 80ms ease;
}
.omni:hover { background: var(--omni-hover); }
.omni:focus-within {
  background: var(--surface); border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-ring);
}
.omni-lead {
  display: flex; align-items: center; justify-content: center;
  width: 22px; height: 22px; border-radius: 999px; flex-shrink: 0;
  color: var(--text-3); background: transparent;
}
.omni-lead.secure { color: var(--green); }
.omni-lead.warn { color: var(--amber); }
.omni-lead.internal { color: var(--text-2); }
#url-bar {
  flex: 1; min-width: 40px; height: 100%; border: none;
  background: transparent; color: var(--text); font-size: 13px; outline: none;
}
.omni-end { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.omni-btn {
  display: flex; align-items: center; justify-content: center;
  width: 24px; height: 24px; border-radius: 999px;
  color: var(--text-2); flex-shrink: 0;
}
.omni-btn:hover { background: var(--hover); color: var(--text); }
.omni-btn:active { background: var(--press); }
.omni-btn.on { color: var(--accent); background: var(--accent-bg); }
.omni-btn.star-on { color: #f59e0b; }
.zoom-pill {
  display: inline-flex; align-items: center; height: 22px; padding: 0 7px;
  border-radius: 999px; background: var(--surface-3); color: var(--text);
  font-size: 11px; font-weight: 600; white-space: nowrap;
}

/* ── Right Utility & Whisper-Quiet Status Pill ── */
.util-group { display: flex; align-items: center; gap: 4px; flex-shrink: 0; }
.status-cluster {
  display: inline-flex; align-items: center; height: 28px; padding: 0 3px; gap: 1px;
  border-radius: 999px; background: var(--surface-2); border: 1px solid var(--divider);
  flex-shrink: 0;
}
.seg {
  display: inline-flex; align-items: center; gap: 4px; height: 22px; padding: 0 7px;
  border-radius: 999px; font-size: 11px; font-weight: 500; color: var(--text-2);
  white-space: nowrap; flex-shrink: 0;
}
.seg:hover { background: var(--hover); color: var(--text); }
.seg:active { background: var(--press); }
.seg.clean { background: var(--green-bg); color: var(--green-text); font-weight: 600; }
.seg.js { background: var(--accent-bg); color: var(--accent-text); font-weight: 600; }
.seg.shield-on { color: var(--text-2); }
.seg.shield-on .svg-ico { color: var(--green); }
.seg-sep { width: 1px; height: 11px; background: var(--divider); flex-shrink: 0; }
.ai-btn {
  display: inline-flex; align-items: center; gap: 5px;
  height: 28px; padding: 0 11px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--surface);
  font-size: 11.5px; font-weight: 600; color: var(--text);
  white-space: nowrap; flex-shrink: 0;
}
.ai-btn:hover { background: var(--hover); border-color: var(--border-strong); }
.ai-btn:active { background: var(--press); }
.ai-btn.on {
  background: var(--ink); border-color: var(--ink); color: var(--ink-fg);
}
.ai-btn.on:hover { opacity: 0.88; color: var(--ink-fg); }
.pill {
  display: inline-flex; align-items: center; gap: 5px;
  height: 26px; padding: 0 9px; border-radius: 999px;
  background: var(--surface-2); border: 1px solid var(--border);
  font-size: 11px; font-weight: 600; color: var(--text-2);
  white-space: nowrap; flex-shrink: 0;
}
.pill:hover { background: var(--hover); color: var(--text); border-color: var(--border-strong); }
.pill.js { background: var(--accent-bg); color: var(--accent-text); border-color: transparent; }
.pill.toast {
  background: var(--surface-3); color: var(--text);
  border-color: transparent; font-weight: 500;
}
.vsep { width: 1px; height: 14px; margin: 0 2px; background: var(--divider); flex-shrink: 0; }

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
  animation: progress-slide 0.65s cubic-bezier(0.45, 0, 0.25, 1) infinite;
}
/* Load finished: the bar sweeps to full width, then fades — a clear "done" beat. */
@keyframes rb-progress-done {
  0% { width: 45%; opacity: 1; }
  55% { width: 100%; opacity: 1; }
  100% { width: 100%; opacity: 0; }
}
.progress-bar.done { background: var(--accent); animation: rb-progress-done 220ms var(--ease-out) both; }

/* ── Press feedback: everything clickable gives under the finger, then snaps back ── */
.tab, .newtab, .tchip, .ib, .omni-btn, .seg, .ai-btn, .pill, .chip, .tab-close, .wbtn {
  transition: background 60ms ease, color 60ms ease, border-color 60ms ease, opacity 60ms ease,
    transform 80ms var(--ease-out);
}
.newtab:active, .tchip:active, .ib:active, .omni-btn:active, .seg:active, .ai-btn:active,
.pill:active, .chip:active, .tab-close:active { transform: scale(0.9); }
.tab:active { transform: scale(0.97); }
.ib.off:active { transform: none; }
.tab.rb-in { animation: rb-tab-in 130ms var(--ease-out) both; }

/* ── Bookmark Bar ── */
.bmbar {
  display: flex; align-items: center; gap: 4px; height: 32px;
  padding: 0 10px; background: var(--surface);
  border-bottom: 1px solid var(--border); overflow: hidden;
}
.bm-items { display: flex; align-items: center; gap: 2px; flex: 1; min-width: 0; overflow: hidden; }
.chip {
  display: inline-flex; align-items: center; gap: 6px;
  height: 24px; padding: 0 9px; border-radius: 999px;
  color: var(--text-2); font-size: 12px; white-space: nowrap; flex-shrink: 0;
}
.chip:hover { background: var(--hover); color: var(--text); }
.chip:active { background: var(--press); }
.chip.drop { background: var(--accent-bg); color: var(--accent-text); font-weight: 600; }
.chip .fav { width: 14px; height: 14px; font-size: 9px; }
.chip .fico { color: var(--amber); opacity: 0.85; display: inline-flex; }
.chip.drop .fico { color: var(--accent-text); opacity: 1; }
.chip.muted { color: var(--text-2); font-size: 11.5px; }
.chip.muted .fico { color: var(--text-2); opacity: 1; }
.bm-right { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.bm-empty { font-size: 11.5px; color: var(--text-3); padding-left: 6px; }
@media (max-width: 780px) {
  .ib { width: 28px; height: 28px; }
  .omni { min-width: 96px; margin: 0 3px; }
  .seg { padding: 0 5px; font-size: 10.5px; }
  .hide-narrow { display: none; }
}
"#;

const CUSTOM_WINDOW_CONTROLS: bool = cfg!(target_os = "windows") || cfg!(target_os = "linux");
const TRAFFIC_LIGHT_INSET: u32 = if cfg!(target_os = "macos") { 78 } else { 8 };

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
    /// Tab ID currently dragged outside the tab strip (releasing opens it in a new window).
    pub detaching_tab_id: Option<u64>,
    /// Number of active in-progress downloads (lights up the toolbar download icon).
    pub downloads_in_progress: usize,
    /// Number of newly finished downloads not yet viewed in the Downloads popup.
    pub downloads_unseen: usize,
    /// Whether the AI assistant sidebar is currently open.
    pub sidebar_open: bool,
    /// Number of tabs currently running in JS WebView mode (separate OS processes).
    pub js_webview_count: usize,
    /// Newer release tag (e.g. `"v0.5.5"`) when the updater found one; shows the install pill.
    pub update_version: Option<&'a str>,
    pub motion: ChromeMotion,
}

/// Time-based animation state for the chrome (see `enter_attrs`).
#[derive(Debug, Clone, Default)]
pub struct ChromeMotion {
    /// `(tab id, ms since it appeared)` for recently opened tabs.
    pub fresh_tabs: Vec<(u64, u64)>,
    /// Ms since the active tab finished loading, while the progress bar's finish sweep plays.
    pub progress_done_ms: Option<u64>,
}

fn spinner_html(dark_mode: bool) -> String {
    let accent = if dark_mode { "#60a5fa" } else { "#2563eb" };
    format!(
        r#"<span class="fav-spin"><svg width="14" height="14" viewBox="0 0 16 16" fill="none"><circle cx="8" cy="8" r="5.5" fill="none" stroke="{accent}" stroke-opacity="0.22" stroke-width="2" /><path d="M 8 2.5 A 5.5 5.5 0 0 1 11.89 11.89" fill="none" stroke="{accent}" stroke-width="2" stroke-linecap="round" /></svg></span>"#
    )
}

pub(super) fn favicon_html(title: &str, url: &str) -> String {
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
        let is_detaching = state.detaching_tab_id == Some(tab.id);
        let fresh_ms = state
            .motion
            .fresh_tabs
            .iter()
            .find(|(id, _)| *id == tab.id)
            .map(|(_, ms)| *ms);
        let (enter_cls, enter_style) = enter_attrs(fresh_ms, 130);
        let cls = match (is_active, is_sleeping, is_detaching) {
            (true, _, true) => "tab active detaching",
            (true, _, false) => "tab active",
            (false, true, true) => "tab sleeping detaching",
            (false, true, false) => "tab sleeping",
            (false, false, true) => "tab detaching",
            (false, false, false) => "tab",
        };
        let fav = if tab.is_loading {
            spinner_html(state.dark_mode)
        } else {
            favicon_html(&tab.title, &tab.url)
        };
        let mode_badge = if tab.media_active {
            r#"<span class="tab-badge live" title="카메라·마이크 또는 소리 사용 중 (탭을 옮겨도 계속 실행)">LIVE</span>"#
        } else if tab.js_mode {
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
            r#"<div class="{cls}{enter_cls}"{enter_style} data-action="tab:{idx}"{bm_attr}>{fav}<span class="tab-title">{title}</span>{mode_badge}<span class="tab-close" data-action="close-tab:{idx}">{close_svg}</span></div>"#,
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
        r#"</div><div class="newtab" data-action="new-tab" title="새 탭 (Ctrl+T)">{plus_svg}</div><div class="drag" data-drag="1"></div>
<div class="title-tools">
  <div class="tchip icon-only" data-action="cycle-theme" title="테마 전환 (시스템 · 라이트 · 다크)">{theme_svg}</div>
  <div class="tchip" data-action="open-palette" title="명령 팔레트 ({palette_label})">{cmd_svg}<span>{palette_label}</span></div>
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
    let loopback_http = is_loopback_http(&active.url);
    let (lead_cls, lead_svg) = if idn_suspicious {
        ("omni-lead warn", svg(Icon::AlertTriangle, 13))
    } else if active.url.starts_with("https://") {
        ("omni-lead secure", svg(Icon::Lock, 13))
    } else if active.url.starts_with("rust://") {
        ("omni-lead internal", svg(Icon::Compass, 14))
    } else if loopback_http {
        ("omni-lead", svg(Icon::Monitor, 13))
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
    let reload_tip = if active.is_loading {
        "로딩 중지 (Esc)"
    } else {
        "새로고침 (Ctrl+R)"
    };
    let _ = write!(
        h,
        r#"<div class="{toolbar_cls}">
<div class="nav-group">
  <div class="ib{back_off}" data-action="back" title="뒤로 (Alt+←)">{back}</div>
  <div class="ib{fwd_off}" data-action="forward" title="앞으로 (Alt+→)">{fwd}</div>
  <div class="ib" data-action="reload" title="{reload_tip}">{reload}</div>
  <div class="ib" data-action="nav:rust://home" title="새 탭 페이지 (Alt+Home)">{home}</div>
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
        } else if loopback_http {
            " title=\"로컬 서버 연결 (HTTP)\""
        } else if active.url.starts_with("http://") {
            " title=\"암호화되지 않은 HTTP 연결\""
        } else {
            ""
        },
    );

    if (active.zoom - 1.0).abs() > 0.001 {
        let _ = write!(
            h,
            r#"<div class="zoom-pill num" data-action="zoom-reset" title="확대/축소 초기화 (Ctrl+0)">{:.0}%</div>"#,
            active.zoom * 100.0
        );
    }
    if !active.url.starts_with("rust://") {
        let r_cls = if active.reader_mode { "omni-btn on" } else { "omni-btn" };
        let _ = write!(
            h,
            r#"<div class="{r_cls}" data-action="toggle-reader" title="리더 모드 전환 (Alt+R)">{}</div>"#,
            svg(Icon::BookOpen, 14)
        );
    }
    let star_cls = if is_bookmarked { "omni-btn star-on" } else { "omni-btn" };
    let star_svg = if is_bookmarked {
        svg(Icon::StarFilled, 14)
    } else {
        svg(Icon::StarOutline, 14)
    };
    let star_tip = if is_bookmarked {
        "북마크 해제 (Ctrl+D)"
    } else {
        "북마크 추가 (Ctrl+D)"
    };
    let _ = write!(
        h,
        r#"<div class="{star_cls}" data-action="toggle-bookmark" title="{star_tip}">{star_svg}</div>
  </div>
</div>
<div class="util-group">"#
    );

    if let Some(note) = state.status_note {
        let _ = write!(
            h,
            r#"<div class="pill toast hide-narrow" data-action="dismiss-toast" title="닫기">{}<span>{}</span></div>"#,
            svg(Icon::Check, 12),
            html_escape(&truncate_chars(note, 26)),
        );
    }

    let (engine_cls, engine_svg, engine_label) = if active.js_mode {
        ("seg js", svg(Icon::Code, 12), "JS")
    } else {
        ("seg clean", svg(Icon::Zap, 12), "Clean")
    };
    let (shield_cls, shield_svg, shield_label) = if state.filter_enabled {
        (
            "seg shield-on num",
            svg(Icon::ShieldCheck, 12),
            state.blocked_count.to_string(),
        )
    } else {
        ("seg", svg(Icon::ShieldOff, 12), "OFF".to_string())
    };
    let (shield_style, shield_title) = if active.js_mode {
        (
            r#" style="opacity:0.55""#,
            "트래커·광고 차단 (Clean 모드 전용 · 현재 JS 모드)",
        )
    } else {
        ("", "트래커·광고 차단 전환")
    };
    let ram_label = if sleeping_count > 0 {
        format!(
            r#"{:.0} MB<span class="hide-narrow"> · {sleeping_count}절전</span>"#,
            state.working_set_mb
        )
    } else {
        format!("{:.0} MB", state.working_set_mb)
    };
    let ram_title = if state.js_webview_count > 0 {
        "메인 프로세스 메모리 (WebView 프로세스 제외) · 클릭하여 회수"
    } else {
        "메모리 즉시 회수"
    };

    let (dl_cls, dl_dot) = if state.downloads_in_progress > 0 {
        ("ib on", r#"<span class="dl-dot active"></span>"#)
    } else if state.downloads_unseen > 0 {
        ("ib dl-done", r#"<span class="dl-dot done"></span>"#)
    } else {
        ("ib", "")
    };
    let ai_cls = if state.sidebar_open { "ai-btn on" } else { "ai-btn" };
    let update_btn = if let Some(ver) = state.update_version {
        format!(
            r#"<div class="pill js" data-action="apply-update" title="새 버전 {ver} 설치 — 설치 파일 내려받아 실행">{dl_svg}<span>업데이트 {ver}</span></div><div class="vsep"></div>"#,
            dl_svg = svg(Icon::Download, 12),
        )
    } else {
        String::new()
    };

    let _ = write!(
        h,
        r#"{update_btn}<div class="status-cluster">
    <div class="{engine_cls}" data-action="toggle-js" title="렌더링 엔진 전환 (Clean / JS)">{engine_svg}<span>{engine_label}</span></div>
    <div class="seg-sep"></div>
    <div class="{shield_cls}"{shield_style} data-action="toggle-filter" title="{shield_title}">{shield_svg}<span>{shield_label}</span></div>
    <div class="seg-sep"></div>
    <div class="seg num" data-action="trim-memory" title="{ram_title}">{cpu_svg}<span>{ram_label}</span></div>
  </div>
  <div class="vsep"></div>
  <div class="{dl_cls}" data-action="open-downloads" title="다운로드 목록 (Ctrl+J)">{dl_svg}{dl_dot}</div>
  <div class="ib hide-narrow" data-action="open-find" title="페이지 내 찾기 (Ctrl+F)">{find_svg}</div>
  <div class="ib" data-action="nav:rust://history" title="방문 기록 (Ctrl+H)">{hist_svg}</div>
  <div class="ib" data-action="nav:rust://bookmarks" title="북마크 관리자">{bm_svg}</div>
  <div class="{ai_cls}" data-action="toggle-sidebar" title="AI 어시스턴트 (Ctrl+Shift+I)">{ai_svg}<span class="hide-narrow">AI</span></div>
</div>"#,
        update_btn = update_btn,
        cpu_svg = svg(Icon::Cpu, 12),
        dl_svg = svg(Icon::Download, 14),
        find_svg = svg(Icon::Search, 14),
        hist_svg = svg(Icon::History, 14),
        bm_svg = svg(Icon::Bookmark, 14),
        ai_svg = svg(Icon::Sparkles, 14),
    );

    if active.is_loading {
        h.push_str(r#"<div class="progress-track"><div class="progress-bar"></div></div>"#);
    } else if let Some(ms) = state.motion.progress_done_ms {
        let _ = write!(
            h,
            r#"<div class="progress-track"><div class="progress-bar done" style="animation-delay:-{ms}ms"></div></div>"#
        );
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
            r#"<div class="chip muted" data-action="toggle-bm-bar" title="북마크 바 숨기기 (Ctrl+Shift+B)">{}</div></div></div>"#,
            svg(Icon::PanelTop, 12),
        );
    }

    h.push_str("</div></body></html>");
    h
}

pub(super) fn push_url_chip(h: &mut String, item: &BookmarkItem, src_spec: &str) {
    let short_title = html_escape(&truncate_chars(&item.title, 18));
    let _ = write!(
        h,
        r#"<div class="chip" data-action="nav:{url}" data-bm-src="{src}" data-bm-title="{short_title}">{fav}<span>{short_title}</span></div>"#,
        url = html_escape(&item.url),
        src = html_escape(src_spec),
        fav = favicon_html(&item.title, &item.url),
    );
}

// ── Bottom-Left Link Hover Preview Pill (Blitz Content) ──

pub fn render_link_preview_html(url: &str) -> String {
    let secure = url.starts_with("https://");
    let (ico_cls, ico) = if secure {
        ("lp-ico secure", svg(Icon::Lock, 12))
    } else {
        ("lp-ico", svg(Icon::ExternalLink, 12))
    };
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><style>{THEME_CSS}
html, body {{ width: 100%; height: 100%; overflow: hidden; background: transparent; padding: 4px 6px; }}
.pill {{
  display: inline-flex; align-items: center; gap: 6px;
  height: 26px; max-width: 100%; padding: 0 11px;
  border-radius: 8px; background: var(--surface); border: 1px solid var(--border-strong);
  box-shadow: var(--shadow-sm); font-size: 11.5px; color: var(--text-2);
  white-space: nowrap; overflow: hidden;
}}
.lp-ico {{ display: inline-flex; flex-shrink: 0; color: var(--text-2); }}
.lp-ico.secure {{ color: var(--green); }}
.url {{ overflow: hidden; color: var(--text); }}
.hint {{ color: var(--text-3); font-size: 10.5px; margin-left: 4px; flex-shrink: 0; }}
</style></head><body><div class="pill"><span class="{ico_cls}">{ico}</span><span class="url">{u}</span><span class="hint">{hint}</span></div></body></html>"#,
        u = html_escape(&truncate_chars(url, 72)),
        hint = if cfg!(target_os = "macos") {
            "⌘+클릭 새 탭"
        } else {
            "Ctrl+클릭 새 탭"
        },
    )
}

pub(super) fn strip_url_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

pub(super) fn format_relative_time(ts: u64) -> String {
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

pub(super) fn monogram_badge(title: &str, url: &str) -> (char, &'static str) {
    // Cohesive muted jewel palette: cobalt, emerald, sky, indigo, teal, rust, slate, navy.
    const PALETTE: &[&str] = &[
        "#2563eb", "#0d9468", "#0284c7", "#4f46e5", "#0f766e", "#c2410c", "#475569", "#1d4ed8",
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
    use super::pages::*;
    use super::*;
    use crate::history::{History, SuggestKind, Suggestion};

    #[test]
    fn flags_idn_homographs_but_not_pure_hangul_hosts() {
        assert!(is_suspicious_host("https://xn--pple-43d.com/"));
        assert!(is_suspicious_host("https://\u{0430}pple.com/")); // Cyrillic а + latin
        assert!(!is_suspicious_host("https://www.rust-lang.org/"));
        assert!(!is_suspicious_host("https://한글.kr/"));
        assert!(!is_suspicious_host("rust://home"));
    }

    #[test]
    fn recognizes_loopback_http_hosts() {
        assert!(is_loopback_http("http://localhost:8080/SASStudio/"));
        assert!(is_loopback_http("http://LocalHost/"));
        assert!(is_loopback_http("http://127.0.0.1:3000/"));
        assert!(is_loopback_http("http://[::1]:8080/"));
        assert!(!is_loopback_http("http://localhost.evil.com/"));
        assert!(!is_loopback_http("http://example.com/"));
        assert!(!is_loopback_http("https://localhost:8080/"));
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
            detaching_tab_id: None,
            downloads_in_progress: 0,
            downloads_unseen: 0,
            sidebar_open: false,
            js_webview_count: 0,
            update_version: None,
            motion: Default::default(),
        });
        assert!(html.contains(r#"value="typed""#));
        assert!(html.contains("T&amp;T"));
        assert!(html.contains("42 MB"));
        assert!(html.contains("Clean"));
        assert!(html.contains("메모리 정리됨"));
        assert!(html.contains("<svg"));
        assert!(!html.contains("apply-update"));
    }

    #[test]
    fn update_pill_offers_one_click_install() {
        let tab = Tab::new(1, "https://a.test/".into());
        let tabs = vec![tab];
        let bm = BookmarkStore::default();
        let base = ChromeRenderState {
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
            show_bookmarks_bar: false,
            theme_pref: ThemePreference::System,
            status_note: None,
            url_override: None,
            drop_target_folder: None,
            detaching_tab_id: None,
            downloads_in_progress: 0,
            downloads_unseen: 0,
            sidebar_open: false,
            js_webview_count: 0,
            update_version: Some("v9.9.9"),
            motion: Default::default(),
        };
        let html = render_chrome_html(&base);
        assert!(html.contains("apply-update"), "{html}");
        assert!(html.contains("v9.9.9"), "{html}");
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
            detaching_tab_id: None,
            downloads_in_progress: 0,
            downloads_unseen: 0,
            sidebar_open: false,
            js_webview_count: 0,
            update_version: None,
            motion: Default::default(),
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

        // First-run welcome lists each detected browser with an import link.
        let welcome = render_welcome_page(&["Chrome (Default)".to_string(), "Edge (Profile 1)".to_string()], true);
        assert!(welcome.contains("rust://welcome?import=0"), "{welcome}");
        assert!(welcome.contains("rust://welcome?import=1"), "{welcome}");
        assert!(welcome.contains("rust://welcome?skip=1"), "{welcome}");
        assert!(welcome.contains("Edge (Profile 1)"), "{welcome}");

        // Labels are escaped, never raw HTML.
        let evil = render_welcome_page(&["<img src=x onerror=alert(1)>".to_string()], false);
        assert!(!evil.contains("<img src=x"), "{evil}");

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
            detaching_tab_id: None,
            downloads_in_progress: 0,
            downloads_unseen: 1,
            sidebar_open: false,
            js_webview_count: 0,
            update_version: None,
            motion: Default::default(),
        });
        assert!(chrome_html.contains("open-downloads"));
        let dl_html = render_downloads_popup_html(&[DownloadItem {
            id: 1,
            url: "https://example.com/report.pdf".into(),
            filename: "report.pdf".into(),
            path: Some(std::path::PathBuf::from("report.pdf")),
            status: DownloadStatus::Completed,
            size_bytes: Some(1_572_864),
            started_at: 0,
        }]);
        assert!(dl_html.contains("report.pdf"));
        assert!(dl_html.contains("1.5 MB"));
        assert!(dl_html.contains("dl-open:1"));
        assert!(dl_html.contains("dl-reveal:1"));
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

    #[test]
    fn sidebar_renders_and_resolves_in_blitz() {
        let messages = vec![
            crate::chat::ChatMessage {
                role: crate::chat::ChatRole::User,
                text: "이 페이지 요약해 줘".into(),
                page_title: Some("Rust".into()),
                provider_label: None,
                activity: Vec::new(),
                elapsed_ms: None,
            },
            crate::chat::ChatMessage {
                role: crate::chat::ChatRole::Assistant,
                text: "## 요약\n- **빠른** 브라우저\n\n```rust\nprintln!(\"hi\");\n```".into(),
                page_title: None,
                provider_label: Some("Claude · Claude Sonnet".into()),
                activity: Vec::new(),
                elapsed_ms: None,
            },
            crate::chat::ChatMessage {
                role: crate::chat::ChatRole::Error,
                text: "로그아웃되었습니다.".into(),
                page_title: None,
                provider_label: Some("Claude".into()),
                activity: Vec::new(),
                elapsed_ms: None,
            },
        ];
        let model_options = crate::chat::preset_models(crate::chat::ChatProvider::Claude, &[]);
        let (html, blocks) = render_sidebar_html(&SidebarRenderState {
            provider: crate::chat::ChatProvider::Claude,
            avail: crate::chat::CliAvailability {
                claude: true,
                ..Default::default()
            },
            picker_open: SidebarPicker::Model,
            active_model_id: "sonnet",
            active_model_label: "Claude Sonnet",
            control_model_id: "haiku",
            effort: "high",
            providers: &[
                crate::chat::ChatProvider::Agy,
                crate::chat::ChatProvider::Claude,
                crate::chat::ChatProvider::OpenAiApi,
                crate::chat::ChatProvider::AnthropicApi,
                crate::chat::ChatProvider::GeminiApi,
            ],
            api_key_hint: "••••••1234",
            api_conn: None,
            custom_presets: &[crate::chat::AgentPreset {
                title: "표 수집".into(),
                prompt: "표 추출".into(),
            }],
            model_options: &model_options,
            agy_models_loading: false,
            models_error: None,
            custom_models: &[],
            context: None,
            compacting: false,
            can_compact: false,
            messages: &messages,
            is_loading: false,
            can_retry: true,
            include_page: true,
            agent_control: true,
            live: None,
            activity_open: &Default::default(),
            sidebar_wide: false,
            show_settings: true,
            active_title: "Rust",
            active_url: "https://www.rust-lang.org/",
            api_endpoint: "",
            has_api_key: true,
            draft: "질문",
            font_size: 13,
            reveal: None,
            show_history: false,
            history: &[],
            session_id: None,
            agent_confirm: None,
            motion: Default::default(),
        });
        assert!(html.contains("id=\"chat-input\""));
        assert!(html.contains("id=\"chat-messages\""));
        assert!(html.contains("class=\"composer\""));
        assert!(html.contains("chat-picker:provider"));
        assert!(html.contains("chat-picker:model"));
        assert!(html.contains("chat-toggle-width"));
        assert!(html.contains("chat-toggle-control"));
        assert!(html.contains("chat-model:sonnet"));
        assert!(html.contains("chat-ctrl-model:haiku"));
        assert!(html.contains("chat-preset-add"));
        assert!(html.contains("chat-open-login"));
        // Effort chips render for Claude with the stored override marked active.
        assert!(html.contains("chat-effort:high"));
        assert!(html.contains("chat-effort:low"));
        assert!(html.contains(r#"<div class="set-chip active" data-action="chat-effort:high">높음</div>"#));
        // Legacy Gemini CLI entry is hidden; the `agy` agent shows as Gemini.
        // (The `gemini_api` direct-API entry stays, so match the closing quote.)
        assert!(!html.contains("chat-provider:gemini\""));
        assert!(html.contains("chat-provider:agy"));
        assert!(html.contains("chat-provider:gemini_api\""));
        // Saved API key shows only as a masked hint, never the secret itself.
        assert!(html.contains("chat-copy-code:0"));
        assert_eq!(blocks, vec!["println!(\"hi\");"]);

        let mut doc = blitz_html::HtmlDocument::from_html(
            &html,
            blitz_dom::DocumentConfig {
                viewport: Some(blitz_traits::shell::Viewport::new(
                    380,
                    720,
                    1.0,
                    blitz_traits::shell::ColorScheme::Dark,
                )),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        // Settings tab fills the body; the composer stays in the DOM (hidden) to keep the draft.
        assert!(doc.get_element_by_id("chat-input").is_some());
        let set = doc.get_element_by_id("chat-settings").expect("chat-settings");
        let sz = doc.get_node(set).unwrap().final_layout().size;
        assert!(sz.width > 300.0 && sz.height > 400.0, "chat-settings={sz:?}");

        let live_steps = [
            crate::chat::ActivityStep {
                name: "navigate".into(),
                detail: "이동 완료".into(),
                ok: true,
                ms: 1200,
                ..Default::default()
            },
            crate::chat::ActivityStep {
                name: "type".into(),
                detail: "요소를 찾지 못함".into(),
                ..Default::default()
            },
            crate::chat::ActivityStep {
                name: "click".into(),
                detail: "검색 버튼".into(),
                running: true,
                ..Default::default()
            },
        ];
        let (loading_html, _) = render_sidebar_html(&SidebarRenderState {
            provider: crate::chat::ChatProvider::Claude,
            avail: crate::chat::CliAvailability::default(),
            picker_open: SidebarPicker::None,
            active_model_id: "sonnet",
            active_model_label: "Claude Sonnet",
            control_model_id: "haiku",
            effort: "",
            providers: &[crate::chat::ChatProvider::Claude],
            api_key_hint: "",
            api_conn: None,
            custom_presets: &[],
            model_options: &model_options,
            agy_models_loading: false,
            models_error: None,
            custom_models: &[],
            context: None,
            compacting: false,
            can_compact: false,
            messages: &messages,
            is_loading: true,
            can_retry: false,
            include_page: true,
            agent_control: true,
            live: Some(("@click 실행 중 · 검색 버튼 (4s)…".to_string(), &live_steps)),
            activity_open: &Default::default(),
            sidebar_wide: false,
            show_settings: false,
            active_title: "Rust",
            active_url: "https://www.rust-lang.org/",
            api_endpoint: "",
            has_api_key: false,
            draft: "",
            font_size: 13,
            reveal: None,
            show_history: false,
            history: &[],
            session_id: None,
            agent_confirm: Some(("새 탭 열기 → https://evil.test/?d=x", "evil.test")),
            motion: Default::default(),
        });
        assert!(loading_html.contains(r#"id="act-live-txt""#) && loading_html.contains("@click 실행 중"));
        assert!(loading_html.contains("이동 완료") && loading_html.contains("완료 · 1.2초"));
        assert!(loading_html.contains("act-st err") && loading_html.contains("act-st run"));
        assert!(loading_html.contains("agent-confirm:allow") && loading_html.contains("evil.test"));

        // ChatRole::Error provider_label and bookmarks folder query must be HTML-escaped / canonicalized.
        let evil_err = [crate::chat::ChatMessage {
            role: crate::chat::ChatRole::Error,
            text: "err".into(),
            page_title: None,
            provider_label: Some(r#"<span data-action="chat-del-all">bad</span>"#.into()),
            activity: Vec::new(),
            elapsed_ms: None,
        }];
        let (err_html, _) = render_sidebar_html(&SidebarRenderState {
            provider: crate::chat::ChatProvider::Claude,
            avail: crate::chat::CliAvailability::default(),
            picker_open: SidebarPicker::None,
            active_model_id: "",
            active_model_label: "",
            control_model_id: "",
            effort: "",
            providers: &[crate::chat::ChatProvider::Claude],
            api_key_hint: "",
            api_conn: None,
            custom_presets: &[],
            model_options: &[],
            agy_models_loading: false,
            models_error: None,
            custom_models: &[],
            context: None,
            compacting: false,
            can_compact: false,
            messages: &evil_err,
            is_loading: false,
            can_retry: false,
            include_page: false,
            agent_control: false,
            live: None,
            activity_open: &Default::default(),
            sidebar_wide: false,
            show_settings: false,
            active_title: "t",
            active_url: "rust://home",
            api_endpoint: "",
            has_api_key: false,
            draft: "",
            font_size: 13,
            reveal: None,
            show_history: false,
            history: &[],
            session_id: None,
            agent_confirm: None,
            motion: Default::default(),
        });
        assert!(!err_html.contains(r#"<span data-action="chat-del-all">"#), "{err_html}");
        assert!(err_html.contains("&lt;span data-action="), "{err_html}");

        let bm = BookmarkStore {
            bar: vec![crate::bookmarks::BookmarkNode::Folder(
                crate::bookmarks::BookmarkFolder {
                    title: "Work".into(),
                    children: vec![crate::bookmarks::BookmarkNode::Folder(
                        crate::bookmarks::BookmarkFolder {
                            title: "Sub".into(),
                            children: vec![],
                        },
                    )],
                },
            )],
            ..Default::default()
        };
        let bm_html = render_bookmarks_page(
            &bm,
            r#"rust://bookmarks?folder=bar-0-"><a href="rust://history?clear=1"#,
            None,
        );
        assert!(!bm_html.contains(r#"rust://history?clear=1"#), "{bm_html}");
        assert!(
            bm_html.contains(r#"href="rust://bookmarks?folder=bar-0-0""#),
            "{bm_html}"
        );
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    use blitz_dom::DocumentConfig;
    use blitz_traits::shell::{ColorScheme, Viewport};

    fn layout(html: &str) -> blitz_html::HtmlDocument {
        let mut doc = blitz_html::HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        doc
    }

    #[test]
    fn entrance_resumes_across_rebuilds_then_stops() {
        assert_eq!(
            enter_attrs(Some(60), 200),
            (" rb-in", r#" style="animation-delay:-60ms""#.to_string())
        );
        assert_eq!(enter_attrs(Some(200), 200), ("", String::new()));
        assert_eq!(enter_attrs(None, 200), ("", String::new()));
    }

    /// The motion CSS must actually animate in Blitz (an invalid rule would silently do nothing).
    #[test]
    fn blitz_runs_entrance_and_skeleton_animations() {
        let sk = layout(&render_page_skeleton_html(true));
        assert!(sk.is_animating(), "page skeleton pulses");
        assert!(
            !layout(&render_page_skeleton_html(false)).is_animating(),
            "static under-WebView skeleton is a single, fully visible frame"
        );

        let msg = format!(
            "<!DOCTYPE html><html><head><style>{THEME_CSS}</style></head><body><div class=\"rb-in\">hi</div></body></html>"
        );
        assert!(layout(&msg).is_animating(), "rb-in entrance animates");
        let popup = format!(
            "<!DOCTYPE html><html><head><style>{THEME_CSS}</style></head><body class=\"rb-enter\"><div>menu</div></body></html>"
        );
        assert!(layout(&popup).is_animating(), "popup drop-in animates");
    }

    #[test]
    fn chrome_marks_fresh_tabs_and_finished_progress() {
        let tabs = vec![
            Tab::new(1, "https://a.test/".into()),
            Tab::new(2, "https://b.test/".into()),
        ];
        let bm = BookmarkStore::default();
        let html = render_chrome_html(&ChromeRenderState {
            tabs: &tabs,
            active_idx: 0,
            bookmarks: &bm,
            can_back: false,
            can_forward: false,
            working_set_mb: 1.0,
            blocked_count: 0,
            filter_enabled: true,
            maximized: false,
            dark_mode: false,
            show_bookmarks_bar: false,
            theme_pref: ThemePreference::System,
            status_note: None,
            url_override: None,
            drop_target_folder: None,
            detaching_tab_id: None,
            downloads_in_progress: 0,
            downloads_unseen: 0,
            sidebar_open: false,
            js_webview_count: 0,
            update_version: None,
            motion: ChromeMotion {
                fresh_tabs: vec![(2, 40)],
                progress_done_ms: Some(100),
            },
        });
        assert_eq!(html.matches(" rb-in\"").count(), 1, "only the new tab animates in");
        assert!(
            html.contains(r#"rb-in" style="animation-delay:-40ms" data-action="tab:1""#),
            "{html}"
        );
        assert!(html.contains(r#"progress-bar done" style="animation-delay:-100ms""#));
        assert!(layout(&html).is_animating());
    }

    #[test]
    fn api_key_masked_hint_renders_without_secret() {
        let model_options = crate::chat::preset_models(crate::chat::ChatProvider::OpenAiApi, &[]);
        let render = |has_api_key: bool, api_key_hint: &str, api_conn: Option<(bool, &str)>| {
            render_sidebar_html(&SidebarRenderState {
                provider: crate::chat::ChatProvider::OpenAiApi,
                avail: crate::chat::CliAvailability::default(),
                providers: &[crate::chat::ChatProvider::OpenAiApi],
                picker_open: SidebarPicker::None,
                active_model_id: "gpt-4o-mini",
                active_model_label: "GPT-4o mini",
                control_model_id: "",
                effort: "",
                api_key_hint,
                api_conn,
                custom_presets: &[],
                model_options: &model_options,
                agy_models_loading: false,
                models_error: None,
                custom_models: &[],
                context: None,
                compacting: false,
                can_compact: false,
                messages: &[],
                is_loading: false,
                can_retry: false,
                include_page: true,
                agent_control: false,
                live: None,
                activity_open: &Default::default(),
                sidebar_wide: false,
                show_settings: true,
                active_title: "t",
                active_url: "https://e.test",
                api_endpoint: "https://api.example.com/v1/chat/completions",
                has_api_key,
                draft: "",
                font_size: 13,
                reveal: None,
                show_history: false,
                history: &[],
                session_id: None,
                agent_confirm: None,
                motion: Default::default(),
            })
        };
        // Saved key: masked tail proves it is stored; the secret itself never renders.
        let (saved_html, _) = render(true, "******9Ab3", None);
        assert!(saved_html.contains("9Ab3"), "{saved_html}");
        assert!(saved_html.contains("placeholder="), "{saved_html}");
        assert!(saved_html.contains("chat-clear-key"), "{saved_html}");
        assert!(!saved_html.contains("sk-secret-value-9Ab3"), "{saved_html}");
        // No key: the input invites entry instead of showing a stored state.
        let (empty_html, _) = render(false, "", None);
        assert!(empty_html.contains("sk-..."), "{empty_html}");
        assert!(!empty_html.contains("chat-clear-key"), "{empty_html}");
        // Connection test result shows next to the test button.
        let (ok_html, _) = render(true, "******9Ab3", Some((true, "ok-12ms")));
        assert!(ok_html.contains("chat-test-connection"), "{ok_html}");
        assert!(ok_html.contains("ok-12ms"), "{ok_html}");
        let (fail_html, _) = render(true, "******9Ab3", Some((false, "fail-401")));
        assert!(fail_html.contains("chat-test-connection"), "{fail_html}");
        assert!(fail_html.contains("fail-401"), "{fail_html}");
    }

    #[test]
    fn page_pill_is_disabled_on_internal_pages() {
        let model_options = crate::chat::preset_models(crate::chat::ChatProvider::OpenAiApi, &[]);
        let render = |url: &str, include_page: bool| {
            render_sidebar_html(&SidebarRenderState {
                provider: crate::chat::ChatProvider::OpenAiApi,
                avail: crate::chat::CliAvailability::default(),
                providers: &[crate::chat::ChatProvider::OpenAiApi],
                picker_open: SidebarPicker::None,
                active_model_id: "gpt-4o-mini",
                active_model_label: "GPT-4o mini",
                control_model_id: "",
                effort: "",
                api_key_hint: "",
                api_conn: None,
                custom_presets: &[],
                model_options: &model_options,
                agy_models_loading: false,
                models_error: None,
                custom_models: &[],
                context: None,
                compacting: false,
                can_compact: false,
                messages: &[],
                is_loading: false,
                can_retry: false,
                include_page,
                agent_control: false,
                live: None,
                activity_open: &Default::default(),
                sidebar_wide: false,
                show_settings: false,
                active_title: "t",
                active_url: url,
                api_endpoint: "",
                has_api_key: false,
                draft: "",
                font_size: 13,
                reveal: None,
                show_history: false,
                history: &[],
                session_id: None,
                agent_confirm: None,
                motion: Default::default(),
            })
        };
        // Internal pages have no page text: no action, so the pill can never
        // look stuck or flip the saved preference on a stray click.
        let (internal_html, _) = render("rust://home", true);
        assert!(!internal_html.contains("chat-toggle-page"), "{internal_html}");
        // Normal pages toggle between on and off.
        let (on_html, _) = render("https://e.test", true);
        assert!(on_html.contains("cpill ctx on"), "{on_html}");
        assert!(on_html.contains("chat-toggle-page"), "{on_html}");
        let (off_html, _) = render("https://e.test", false);
        assert!(!off_html.contains("cpill ctx on"), "{off_html}");
        assert!(off_html.contains("chat-toggle-page"), "{off_html}");
    }
}
