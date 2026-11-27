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
const THEME_CSS: &str = r#"
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
/* ── Motion: short, decelerating, consistent (fast 90ms · base 140–200ms) ── */
:root { --ease-out: cubic-bezier(0.22, 1, 0.36, 1); }
@keyframes rb-rise { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
@keyframes rb-drop { from { opacity: 0; transform: translateY(-6px) scale(0.98); } to { opacity: 1; transform: translateY(0) scale(1); } }
@keyframes rb-fade { from { opacity: 0; } to { opacity: 1; } }
@keyframes rb-slide-l { from { opacity: 0; transform: translateX(16px); } to { opacity: 1; transform: translateX(0); } }
@keyframes rb-tab-in { from { opacity: 0; transform: translateY(8px) scale(0.94); } to { opacity: 1; transform: translateY(0) scale(1); } }
@keyframes rb-pulse { 0%, 100% { opacity: 0.42; } 50% { opacity: 1; } }
.rb-in { animation: rb-rise 180ms var(--ease-out) both; }
/* Popup windows: content drops in once when the popup opens (not on every refresh). */
body.rb-enter > * { animation: rb-drop 140ms var(--ease-out) both; }
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

/// Neutral page placeholder painted while a Clean-mode page loads into an empty content area
/// (new tab, waking a sleeping tab, leaving JS mode). Fades in after 140 ms so fast loads never
/// flash it.
pub fn render_page_skeleton_html() -> String {
    let mut h = String::with_capacity(4096);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(
        r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); }
.sk-page {
  max-width: 760px; margin: 0 auto; padding: 44px 32px;
  display: flex; flex-direction: column; gap: 14px;
  animation: rb-fade 220ms ease-out 140ms both;
}
.b { border-radius: 8px; background: var(--surface-3); animation: rb-pulse 1.2s ease-in-out infinite; }
.t { height: 30px; width: 62%; }
.m { height: 12px; width: 34%; margin-bottom: 12px; }
.img { height: 200px; width: 100%; border-radius: 14px; margin: 4px 0 10px; animation-delay: 80ms; }
.l { height: 13px; animation-delay: 160ms; }
.gap { height: 10px; }
</style></head><body><div class="sk-page"><div class="b t"></div><div class="b m"></div><div class="b img"></div>"#,
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
  transition: background 110ms ease, color 110ms ease;
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
  transition: background 120ms ease, border-color 120ms ease, box-shadow 120ms ease;
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
  animation: progress-slide 0.8s cubic-bezier(0.45, 0, 0.25, 1) infinite;
}
/* Load finished: the bar sweeps to full width, then fades — a clear "done" beat. */
@keyframes rb-progress-done {
  0% { width: 45%; opacity: 1; }
  55% { width: 100%; opacity: 1; }
  100% { width: 100%; opacity: 0; }
}
.progress-bar.done { background: var(--accent); animation: rb-progress-done 320ms var(--ease-out) both; }

/* ── Press feedback: everything clickable gives under the finger, then snaps back ── */
.tab, .newtab, .tchip, .ib, .omni-btn, .seg, .ai-btn, .pill, .chip, .tab-close, .wbtn {
  transition: background 90ms ease, color 90ms ease, border-color 90ms ease, opacity 90ms ease,
    transform 120ms var(--ease-out);
}
.newtab:active, .tchip:active, .ib:active, .omni-btn:active, .seg:active, .ai-btn:active,
.pill:active, .chip:active, .tab-close:active { transform: scale(0.9); }
.tab:active { transform: scale(0.97); }
.ib.off:active { transform: none; }
.tab.rb-in { animation: rb-tab-in 200ms var(--ease-out) both; }

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
        let is_detaching = state.detaching_tab_id == Some(tab.id);
        let fresh_ms = state
            .motion
            .fresh_tabs
            .iter()
            .find(|(id, _)| *id == tab.id)
            .map(|(_, ms)| *ms);
        let (enter_cls, enter_style) = enter_attrs(fresh_ms, 200);
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

    // PDF tabs: one-click switch between WebView2's viewer and the pdf.js annotator.
    let pdf_btn = if active.is_pdf() {
        let (cls, label) = if active.editing_pdf() {
            ("pill js", "필기 끄기")
        } else {
            ("pill", "필기")
        };
        format!(
            r#"<div class="{cls}" data-action="pdf-editor" title="PDF 필기 모드 전환 (Alt+E)">{}<span>{label}</span></div>"#,
            svg(Icon::Pencil, 12)
        )
    } else {
        String::new()
    };
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
    let ram_label = if sleeping_count > 0 {
        format!(
            r#"{:.0} MB<span class="hide-narrow"> · {sleeping_count}절전</span>"#,
            state.working_set_mb
        )
    } else {
        format!("{:.0} MB", state.working_set_mb)
    };

    let (dl_cls, dl_dot) = if state.downloads_in_progress > 0 {
        ("ib on", r#"<span class="dl-dot active"></span>"#)
    } else if state.downloads_unseen > 0 {
        ("ib dl-done", r#"<span class="dl-dot done"></span>"#)
    } else {
        ("ib", "")
    };
    let ai_cls = if state.sidebar_open { "ai-btn on" } else { "ai-btn" };

    let _ = write!(
        h,
        r#"{pdf_btn}<div class="status-cluster">
    <div class="{engine_cls}" data-action="toggle-js" title="렌더링 엔진 전환 (Clean / JS)">{engine_svg}<span>{engine_label}</span></div>
    <div class="seg-sep"></div>
    <div class="{shield_cls}" data-action="toggle-filter" title="트래커·광고 차단 전환">{shield_svg}<span>{shield_label}</span></div>
    <div class="seg-sep"></div>
    <div class="seg num" data-action="trim-memory" title="메모리 즉시 회수">{cpu_svg}<span>{ram_label}</span></div>
  </div>
  <div class="vsep"></div>
  <div class="{dl_cls}" data-action="open-downloads" title="다운로드 목록 (Ctrl+J)">{dl_svg}{dl_dot}</div>
  <div class="ib hide-narrow" data-action="open-find" title="페이지 내 찾기 (Ctrl+F)">{find_svg}</div>
  <div class="ib" data-action="nav:rust://history" title="방문 기록 (Ctrl+H)">{hist_svg}</div>
  <div class="ib" data-action="nav:rust://bookmarks" title="북마크 관리자">{bm_svg}</div>
  <div class="{ai_cls}" data-action="toggle-sidebar" title="AI 어시스턴트 (Ctrl+Shift+I)">{ai_svg}<span class="hide-narrow">AI</span></div>
</div>"#,
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
.shell { display: flex; flex-direction: column; width: 100%; height: 100%; border: 1px solid var(--border-strong); background: var(--surface); }
.hdr { display: flex; align-items: center; gap: 8px; height: 36px; padding: 0 10px; border-bottom: 1px solid var(--divider); background: var(--surface-2); flex-shrink: 0; }
.back { display: flex; align-items: center; justify-content: center; width: 24px; height: 24px; border-radius: 999px; color: var(--text-2); }
.back:hover { background: var(--hover); color: var(--text); }
.hdr-ico { color: var(--text-2); display: inline-flex; }
.hdr-title { flex: 1; font-size: 12px; font-weight: 600; color: var(--text); white-space: nowrap; overflow: hidden; }
.hdr-cnt { font-size: 11px; color: var(--text-2); padding: 1px 7px; border-radius: 999px; background: var(--surface-3); }
.list { flex: 1; min-height: 0; overflow-y: auto; padding: 6px; }
.row { display: flex; align-items: center; gap: 10px; height: 32px; padding: 0 10px; border-radius: 8px; font-size: 12.5px; color: var(--text); }
.row:hover { background: var(--hover); }
.row:active { background: var(--press); }
.row.sel { background: var(--surface-3); color: var(--text); font-weight: 600; }
.fico { color: var(--amber); opacity: 0.85; display: inline-flex; flex-shrink: 0; }
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
.shell { display: flex; flex-direction: column; height: 100vh; border: 1px solid var(--border-strong); background: var(--surface); }
.list { flex: 1; padding: 6px; overflow: hidden; }
.row {
  position: relative; display: flex; align-items: center; gap: 10px;
  height: 38px; padding: 0 12px; border-radius: 9px;
  font-size: 13px; white-space: nowrap; overflow: hidden;
}
.row:hover { background: var(--hover); }
.row.sel { background: var(--surface-3); color: var(--text); }
.sico { width: 16px; display: inline-flex; align-items: center; justify-content: center; color: var(--text-2); flex-shrink: 0; }
.row.sel .sico { color: var(--accent); }
.main { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; overflow: hidden; }
.title { flex: 0 1 auto; min-width: 0; overflow: hidden; font-weight: 500; }
.url { flex: 1 1 0; min-width: 0; overflow: hidden; color: var(--text-3); font-size: 12px; }
.row.sel .url { color: var(--text-2); }
.hl { font-weight: 700; color: var(--accent); }
.row.sel .hl { color: var(--accent-text); text-decoration: underline; }
.badge {
  font-size: 10.5px; font-weight: 600; padding: 2px 7px;
  border-radius: 999px; background: var(--surface-3); color: var(--text-2); flex-shrink: 0;
}
.badge.tab { background: var(--green-bg); color: var(--green-text); }
.badge.bm { background: var(--amber-bg); color: var(--amber-text); }
.row.sel .badge { background: var(--surface); color: var(--text); }
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
.shell { display: flex; flex-direction: column; width: 100%; height: 100%; border: 1px solid var(--border-strong); background: var(--surface); }
.search-row {
  display: flex; align-items: center; gap: 10px; height: 50px;
  padding: 0 14px; border-bottom: 1px solid var(--divider); background: var(--surface); flex-shrink: 0;
}
.search-ico { color: var(--text-2); display: inline-flex; flex-shrink: 0; }
#palette-input {
  flex: 1; height: 34px; border: none; background: transparent;
  color: var(--text); font-size: 14px; outline: none;
}
.list { flex: 1; min-height: 0; overflow-y: auto; padding: 6px; }
.cat { padding: 7px 10px 4px; font-size: 11px; font-weight: 600; color: var(--text-3); letter-spacing: 0.03em; }
.row {
  display: flex; align-items: center; gap: 10px; height: 38px;
  padding: 0 10px; border-radius: 9px; font-size: 13px; color: var(--text);
}
.row:hover { background: var(--hover); }
.row:active { background: var(--press); }
.row.sel { background: var(--surface-3); color: var(--text); }
.pico {
  width: 24px; height: 24px; border-radius: 7px;
  display: inline-flex; align-items: center; justify-content: center;
  background: var(--surface-2); color: var(--text-2); flex-shrink: 0;
}
.row.sel .pico { background: var(--ink); color: var(--ink-fg); }
.pmain { display: flex; align-items: baseline; gap: 8px; flex: 1; min-width: 0; overflow: hidden; }
.ptitle { font-weight: 500; white-space: nowrap; overflow: hidden; flex-shrink: 0; max-width: 58%; }
.psub { font-size: 11.5px; color: var(--text-3); white-space: nowrap; overflow: hidden; flex: 1; }
.row.sel .psub { color: var(--text-2); }
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
  padding: 0 8px 0 12px; border: 1px solid var(--border-strong); background: var(--surface);
}
.find-shell:focus-within { border-color: var(--accent); }
.find-ico { color: var(--accent); display: inline-flex; flex-shrink: 0; }
#find-input {
  flex: 1; min-width: 80px; height: 30px; padding: 0 6px; border-radius: 6px;
  border: none; background: transparent;
  color: var(--text); font-size: 13px; outline: none;
}
#find-count {
  font-size: 11.5px; font-weight: 600; color: var(--text-2); padding: 2px 7px; border-radius: 6px;
  background: var(--surface-3); white-space: nowrap; flex-shrink: 0;
}
#find-count.none { color: var(--danger); background: var(--danger-bg); }
.fbtn {
  display: flex; align-items: center; justify-content: center;
  width: 28px; height: 28px; border-radius: 999px; color: var(--text-2); flex-shrink: 0;
}
.fbtn:hover { background: var(--hover); color: var(--text); }
.fbtn:active { background: var(--press); }
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

// ── Downloads Popup (`Ctrl+J` / Toolbar Download Button) ──

pub fn downloads_popup_height(item_count: usize) -> u32 {
    if item_count == 0 {
        return 174;
    }
    let rows = (item_count.min(6) as u32) * 58;
    (44 + rows + 14 + 38).clamp(174, DOWNLOADS_POPUP_MAX_HEIGHT)
}

fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", (b / KB).max(1.0))
    } else {
        format!("{bytes} B")
    }
}

const DOWNLOADS_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.dl-shell {
  display: flex; flex-direction: column; width: 100%; height: 100%;
  border: 1px solid var(--border-strong); background: var(--surface);
}
.dl-hdr {
  display: flex; align-items: center; justify-content: space-between;
  height: 44px; padding: 0 10px 0 14px; border-bottom: 1px solid var(--divider);
  background: var(--surface-2); flex-shrink: 0;
}
.dl-title { display: flex; align-items: center; gap: 7px; font-size: 13px; font-weight: 700; color: var(--text); }
.dl-title-ico { color: var(--accent); display: inline-flex; }
.dl-hdr-acts { display: flex; align-items: center; gap: 4px; }
.dl-hbtn {
  display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 9px;
  border-radius: 999px; font-size: 11.5px; font-weight: 500; color: var(--text-2);
}
.dl-hbtn:hover { background: var(--hover); color: var(--text); }
.dl-list { flex: 1; min-height: 0; overflow-y: auto; padding: 7px; }
.dl-row {
  display: flex; align-items: center; gap: 10px; min-height: 54px;
  padding: 8px 10px; border-radius: 9px; border: 1px solid transparent;
}
.dl-row:hover { background: var(--hover); border-color: var(--divider); }
.dl-row:active { background: var(--press); }
.dl-ico {
  width: 32px; height: 32px; border-radius: 8px;
  display: flex; align-items: center; justify-content: center;
  background: var(--surface-3); color: var(--text-2); flex-shrink: 0;
}
.dl-ico.prog { background: var(--accent-bg); color: var(--accent); }
.dl-ico.done { background: var(--green-bg); color: var(--green); }
.dl-ico.fail { background: var(--danger-bg); color: var(--danger); }
.dl-ico.block { background: var(--amber-bg); color: var(--amber); }
.dl-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.dl-name { font-size: 12.5px; font-weight: 600; color: var(--text); white-space: nowrap; overflow: hidden; }
.dl-meta { font-size: 11px; color: var(--text-2); white-space: nowrap; overflow: hidden; }
.dl-meta.done { color: var(--green-text); }
.dl-meta.fail { color: var(--danger); }
.dl-meta.block { color: var(--amber-text); }
.dl-acts { display: flex; align-items: center; gap: 4px; flex-shrink: 0; }
.dl-abtn {
  display: inline-flex; align-items: center; justify-content: center; gap: 4px;
  height: 26px; padding: 0 10px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--surface);
  font-size: 11px; font-weight: 600; color: var(--text);
}
.dl-abtn:hover { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); }
.dl-abtn:active { background: var(--press); }
.dl-abtn.icon { width: 26px; padding: 0; color: var(--text-2); }
.dl-empty {
  display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 7px;
  padding: 22px 16px; text-align: center; color: var(--text-3); font-size: 12.5px;
}
.dl-empty-ico {
  width: 38px; height: 38px; border-radius: 19px;
  display: flex; align-items: center; justify-content: center;
  background: var(--surface-3); color: var(--text-2);
}
.dl-empty-sub { font-size: 11.5px; opacity: 0.85; }
.dl-ftr {
  display: flex; align-items: center; justify-content: space-between;
  height: 38px; padding: 0 12px; border-top: 1px solid var(--divider);
  background: var(--surface-2); font-size: 11.5px; color: var(--text-2); flex-shrink: 0;
}
.dl-flink { display: inline-flex; align-items: center; gap: 5px; padding: 4px 8px; border-radius: 7px; font-weight: 500; }
.dl-flink:hover { background: var(--hover); color: var(--text); }
"#;

pub fn render_downloads_popup_html(items: &[DownloadItem]) -> String {
    let mut h = String::with_capacity(4096 + items.len() * 420);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(DOWNLOADS_CSS);
    let count_badge = if items.is_empty() {
        String::new()
    } else {
        format!(r#" <span class="num">({})</span>"#, items.len())
    };
    let clear_btn = if items.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div class="dl-hbtn" data-action="dl-clear">{}<span>지우기</span></div>"#,
            svg(Icon::Trash, 12)
        )
    };
    let _ = write!(
        h,
        r#"</style></head><body><div class="dl-shell">
<div class="dl-hdr">
  <div class="dl-title"><span class="dl-title-ico">{dl_ico}</span><span>다운로드{count_badge}</span></div>
  <div class="dl-hdr-acts">{clear_btn}<div class="dl-hbtn" data-action="dl-close">{close_ico}</div></div>
</div>
<div class="dl-list">"#,
        dl_ico = svg(Icon::Download, 14),
        close_ico = svg(Icon::Close, 12),
    );
    if items.is_empty() {
        let _ = write!(
            h,
            r#"<div class="dl-empty"><div class="dl-empty-ico">{}</div><div>최근 다운로드한 파일이 없습니다.</div><div class="dl-empty-sub">파일을 다운로드하면 상태와 저장 위치가 여기에 표시됩니다.</div></div>"#,
            svg(Icon::Download, 18),
        );
    } else {
        let folder_ico = svg(Icon::Folder, 12);
        for item in items {
            let host = short_host(&item.url);
            let ago = format_relative_time(item.started_at);
            let (ico_cls, ico_svg, meta_cls, meta_text) = match item.status {
                DownloadStatus::InProgress => (
                    "dl-ico prog",
                    svg(Icon::Download, 15),
                    "dl-meta",
                    if host.is_empty() {
                        "다운로드 중…".to_string()
                    } else {
                        format!("다운로드 중… · {host}")
                    },
                ),
                DownloadStatus::Completed => {
                    let sz = item
                        .size_bytes
                        .map(|b| format!(" · {}", format_file_size(b)))
                        .unwrap_or_default();
                    (
                        "dl-ico done",
                        svg(Icon::Check, 15),
                        "dl-meta done num",
                        format!("완료{sz} · {ago}"),
                    )
                }
                DownloadStatus::Failed => (
                    "dl-ico fail",
                    svg(Icon::AlertTriangle, 15),
                    "dl-meta fail",
                    if host.is_empty() {
                        format!("저장 실패 · {ago}")
                    } else {
                        format!("저장 실패 · {host}")
                    },
                ),
                DownloadStatus::Blocked => (
                    "dl-ico block",
                    svg(Icon::ShieldCheck, 15),
                    "dl-meta block",
                    if host.is_empty() {
                        "보안상 차단됨 (실행 파일)".to_string()
                    } else {
                        format!("보안상 차단됨 (실행 파일) · {host}")
                    },
                ),
            };
            let acts = if item.status == DownloadStatus::Completed && item.path.is_some() {
                format!(
                    r#"<div class="dl-acts"><div class="dl-abtn" data-action="dl-open:{}">열기</div><div class="dl-abtn icon" data-action="dl-reveal:{}" title="폴더에서 보기">{folder_ico}</div></div>"#,
                    item.id, item.id
                )
            } else {
                String::new()
            };
            let _ = write!(
                h,
                r#"<div class="dl-row"><div class="{ico_cls}">{ico_svg}</div><div class="dl-body"><div class="dl-name">{name}</div><div class="{meta_cls}">{meta}</div></div>{acts}</div>"#,
                name = html_escape(&truncate_chars(&item.filename, 34)),
                meta = html_escape(&meta_text),
            );
        }
    }
    let _ = write!(
        h,
        r#"</div>
<div class="dl-ftr">
  <div class="dl-flink" data-action="dl-folder">{folder}<span>다운로드 폴더 열기</span></div>
  <span><kbd>Ctrl+J</kbd></span>
</div>
</div></body></html>"#,
        folder = svg(Icon::Folder, 13),
    );
    h
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

// ── Docked AI Assistant Sidebar ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarPicker {
    #[default]
    None,
    Provider,
    Model,
}

pub struct SidebarRenderState<'a> {
    pub provider: crate::chat::ChatProvider,
    pub avail: crate::chat::CliAvailability,
    pub messages: &'a [crate::chat::ChatMessage],
    pub is_loading: bool,
    pub include_page: bool,
    pub agent_control: bool,
    pub agent_status: Option<&'a str>,
    pub agent_log: &'a [String],
    pub sidebar_wide: bool,
    pub show_settings: bool,
    pub picker_open: SidebarPicker,
    pub active_title: &'a str,
    pub active_url: &'a str,
    pub active_model_id: &'a str,
    pub active_model_label: &'a str,
    pub control_model_id: &'a str,
    pub custom_presets: &'a [crate::chat::AgentPreset],
    pub model_options: &'a [crate::chat::ModelOption],
    pub agy_models_loading: bool,
    pub can_retry: bool,
    pub api_endpoint: &'a str,
    pub api_model: &'a str,
    pub has_api_key: bool,
    pub draft: &'a str,
    /// Message text size in px.
    pub font_size: u32,
    /// `(message index, chars shown)` while a reply is typing out.
    pub reveal: Option<(usize, usize)>,
    pub show_history: bool,
    pub history: &'a [crate::chat::ChatSession],
    pub session_id: Option<u64>,
    /// `(action description, host)` of an agent navigation awaiting the user's OK.
    pub agent_confirm: Option<(&'a str, &'a str)>,
    pub motion: SidebarMotion,
}

/// Time-based entrance animations for the sidebar (see `enter_attrs`), in ms since appearing.
#[derive(Debug, Clone, Copy, Default)]
pub struct SidebarMotion {
    /// `(message index, ms)` for the newest message.
    pub msg: Option<(usize, u64)>,
    /// Settings card / history list / picker sheet that was just opened.
    pub panel_ms: Option<u64>,
    /// The sidebar itself, just opened.
    pub shell_ms: Option<u64>,
}

const ENTER_WINDOW_MS: u64 = 260;

/// Pixel height of the chat textarea showing `lines` lines (line-height 20px + padding).
pub fn chat_input_height(lines: usize) -> u32 {
    8 + 20 * lines.clamp(1, 6) as u32
}

// Text selection: THEME_CSS sets `* { user-select: none }`, and Blitz starts a drag-selection only
// when the pressed element computes `auto` AND its parent isn't `none`. So message text (and its
// wrappers, for the parent check) opts back in; buttons inside replies stay `none`.
const SIDEBAR_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.msg-u-wrap, .msg-u, .msg-a, .msg-body, .msg-body *, .msg-a.err > div { user-select: auto; }
.msg-body .chat-code-hdr, .msg-body .chat-code-hdr *, .msg-a-hdr, .msg-a-hdr *, .msg-a-ftr, .msg-a-ftr *,
.msg-u-ctx, .msg-u-ctx *, .err-acts, .err-acts * { user-select: none; }
.sb-shell {
  display: flex; flex-direction: column; width: 100%; height: 100%;
  border-left: 1px solid var(--border); background: var(--surface);
}
.sb-hdr {
  display: flex; align-items: center; justify-content: space-between;
  height: 46px; padding: 0 8px 0 14px;
  border-bottom: 1px solid var(--divider);
  background: var(--surface); flex-shrink: 0;
}
.sb-title { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 700; color: var(--text); letter-spacing: -0.015em; min-width: 0; }
.sb-title-ico {
  width: 24px; height: 24px; border-radius: 8px;
  background: var(--ink); color: var(--ink-fg);
  display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
}
.sb-title-txt { white-space: nowrap; overflow: hidden; }
.sb-mode-pill {
  display: inline-flex; align-items: center; gap: 4px;
  height: 20px; padding: 0 7px; border-radius: 999px;
  background: var(--accent-bg); color: var(--accent-text);
  font-size: 10px; font-weight: 600; flex-shrink: 0;
}
.sb-acts { display: flex; align-items: center; gap: 1px; flex-shrink: 0; }
.sb-hbtn {
  display: inline-flex; align-items: center; justify-content: center;
  width: 30px; height: 30px; border-radius: 9px;
  color: var(--text-2);
}
.sb-hbtn:hover { background: var(--hover); color: var(--text); }
.sb-hbtn:active { background: var(--press); }
.sb-hbtn.on { background: var(--surface-3); color: var(--text); }

/* ── Settings card ── */
.sb-settings {
  flex: 0 1 auto; min-height: 0; max-height: calc(100% - 150px); overflow-y: auto;
  margin: 8px 10px 4px; padding: 4px 12px 12px; border-radius: 14px;
  border: 1px solid var(--border-strong); background: var(--surface);
  box-shadow: var(--shadow-md);
  display: flex; flex-direction: column;
}
.set-sec { padding: 10px 0; border-bottom: 1px solid var(--divider); display: flex; flex-direction: column; gap: 6px; }
.set-sec:last-child { border-bottom: none; padding-bottom: 2px; }
.set-lbl { font-size: 10.5px; font-weight: 700; color: var(--text-2); letter-spacing: 0.01em; }
.set-lbl-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.set-chips { display: flex; flex-wrap: wrap; gap: 5px; }
.set-chip {
  display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 10px;
  border-radius: 999px; border: 1px solid var(--border); background: var(--surface-2);
  font-size: 11px; font-weight: 500; color: var(--text-2); white-space: nowrap;
}
.set-chip:hover { background: var(--hover); color: var(--text); border-color: var(--border-strong); }
.set-chip.active { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); font-weight: 600; }
.set-chip.dim { opacity: 0.55; }
.set-inp {
  width: 100%; height: 32px; padding: 0 10px; border-radius: 9px;
  border: 1px solid var(--border); background: var(--surface-2);
  color: var(--text); font-size: 12px; outline: none;
}
.set-inp:focus { background: var(--surface); border-color: var(--accent); box-shadow: 0 0 0 2.5px var(--accent-ring); }
.set-inp.secret { color: transparent; caret-color: var(--text); }
.set-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.set-note { font-size: 10.5px; color: var(--text-3); line-height: 1.4; flex: 1; }
.set-btns { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.font-row { display: flex; align-items: center; gap: 6px; }
.font-step {
  display: inline-flex; align-items: center; justify-content: center;
  min-width: 30px; height: 28px; padding: 0 8px; border-radius: 8px;
  border: 1px solid var(--border); background: var(--surface-2);
  font-size: 13px; font-weight: 600; color: var(--text);
}
.font-step:hover { background: var(--hover); border-color: var(--border-strong); }
.font-val { min-width: 44px; text-align: center; font-size: 12px; font-weight: 600; color: var(--text); }
.font-prev { flex: 1; text-align: right; color: var(--text-2); white-space: nowrap; overflow: hidden; }
.sbtn {
  display: inline-flex; align-items: center; justify-content: center;
  height: 28px; padding: 0 12px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--surface);
  font-size: 11px; font-weight: 600; color: var(--text);
}
.sbtn:hover { background: var(--hover); }
.sbtn.primary { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); }
.sbtn.primary:hover { opacity: 0.88; }
.sbtn.danger { color: var(--danger); }

/* ── Conversation ── */
.sb-msgs {
  flex: 1; min-height: 0; overflow-y: auto; padding: 16px 16px 18px;
  background: var(--surface);
  display: flex; flex-direction: column; gap: 20px;
}
.sb-empty {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  text-align: center; padding: 18px 4px 8px; gap: 9px; margin: auto 0;
}
.sb-emblem {
  width: 44px; height: 44px; border-radius: 14px;
  display: flex; align-items: center; justify-content: center;
  background: var(--ink); color: var(--ink-fg);
  box-shadow: var(--shadow-sm); margin-bottom: 2px;
}
.sb-empty-t { font-size: 17px; font-weight: 700; color: var(--text); letter-spacing: -0.025em; }
.sb-empty-s { font-size: 12px; color: var(--text-2); line-height: 1.5; max-width: 268px; }
.sb-page-pill {
  display: inline-flex; align-items: center; gap: 6px; max-width: 100%;
  padding: 4px 10px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--surface-2);
  font-size: 11px; color: var(--text-2); white-space: nowrap; overflow: hidden;
}
.sb-page-dot { width: 6px; height: 6px; border-radius: 3px; background: var(--green); flex-shrink: 0; }
.sb-quick-grid {
  display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px;
  width: 100%; margin-top: 10px;
}
.qcard {
  display: flex; flex-direction: column; align-items: flex-start; gap: 3px;
  padding: 11px 12px; border-radius: 13px; text-align: left;
  border: 1px solid var(--border); background: var(--surface);
  box-shadow: var(--shadow-xs);
}
.qcard:hover { background: var(--surface-2); border-color: var(--border-strong); }
.qcard:active { background: var(--press); }
.qcard-top { display: flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; color: var(--text); }
.qcard-ico { color: var(--text-2); display: inline-flex; flex-shrink: 0; }
.qcard:hover .qcard-ico { color: var(--accent); }
.qcard-sub { font-size: 11px; color: var(--text-3); line-height: 1.35; }

.msg-u-wrap { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; }
.msg-u-ctx {
  display: inline-flex; align-items: center; gap: 5px;
  font-size: 10.5px; color: var(--text-3); padding-right: 4px; max-width: 86%; white-space: nowrap; overflow: hidden;
}
.msg-u {
  max-width: 86%; padding: 9px 13px;
  border-radius: 16px 16px 5px 16px;
  background: var(--accent-bg); color: var(--text);
  font-size: var(--chat-fs); line-height: 1.55; white-space: pre-wrap;
}
.msg-a { align-self: stretch; padding: 2px 0; color: var(--text); }
.msg-a-hdr { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
.msg-a-ico {
  width: 22px; height: 22px; border-radius: 7px;
  background: var(--ink); color: var(--ink-fg);
  display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
}
.msg-a-tag { font-size: 11.5px; font-weight: 600; color: var(--text-2); letter-spacing: -0.01em; }
.msg-body {
  padding-left: 30px;
  font-size: var(--chat-fs); line-height: 1.68; color: var(--text);
}
.msg-body p { margin: 0 0 10px; }
.msg-body p:last-child { margin-bottom: 0; }
.msg-body h3 { font-size: 1.08em; font-weight: 700; letter-spacing: -0.015em; margin: 16px 0 6px; }
.msg-body h4 { font-size: 1em; font-weight: 700; margin: 12px 0 4px; }
.msg-body ul, .msg-body ol { margin: 4px 0 10px; padding-left: 20px; }
.msg-body li { margin-bottom: 4px; }
.msg-body blockquote {
  margin: 10px 0; padding: 8px 12px; border-left: 3px solid var(--accent);
  background: var(--surface-2); border-radius: 0 9px 9px 0; color: var(--text-2);
}
.msg-body blockquote p { margin: 0 0 4px; }
.msg-body blockquote p:last-child { margin-bottom: 0; }
.msg-body hr { border: none; border-top: 1px solid var(--divider); margin: 14px 0; }
.msg-body a { color: var(--accent); text-decoration: underline; text-underline-offset: 2px; }
.msg-body code {
  padding: 1.5px 5px; border-radius: 5px; background: var(--surface-3);
  font-family: "Cascadia Code", Consolas, monospace; font-size: 0.92em;
}
.type-caret {
  display: inline-block; width: 7px; height: 1.05em; margin-left: 2px;
  border-radius: 2px; background: var(--accent); vertical-align: text-bottom;
}
.chat-tbl {
  margin: 10px 0; border-radius: 10px; border: 1px solid var(--border);
  overflow-x: auto; background: var(--surface);
}
.chat-tbl table { width: 100%; border-collapse: collapse; font-size: 0.92em; }
.chat-tbl th, .chat-tbl td {
  padding: 7px 10px; border-bottom: 1px solid var(--divider);
  border-right: 1px solid var(--divider); text-align: left;
}
.chat-tbl th:last-child, .chat-tbl td:last-child { border-right: none; }
.chat-tbl tr:last-child td { border-bottom: none; }
.chat-tbl th { font-weight: 600; color: var(--text-2); background: var(--surface-2); }
.chat-code {
  margin: 10px 0; border-radius: 11px; border: 1px solid var(--border);
  background: var(--surface-2); overflow: hidden;
}
.chat-code-hdr {
  display: flex; align-items: center; justify-content: space-between;
  padding: 4px 6px 4px 12px; border-bottom: 1px solid var(--divider);
  background: var(--surface-3); font-size: 11px; font-weight: 600; color: var(--text-2);
}
.chat-code pre {
  margin: 0; padding: 11px 13px; overflow-x: auto;
  font-family: "Cascadia Code", Consolas, monospace; font-size: 0.92em; line-height: 1.55;
}
.chat-code pre code { padding: 0; background: transparent; font-size: 1em; }
.msg-a-ftr { display: flex; align-items: center; gap: 2px; padding-left: 26px; margin-top: 6px; }
.chat-copy {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 3px 8px; border-radius: 999px; font-size: 11px; font-weight: 500; color: var(--text-3);
}
.chat-copy:hover { background: var(--hover); color: var(--text); }
.sel-hint { font-size: 10.5px; color: var(--text-3); margin-left: auto; padding-right: 4px; }

.msg-a.err {
  padding: 11px 13px; border-radius: 12px;
  background: var(--danger-bg); color: var(--danger);
  font-size: 12.5px; line-height: 1.55;
}
.msg-a.err .msg-a-hdr { margin-bottom: 4px; }
.msg-a.err .msg-a-tag { color: var(--danger); }
.err-acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 9px; }
.err-retry {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 4px 10px; border-radius: 999px;
  background: var(--surface); border: 1px solid var(--border);
  font-size: 11px; font-weight: 600; color: var(--text);
}
.err-retry:hover { border-color: var(--border-strong); background: var(--surface-2); }

.sb-loading {
  display: flex; align-items: center; justify-content: space-between;
  padding: 4px 0 4px 2px; font-size: 12.5px; font-weight: 500; color: var(--text-2);
}
.sb-loading-l { display: flex; align-items: center; gap: 8px; }
.sb-agent-log {
  display: flex; flex-direction: column; gap: 4px;
  padding: 8px 11px; border-radius: 11px;
  background: var(--surface-2); border: 1px solid var(--border);
  font-size: 11px; color: var(--text-2);
}
.sb-agent-step { white-space: nowrap; overflow: hidden; line-height: 1.4; }
.sb-agent-step.err { color: var(--danger); }
.agent-confirm {
  display: flex; flex-direction: column; gap: 6px;
  padding: 12px 13px; border-radius: 13px;
  background: var(--amber-bg); border: 1px solid var(--amber);
  font-size: 12px; color: var(--text); line-height: 1.5;
}
.ac-t { font-weight: 700; color: var(--amber-text); }
.ac-d { font-size: 11px; color: var(--text-2); font-family: "Cascadia Code", Consolas, monospace; white-space: nowrap; overflow: hidden; }
.ac-btns { display: flex; justify-content: flex-end; gap: 6px; margin-top: 2px; }
@keyframes tab-spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
.fav-spin {
  display: inline-flex; align-items: center; justify-content: center;
  width: 20px; height: 20px; flex-shrink: 0;
  animation: tab-spin 0.8s linear infinite;
}

/* ── Conversation history ── */
.hist-hdr { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.hist-t { font-size: 13px; font-weight: 700; color: var(--text); }
.hist-sub { font-size: 11px; color: var(--text-3); margin-top: 2px; }
.hist-list { display: flex; flex-direction: column; gap: 4px; }
.hist-row {
  display: flex; align-items: center; gap: 10px;
  padding: 9px 8px 9px 12px; border-radius: 11px;
  border: 1px solid transparent;
}
.hist-row:hover { background: var(--hover); }
.hist-row.active { background: var(--surface-2); border-color: var(--border); }
.hist-main { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
.hist-title { font-size: 12.5px; font-weight: 600; color: var(--text); white-space: nowrap; overflow: hidden; }
.hist-meta { font-size: 10.5px; color: var(--text-3); white-space: nowrap; }
.hist-del {
  display: inline-flex; align-items: center; justify-content: center;
  width: 26px; height: 26px; border-radius: 8px; color: var(--text-3); flex-shrink: 0;
}
.hist-del:hover { background: var(--danger-bg); color: var(--danger); }
.hist-empty { padding: 36px 8px; text-align: center; font-size: 12px; color: var(--text-3); }

/* ── Picker sheet (above the composer) ── */
.sb-sheet {
  display: flex; flex-direction: column;
  margin: 0 12px 6px; border-radius: 14px;
  border: 1px solid var(--border-strong); background: var(--surface);
  box-shadow: var(--shadow-md); flex-shrink: 0; overflow: hidden;
}
.sheet-hdr {
  display: flex; align-items: center; justify-content: space-between;
  padding: 9px 12px 6px; font-size: 11px; font-weight: 600; color: var(--text-2);
}
.sheet-act {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 2px 8px; border-radius: 999px; font-size: 10.5px; font-weight: 500; color: var(--accent);
}
.sheet-act:hover { background: var(--accent-bg); }
.sheet-list { max-height: 220px; overflow-y: auto; padding: 2px 8px 8px; display: flex; flex-direction: column; gap: 2px; }
.sheet-sec { padding: 6px 6px 2px; font-size: 10px; font-weight: 700; color: var(--text-3); letter-spacing: 0.03em; }
.sheet-row {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  min-height: 32px; padding: 5px 9px; border-radius: 9px;
  font-size: 12px; color: var(--text);
}
.sheet-row:hover { background: var(--hover); }
.sheet-row.active { background: var(--surface-3); color: var(--text); font-weight: 600; }
.sheet-row-l { display: flex; align-items: center; gap: 7px; min-width: 0; flex: 1; }
.sheet-row-txt { display: flex; flex-direction: column; min-width: 0; }
.sheet-name { white-space: nowrap; overflow: hidden; }
.sheet-sub { font-size: 10px; color: var(--text-3); font-family: "Cascadia Code", Consolas, monospace; white-space: nowrap; overflow: hidden; }
.sheet-chk { color: var(--accent); display: inline-flex; flex-shrink: 0; }
.sheet-tag { font-size: 10px; color: var(--text-3); flex-shrink: 0; }
.sheet-ftr {
  display: flex; align-items: center; justify-content: space-between;
  padding: 8px 12px; border-top: 1px solid var(--divider); background: var(--surface-2);
  font-size: 11px; font-weight: 500; color: var(--text-2);
}
.sheet-ftr:hover { background: var(--hover); color: var(--text); }

/* ── Composer ── */
.sb-comp { padding: 6px 12px 10px; background: var(--surface); flex-shrink: 0; }
.composer {
  display: flex; flex-direction: column; gap: 8px;
  padding: 10px 10px 9px 14px; border-radius: 18px;
  border: 1px solid var(--border-strong); background: var(--surface);
  box-shadow: var(--shadow-sm);
}
.composer:focus-within {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-ring), var(--shadow-sm);
}
#chat-input {
  width: 100%; padding: 4px 0; border: none; background: transparent; resize: none;
  color: var(--text); font-size: 13px; line-height: 20px; outline: none;
  font-family: inherit;
}
.comp-bar { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.comp-left { display: flex; align-items: center; gap: 4px; min-width: 0; flex: 1; overflow: hidden; }
.cpill {
  display: inline-flex; align-items: center; gap: 5px;
  height: 26px; padding: 0 9px; border-radius: 999px;
  background: var(--surface-2);
  font-size: 11px; font-weight: 600; color: var(--text-2);
  white-space: nowrap; min-width: 0;
}
.cpill:hover { background: var(--hover); color: var(--text); }
.cpill.on { background: var(--surface-3); color: var(--text); }
.cpill.mod { flex: 0 1 auto; }
.cpill.ctx.on { background: var(--green-bg); color: var(--green-text); }
.cpill.ctrl.on { background: var(--accent-bg); color: var(--accent-text); }
.cpill-txt { white-space: nowrap; overflow: hidden; }
.cpill-chev { color: var(--text-3); display: inline-flex; flex-shrink: 0; }
.cpill.on .cpill-chev { color: var(--text); }
.pdot { width: 6px; height: 6px; border-radius: 3px; flex-shrink: 0; }
.pdot.ok { background: var(--green); }
.pdot.off { background: var(--text-3); opacity: 0.45; }
.cpill-dot { width: 5px; height: 5px; border-radius: 3px; background: var(--text-3); opacity: 0.45; flex-shrink: 0; }
.cpill.ctx.on .cpill-dot { background: var(--green); opacity: 1; }
.cpill.ctrl.on .cpill-dot { background: var(--accent); opacity: 1; }
.send-btn {
  display: flex; align-items: center; justify-content: center;
  width: 30px; height: 30px; border-radius: 999px;
  background: var(--ink); color: var(--ink-fg); flex-shrink: 0;
}
.send-btn:hover { opacity: 0.85; }
.send-btn.stop { background: var(--danger); color: #ffffff; }
.comp-hint { padding: 5px 6px 0; font-size: 10px; color: var(--text-3); text-align: center; }

/* ── Motion ── */
.sb-shell.rb-in { animation: rb-slide-l 170ms var(--ease-out) both; }
.sb-settings.rb-in, .sb-sheet.rb-in { animation: rb-drop 150ms var(--ease-out) both; }
.sb-hbtn, .set-chip, .sbtn, .cpill, .send-btn, .qcard, .chat-copy, .font-step, .hist-row,
.hist-del, .err-retry, .sheet-row, .sheet-ftr {
  transition: background 90ms ease, color 90ms ease, border-color 90ms ease, opacity 90ms ease,
    transform 120ms var(--ease-out);
}
.sb-hbtn:active, .set-chip:active, .sbtn:active, .cpill:active, .chat-copy:active, .font-step:active,
.hist-del:active, .err-retry:active { transform: scale(0.92); }
.send-btn:active { transform: scale(0.86); }
.qcard:active, .hist-row:active, .sheet-row:active { transform: scale(0.98); }
.qcard:hover { transform: translateY(-1px); }

/* ── Answer skeleton (replaces the spinner while the model thinks) ── */
.sk-msg { display: flex; flex-direction: column; gap: 8px; padding-left: 30px; padding-top: 2px; }
.sk-line { height: 10px; border-radius: 6px; background: var(--surface-3); animation: rb-pulse 1.1s ease-in-out infinite; }
.sk-line.w1 { width: 92%; }
.sk-line.w2 { width: 76%; animation-delay: 140ms; }
.sk-line.w3 { width: 52%; animation-delay: 280ms; }
.msg-a-hdr .sk-stop { margin-left: auto; }
"#;

/// Inserts the typing caret inside the last text block so it trails the words, not a new line.
fn with_type_caret(mut body_html: String) -> String {
    const CARET: &str = r#"<span class="type-caret"></span>"#;
    for close in [
        "</p>",
        "</li>",
        "</h3>",
        "</h4>",
        "</code></pre></div>",
        "</td></tr></tbody></table></div>",
    ] {
        if body_html.ends_with(close) {
            let at = body_html.len() - close.len();
            body_html.insert_str(at, CARET);
            return body_html;
        }
    }
    body_html.push_str(CARET);
    body_html
}

fn render_settings_card(h: &mut String, state: &SidebarRenderState<'_>) {
    let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
    let _ = write!(
        h,
        r#"<div id="chat-settings" class="sb-settings{enter_cls}"{enter_style}>"#
    );
    h.push_str(r#"<div class="set-sec"><div class="set-lbl">AI 프로바이더</div><div class="set-chips">"#);
    for &p in crate::chat::ChatProvider::ALL {
        let is_act = p == state.provider;
        let cli_missing = p.is_cli() && !state.avail.is_available(p);
        let cls = match (is_act, cli_missing) {
            (true, true) => "set-chip active dim",
            (true, false) => "set-chip active",
            (false, true) => "set-chip dim",
            (false, false) => "set-chip",
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-provider:{pid}">{plbl}</div>"#,
            pid = p.id(),
            plbl = html_escape(p.short_label()),
        );
    }
    h.push_str("</div></div>");

    // Connection first: these inputs and the 저장 button used to sit below the card's fold.
    if state.provider.is_cli() {
        let _ = write!(
            h,
            r#"<div class="set-sec">
  <div class="set-lbl">사용자 지정 모델 ID (비워두면 기본 모델)</div><input id="chat-api-model" class="set-inp" type="text" value="{model}" placeholder="예: gemini-4-neon-high / sonnet / o4-mini" />
  <div class="set-row">
    <span class="set-note">{hint}</span>
    <div class="set-btns"><div class="sbtn" data-action="chat-open-login">터미널 로그인</div><div class="sbtn primary" data-action="chat-save-settings">적용</div></div>
  </div>
</div>"#,
            model = html_escape(state.api_model),
            hint = html_escape(state.provider.login_hint()),
        );
    } else {
        let key_lbl = if state.has_api_key {
            "API Key · 저장됨 (바꾸려면 새 키 입력, 입력값은 가려짐)"
        } else {
            "API Key (입력값은 가려짐 · 로컬 Ollama는 비워두기)"
        };
        let clear_key_btn = if state.has_api_key {
            r#"<div class="sbtn danger" data-action="chat-clear-key">키 삭제</div>"#
        } else {
            ""
        };
        let _ = write!(
            h,
            r#"<div class="set-sec">
  <div class="set-lbl">Endpoint URL (Ollama: http://localhost:11434/v1/chat/completions)</div><input id="chat-api-endpoint" class="set-inp" type="text" value="{ep}" />
  <div class="set-lbl">Model</div><input id="chat-api-model" class="set-inp" type="text" value="{model}" />
  <div class="set-lbl">{key_lbl}</div><input id="chat-api-key" class="set-inp secret" type="password" value="" />
  <div class="set-row">
    <span class="set-note">{hint}</span>
    <div class="set-btns">{clear_key_btn}<div class="sbtn primary" data-action="chat-save-settings">저장</div></div>
  </div>
</div>"#,
            ep = html_escape(state.api_endpoint),
            model = html_escape(state.api_model),
            hint = html_escape(state.provider.login_hint()),
        );
    }

    let _ = write!(
        h,
        r#"<div class="set-sec"><div class="set-lbl">글자 크기</div><div class="font-row"><div class="font-step" data-action="chat-font:dec" title="작게">−</div><span class="font-val">{fs}px</span><div class="font-step" data-action="chat-font:inc" title="크게">+</div><div class="font-step" data-action="chat-font:reset" title="기본 크기">기본</div><span class="font-prev" style="font-size:{fs}px">가나다 Abc</span></div></div>"#,
        fs = state.font_size,
    );

    let set_refresh = if state.provider == crate::chat::ChatProvider::Agy {
        if state.agy_models_loading {
            r#"<span class="sheet-tag">불러오는 중…</span>"#.to_string()
        } else {
            format!(
                r#"<div class="sheet-act" data-action="chat-refresh-models">{}<span>새로고침</span></div>"#,
                svg(Icon::Reload, 10)
            )
        }
    } else {
        String::new()
    };
    let _ = write!(
        h,
        r#"<div class="set-sec"><div class="set-lbl-row"><span class="set-lbl">모델 프리셋</span>{set_refresh}</div><div class="set-chips">"#,
    );
    for opt in state.model_options {
        let cls = if opt.id == state.active_model_id {
            "set-chip active"
        } else {
            "set-chip"
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-model:{id}">{lbl}</div>"#,
            id = html_escape(&opt.id),
            lbl = html_escape(&opt.label),
        );
    }
    h.push_str("</div></div>");

    h.push_str(r#"<div class="set-sec"><div class="set-lbl">브라우저 제어용 빠른 모델</div><div class="set-chips">"#);
    let ctrl_same_cls = if state.control_model_id.is_empty() {
        "set-chip active"
    } else {
        "set-chip"
    };
    let ctrl_auto_lbl = if state.provider == crate::chat::ChatProvider::Agy {
        "자동 (Flash Low · 빠름)"
    } else {
        "대화 모델과 동일"
    };
    let _ = write!(
        h,
        r#"<div class="{ctrl_same_cls}" data-action="chat-ctrl-model:">{ctrl_auto_lbl}</div>"#
    );
    for opt in state.model_options.iter().filter(|o| !o.id.is_empty()) {
        let cls = if opt.id == state.control_model_id {
            "set-chip active"
        } else {
            "set-chip"
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-ctrl-model:{id}">{lbl}</div>"#,
            id = html_escape(&opt.id),
            lbl = html_escape(&opt.label),
        );
    }
    h.push_str("</div></div>");

    h.push_str(r#"<div class="set-sec"><div class="set-lbl">제어 프리셋 (원클릭 매크로)</div>"#);
    if !state.custom_presets.is_empty() {
        h.push_str(r#"<div class="set-chips">"#);
        for (i, preset) in state.custom_presets.iter().enumerate() {
            let _ = write!(
                h,
                r#"<div class="set-chip"><span>{title}</span><span style="margin-left:4px;opacity:0.7" data-action="chat-preset-del:{i}" title="삭제">×</span></div>"#,
                title = html_escape(&preset.title),
            );
        }
        h.push_str("</div>");
    }
    h.push_str(
        r#"<div style="display:flex;gap:5px;align-items:center"><input id="chat-preset-title" class="set-inp" style="flex:0 0 110px" type="text" value="" placeholder="이름" /><input id="chat-preset-prompt" class="set-inp" style="flex:1" type="text" value="" placeholder="실행할 지시 내용" /><div class="sbtn" data-action="chat-preset-add">추가</div></div><div class="set-note">이름과 지시 내용을 입력하고 추가하면 빈 대화 화면에 실행 카드로 나타납니다.</div></div>"#,
    );
    h.push_str("</div>");
}

fn render_history_panel(h: &mut String, state: &SidebarRenderState<'_>) {
    let del_all = if state.history.is_empty() {
        String::new()
    } else {
        r#"<div class="sbtn danger" data-action="chat-del-all">전체 삭제</div>"#.to_string()
    };
    let _ = write!(
        h,
        r#"<div class="hist-hdr"><div><div class="hist-t">대화 기록</div><div class="hist-sub">이 PC에만 저장됩니다 · {n}개</div></div>{del_all}</div>"#,
        n = state.history.len(),
    );
    if state.history.is_empty() {
        h.push_str(r#"<div class="hist-empty">저장된 대화가 없습니다.<br/>대화를 시작하면 자동으로 저장됩니다.</div>"#);
        return;
    }
    let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
    let _ = write!(h, r#"<div class="hist-list{enter_cls}"{enter_style}>"#);
    let trash = svg(Icon::Trash, 13);
    for s in state.history {
        let cls = if Some(s.id) == state.session_id {
            "hist-row active"
        } else {
            "hist-row"
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-load:{id}"><div class="hist-main"><span class="hist-title">{title}</span><span class="hist-meta">{when} · 메시지 {n}개</span></div><span class="hist-del" data-action="chat-del:{id}" title="이 대화 삭제">{trash}</span></div>"#,
            id = s.id,
            title = html_escape(&s.title),
            when = format_relative_time(s.updated),
            n = s.messages.len(),
        );
    }
    h.push_str("</div>");
}

fn render_quick_actions(state: &SidebarRenderState<'_>, is_internal: bool) -> String {
    let cards: &[(&str, Icon, &str, &str)] = if state.agent_control {
        &[
            (
                "chat-quick:extract-tables",
                Icon::Sparkles,
                "표 데이터 추출",
                "테이블 전체를 표로 정리",
            ),
            (
                "chat-quick:compare-tabs",
                Icon::BookOpen,
                "열린 탭 비교",
                "탭 전환하며 핵심 비교",
            ),
            (
                "chat-quick:extract-links",
                Icon::ExternalLink,
                "주요 링크 수집",
                "페이지 링크 분류 정리",
            ),
            ("chat-quick:summary", Icon::Zap, "페이지 요약", "핵심 내용 한눈에"),
        ]
    } else if is_internal {
        &[]
    } else {
        &[
            ("chat-quick:summary", Icon::Sparkles, "페이지 요약", "핵심 내용 한눈에"),
            ("chat-quick:bullets", Icon::BookOpen, "핵심 포인트", "3~5줄 불릿 정리"),
            ("chat-quick:explain-ko", Icon::Zap, "쉽게 설명", "한국어 쉬운 해설"),
            (
                "chat-quick:translate",
                Icon::ExternalLink,
                "한국어 번역",
                "자연스러운 번역",
            ),
        ]
    };
    if cards.is_empty() && state.custom_presets.is_empty() {
        return String::new();
    }
    let mut qa = String::from(r#"<div class="sb-quick-grid">"#);
    for (action, icon, title, sub) in cards {
        let _ = write!(
            qa,
            r#"<div class="qcard" data-action="{action}"><div class="qcard-top"><span class="qcard-ico">{ico}</span><span>{title}</span></div><div class="qcard-sub">{sub}</div></div>"#,
            ico = svg(*icon, 13),
        );
    }
    for (i, preset) in state.custom_presets.iter().enumerate() {
        let _ = write!(
            qa,
            r#"<div class="qcard" data-action="chat-preset-run:{i}"><div class="qcard-top"><span class="qcard-ico">{ico}</span><span>{title}</span></div><div class="qcard-sub">{sub}</div></div>"#,
            ico = svg(Icon::Zap, 13),
            title = html_escape(&truncate_chars(&preset.title, 18)),
            sub = html_escape(&truncate_chars(&preset.prompt, 26)),
        );
    }
    qa.push_str("</div>");
    qa
}

pub fn render_sidebar_html(state: &SidebarRenderState<'_>) -> (String, Vec<String>) {
    let mut code_blocks = Vec::new();
    let mut h = String::with_capacity(16 * 1024);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(SIDEBAR_CSS);
    let _ = write!(h, ":root {{ --chat-fs: {}px; }}", state.font_size);
    let on = |b: bool| if b { "sb-hbtn on" } else { "sb-hbtn" };
    let mode_badge = if state.agent_control {
        r#"<span class="sb-mode-pill"><span class="pdot ok"></span><span>제어</span></span>"#
    } else {
        ""
    };
    let _ = write!(
        h,
        r#"</style></head><body><div class="sb-shell{shell_cls}"{shell_style}>
<div class="sb-hdr">
  <div class="sb-title"><span class="sb-title-ico">{spark}</span><span class="sb-title-txt">AI Assistant</span>{mode_badge}</div>
  <div class="sb-acts">
    <div class="sb-hbtn" data-action="chat-clear" title="새 대화">{plus}</div>
    <div class="{hist_cls}" data-action="chat-toggle-history" title="대화 기록">{hist}</div>
    <div class="{width_cls}" data-action="chat-toggle-width" title="사이드바 폭 전환 (경계 드래그로도 조절)">{width_ico}</div>
    <div class="{settings_cls}" data-action="chat-toggle-settings" title="모델 / API / 글자 크기 설정">{sliders}</div>
    <div class="sb-hbtn" data-action="toggle-sidebar" title="닫기 (Ctrl+Shift+I)">{close}</div>
  </div>
</div>"#,
        spark = svg(Icon::Sparkles, 13),
        plus = svg(Icon::Plus, 14),
        hist = svg(Icon::History, 14),
        width_ico = svg(Icon::MoveHorizontal, 13),
        sliders = svg(Icon::Sliders, 13),
        close = svg(Icon::Close, 12),
        hist_cls = on(state.show_history),
        width_cls = on(state.sidebar_wide),
        settings_cls = on(state.show_settings),
        shell_cls = enter_attrs(state.motion.shell_ms, ENTER_WINDOW_MS).0,
        shell_style = enter_attrs(state.motion.shell_ms, ENTER_WINDOW_MS).1,
    );

    if state.show_settings {
        render_settings_card(&mut h, state);
    }

    h.push_str(r#"<div id="chat-messages" class="sb-msgs">"#);
    let is_internal = state.active_url.starts_with("rust://");
    if state.show_history {
        render_history_panel(&mut h, state);
    } else if state.messages.is_empty() && !state.is_loading {
        let page_pill = if is_internal {
            String::new()
        } else {
            format!(
                r#"<div class="sb-page-pill"><span class="sb-page-dot"></span><span>{}</span></div>"#,
                html_escape(&truncate_chars(state.active_title, 34))
            )
        };
        let _ = write!(
            h,
            r#"<div class="sb-empty">
  <div class="sb-emblem">{emblem}</div>
  <div class="sb-empty-t">무엇을 도와드릴까요?</div>
  <div class="sb-empty-s">웹페이지를 읽고 요약·번역·코드 분석까지 바로 답해드립니다.</div>
  {page_pill}
  {quick}
</div>"#,
            emblem = svg(Icon::Sparkles, 20),
            quick = render_quick_actions(state, is_internal),
        );
    } else {
        let copy_svg = svg(Icon::Copy, 11);
        let regen_svg = svg(Icon::Reload, 11);
        let spark_mini = svg(Icon::Sparkles, 11);
        let msg_count = state.messages.len();
        let last_assistant = state
            .messages
            .iter()
            .rposition(|m| m.role == crate::chat::ChatRole::Assistant);
        for (idx, msg) in state.messages.iter().enumerate() {
            let enter_ms = state.motion.msg.filter(|(i, _)| *i == idx).map(|(_, ms)| ms);
            let (enter_cls, enter_style) = enter_attrs(enter_ms, ENTER_WINDOW_MS);
            match msg.role {
                crate::chat::ChatRole::User => {
                    let ctx_badge = msg
                        .page_title
                        .as_deref()
                        .map(|t| {
                            format!(
                                r#"<div class="msg-u-ctx"><span class="sb-page-dot"></span>페이지 포함 · {}</div>"#,
                                html_escape(&truncate_chars(t, 28))
                            )
                        })
                        .unwrap_or_default();
                    let _ = write!(
                        h,
                        r#"<div class="msg-u-wrap{enter_cls}"{enter_style}>{ctx_badge}<div class="msg-u">{}</div></div>"#,
                        html_escape(&msg.text),
                    );
                }
                crate::chat::ChatRole::Assistant => {
                    let typing = state.reveal.filter(|(i, _)| *i == idx).map(|(_, n)| n);
                    let body_html = match typing {
                        Some(n) => {
                            let partial: String = msg.text.chars().take(n).collect();
                            with_type_caret(crate::chat::render_markdown_to_html(&partial, &mut code_blocks))
                        }
                        None => crate::chat::render_markdown_to_html(&msg.text, &mut code_blocks),
                    };
                    let footer = if typing.is_some() {
                        String::new()
                    } else {
                        let regen = if Some(idx) == last_assistant && !state.is_loading {
                            format!(
                                r#"<span class="chat-copy" data-action="chat-regen" title="이 답변 다시 생성">{regen_svg}<span>다시 생성</span></span>"#
                            )
                        } else {
                            String::new()
                        };
                        let hint = if Some(idx) == last_assistant {
                            r#"<span class="sel-hint">드래그해서 일부만 선택 → Ctrl+C</span>"#
                        } else {
                            ""
                        };
                        format!(
                            r#"<div class="msg-a-ftr"><span class="chat-copy" data-action="chat-copy-msg:{idx}" title="답변 전체 복사">{copy_svg}<span>복사</span></span>{regen}{hint}</div>"#
                        )
                    };
                    let tag = msg.provider_label.as_deref().unwrap_or("AI");
                    // A typing reply is its own entrance; don't stack a slide on top of it.
                    let (enter_cls, enter_style) = if typing.is_some() {
                        ("", String::new())
                    } else {
                        (enter_cls, enter_style)
                    };
                    let _ = write!(
                        h,
                        r#"<div class="msg-a{enter_cls}"{enter_style}><div class="msg-a-hdr"><span class="msg-a-ico">{spark_mini}</span><span class="msg-a-tag">{tag}</span></div><div class="msg-body">{body_html}</div>{footer}</div>"#,
                        tag = html_escape(tag),
                    );
                }
                crate::chat::ChatRole::Error => {
                    let tag = msg.provider_label.as_deref().unwrap_or("안내");
                    let show_retry = state.can_retry && idx + 1 == msg_count;
                    let show_login = state.provider.is_cli();
                    let mut acts_html = String::new();
                    if show_retry || show_login {
                        acts_html.push_str(r#"<div class="err-acts">"#);
                        if show_retry {
                            let _ = write!(
                                acts_html,
                                r#"<div class="err-retry" data-action="chat-retry">{}<span>다시 시도</span></div>"#,
                                svg(Icon::Reload, 11)
                            );
                        }
                        if show_login {
                            acts_html.push_str(
                                r#"<div class="err-retry" data-action="chat-open-login"><span>로그인 열기</span></div>"#,
                            );
                        }
                        acts_html.push_str("</div>");
                    }
                    let _ = write!(
                        h,
                        r#"<div class="msg-a err{enter_cls}"{enter_style}><div class="msg-a-hdr"><span class="msg-a-tag">{tag}</span></div><div>{}</div>{acts_html}</div>"#,
                        html_escape(&msg.text),
                    );
                }
            }
        }
    }

    if state.is_loading && !state.show_history {
        let loading_lbl = match state.agent_status {
            Some(st) => st.to_string(),
            None => format!("{} 생각 중…", state.provider.short_label()),
        };
        // Skeleton answer instead of a spinner: the reply's shape is already on screen.
        let _ = write!(
            h,
            r#"<div class="msg-a rb-in"><div class="msg-a-hdr"><span class="msg-a-ico">{spark}</span><span class="msg-a-tag">{lbl}</span><span class="chat-copy sk-stop" data-action="chat-stop">중지</span></div><div class="sk-msg"><div class="sk-line w1"></div><div class="sk-line w2"></div><div class="sk-line w3"></div></div></div>"#,
            spark = svg(Icon::Sparkles, 11),
            lbl = html_escape(&loading_lbl),
        );
        if let Some((desc, host)) = state.agent_confirm {
            let _ = write!(
                h,
                r#"<div class="agent-confirm"><div class="ac-t">새 사이트로 이동 요청</div><div>AI가 <b>{host}</b>(으)로 이동하려고 합니다. 웹페이지에 숨겨진 지시일 수 있으니 요청한 작업이 맞는지 확인하세요.</div><div class="ac-d">{desc}</div><div class="ac-btns"><div class="sbtn" data-action="agent-confirm:deny">중단</div><div class="sbtn primary" data-action="agent-confirm:allow">허용</div></div></div>"#,
                host = html_escape(host),
                desc = html_escape(desc),
            );
        }
        if !state.agent_log.is_empty() {
            h.push_str(r#"<div class="sb-agent-log">"#);
            let start = state.agent_log.len().saturating_sub(6);
            for entry in &state.agent_log[start..] {
                let is_err = entry.starts_with("ERR:");
                let cls = if is_err { "sb-agent-step err" } else { "sb-agent-step" };
                let marker = if is_err { "!" } else { "✓" };
                let body = entry
                    .strip_prefix("OK: ")
                    .or_else(|| entry.strip_prefix("ERR: "))
                    .unwrap_or(entry);
                let _ = write!(
                    h,
                    r#"<div class="{cls}">{marker} {}</div>"#,
                    html_escape(&truncate_chars(body, 56)),
                );
            }
            h.push_str("</div>");
        }
    }
    h.push_str("</div>");

    match state.picker_open {
        SidebarPicker::Provider => {
            let chk = svg(Icon::Check, 13);
            let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
            let _ = write!(
                h,
                r#"<div class="sb-sheet{enter_cls}"{enter_style}><div class="sheet-hdr"><span>에이전트 / API 선택</span></div><div class="sheet-list"><div class="sheet-sec">로컬 로그인 에이전트</div>"#
            );
            for &prov in crate::chat::ChatProvider::ALL {
                if prov == crate::chat::ChatProvider::OpenAiApi {
                    h.push_str(r#"<div class="sheet-sec">직접 API / 로컬 LLM</div>"#);
                }
                let is_act = prov == state.provider;
                let row_cls = if is_act { "sheet-row active" } else { "sheet-row" };
                let dot = if prov.is_cli() {
                    let dc = if state.avail.is_available(prov) {
                        "pdot ok"
                    } else {
                        "pdot off"
                    };
                    format!(r#"<span class="{dc}"></span>"#)
                } else {
                    String::new()
                };
                let right = if is_act {
                    format!(r#"<span class="sheet-chk">{chk}</span>"#)
                } else if prov.is_cli() && !state.avail.is_available(prov) {
                    r#"<span class="sheet-tag">미설치</span>"#.to_string()
                } else {
                    String::new()
                };
                let _ = write!(
                    h,
                    r#"<div class="{row_cls}" data-action="chat-provider:{id}"><div class="sheet-row-l">{dot}<span class="sheet-name">{lbl}</span></div>{right}</div>"#,
                    id = prov.id(),
                    lbl = html_escape(prov.label()),
                );
            }
            h.push_str("</div></div>");
        }
        SidebarPicker::Model => {
            let chk = svg(Icon::Check, 13);
            let refresh_btn = if state.provider == crate::chat::ChatProvider::Agy {
                if state.agy_models_loading {
                    r#"<span class="sheet-tag">불러오는 중…</span>"#.to_string()
                } else {
                    format!(
                        r#"<div class="sheet-act" data-action="chat-refresh-models">{}<span>새로고침</span></div>"#,
                        svg(Icon::Reload, 11)
                    )
                }
            } else {
                String::new()
            };
            let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
            let _ = write!(
                h,
                r#"<div class="sb-sheet{enter_cls}"{enter_style}><div class="sheet-hdr"><span>{} 모델 선택</span>{refresh_btn}</div><div class="sheet-list">"#,
                html_escape(state.provider.short_label()),
            );
            for opt in state.model_options {
                let is_act = opt.id == state.active_model_id;
                let row_cls = if is_act { "sheet-row active" } else { "sheet-row" };
                let sub = if !opt.id.is_empty() && opt.id != opt.label {
                    format!(r#"<span class="sheet-sub">{}</span>"#, html_escape(&opt.id))
                } else {
                    String::new()
                };
                let right = if is_act {
                    format!(r#"<span class="sheet-chk">{chk}</span>"#)
                } else {
                    String::new()
                };
                let _ = write!(
                    h,
                    r#"<div class="{row_cls}" data-action="chat-model:{id}"><div class="sheet-row-l"><div class="sheet-row-txt"><span class="sheet-name">{lbl}</span>{sub}</div></div>{right}</div>"#,
                    id = html_escape(&opt.id),
                    lbl = html_escape(&opt.label),
                );
            }
            let _ = write!(
                h,
                r#"</div><div class="sheet-ftr" data-action="chat-toggle-settings"><span>직접 모델명 입력</span>{}</div></div>"#,
                svg(Icon::ChevronRight, 12),
            );
        }
        SidebarPicker::None => {}
    }

    let prov_pill_cls = if state.picker_open == SidebarPicker::Provider {
        "cpill on"
    } else {
        "cpill"
    };
    let mod_pill_cls = if state.picker_open == SidebarPicker::Model {
        "cpill mod on"
    } else {
        "cpill mod"
    };
    let prov_dot = if state.provider.is_cli() {
        let cls = if state.avail.is_available(state.provider) {
            "pdot ok"
        } else {
            "pdot off"
        };
        format!(r#"<span class="{cls}"></span>"#)
    } else {
        String::new()
    };
    let ctx_cls = if state.include_page && !is_internal {
        "cpill ctx on"
    } else {
        "cpill ctx"
    };
    // No page title here: a title change (unread counters…) would rebuild the whole sidebar.
    let ctx_tip = if is_internal {
        "내부 페이지 (컨텍스트 없음)"
    } else if state.include_page {
        "현재 페이지 본문을 함께 보냄 (끄려면 클릭)"
    } else {
        "현재 페이지 본문 포함 켜기"
    };
    let ctrl_cls = if state.agent_control {
        "cpill ctrl on"
    } else {
        "cpill ctrl"
    };
    let chev = svg(Icon::ChevronDown, 10);
    let send_btn = if state.is_loading {
        format!(
            r#"<div class="send-btn stop" data-action="chat-stop" title="생성 중지">{}</div>"#,
            svg(Icon::Stop, 12)
        )
    } else {
        format!(
            r#"<div class="send-btn" data-action="chat-send" title="전송 (Enter)">{}</div>"#,
            svg(Icon::Send, 13)
        )
    };
    let input_h = chat_input_height(state.draft.split('\n').count());
    let _ = write!(
        h,
        r#"<div class="sb-comp">
  <div class="composer">
    <textarea id="chat-input" rows="1" style="height:{input_h}px" placeholder="무엇이든 물어보세요…">{draft}</textarea>
    <div class="comp-bar">
      <div class="comp-left">
        <div class="{prov_pill_cls}" data-action="chat-picker:provider" title="에이전트 / API 전환">{prov_dot}<span class="cpill-txt">{prov_lbl}</span><span class="cpill-chev">{chev}</span></div>
        <div class="{mod_pill_cls}" data-action="chat-picker:model" title="사용할 모델 변경"><span class="cpill-txt">{mod_lbl}</span><span class="cpill-chev">{chev}</span></div>
        <div class="{ctx_cls}" data-action="chat-toggle-page" title="{ctx_tip}"><span class="cpill-dot"></span><span class="cpill-txt">페이지</span></div>
        <div class="{ctrl_cls}" data-action="chat-toggle-control" title="AI 에이전트 브라우저 화면 제어 (클릭·입력·이동)"><span class="cpill-dot"></span><span class="cpill-txt">제어</span></div>
      </div>
      {send_btn}
    </div>
  </div>
  <div class="comp-hint">Enter 전송 · Shift+Enter 줄바꿈</div>
</div>
</div></body></html>"#,
        draft = html_escape(state.draft),
        prov_lbl = html_escape(state.provider.short_label()),
        mod_lbl = html_escape(&truncate_chars(state.active_model_label, 15)),
    );

    (h, code_blocks)
}

// ── Built-in Page: Home Bento Dashboard (`rust://home`) ──

const HOME_CSS: &str = r#"
body {
  background: var(--surface-2);
  padding: 56px 28px 48px;
}
.wrap { max-width: 820px; margin: 0 auto; }
.wrap > * { animation: rb-rise 220ms var(--ease-out) both; }
.wrap > *:nth-child(2) { animation-delay: 30ms; }
.wrap > *:nth-child(3) { animation-delay: 60ms; }
.wrap > *:nth-child(4) { animation-delay: 90ms; }
.wrap > *:nth-child(5) { animation-delay: 120ms; }
.wrap > *:nth-child(6) { animation-delay: 150ms; }
.tile, .bcard { transition: background 90ms ease, border-color 90ms ease, transform 120ms var(--ease-out); }
.tile:active, .bcard:active { transform: scale(0.97); }
.hero { display: flex; flex-direction: column; align-items: center; margin-bottom: 24px; text-align: center; }
.hero-brand { display: flex; align-items: center; gap: 12px; margin-bottom: 8px; }
.brand-name { font-size: 24px; font-weight: 700; letter-spacing: -0.03em; color: var(--text); }
.brand-name span { color: var(--text-2); font-weight: 500; }
.hero-sub {
  display: inline-flex; align-items: center; gap: 6px;
  font-size: 12px; font-weight: 500; color: var(--text-3);
}
.hero-dot { width: 6px; height: 6px; border-radius: 3px; background: var(--green); flex-shrink: 0; }
.search-wrap {
  display: flex; align-items: center; gap: 11px;
  width: 100%; max-width: 580px; height: 50px; margin: 0 auto 10px; padding: 0 14px 0 20px;
  border-radius: 999px; border: 1px solid var(--border);
  background: var(--surface); box-shadow: var(--shadow-sm);
}
.search-wrap:focus-within { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-ring), var(--shadow-md); }
.search-ico { color: var(--text-3); display: inline-flex; flex-shrink: 0; }
.search-wrap:focus-within .search-ico { color: var(--accent); }
#home-search {
  flex: 1; height: 100%; border: none; background: transparent;
  color: var(--text); font-size: 14.5px; outline: none;
}
.search-kbds { display: flex; align-items: center; gap: 5px; flex-shrink: 0; }
.hint { text-align: center; font-size: 11.5px; color: var(--text-3); margin-bottom: 34px; }

.section-hdr { display: flex; align-items: center; justify-content: space-between; margin-bottom: 10px; padding: 0 4px; }
.section-t { font-size: 11.5px; font-weight: 600; color: var(--text-2); letter-spacing: 0.02em; }
.section-link { font-size: 12px; color: var(--text-2); text-decoration: none; font-weight: 500; }
.section-link:hover { color: var(--text); text-decoration: underline; }

.dial-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(112px, 1fr)); gap: 8px; margin-bottom: 28px; }
.tile {
  display: flex; flex-direction: column; align-items: center; gap: 8px;
  padding: 12px 8px 10px; border-radius: 14px;
  background: transparent; border: 1px solid transparent;
  text-decoration: none; color: var(--text);
}
.tile:hover { background: var(--surface); border-color: var(--border); box-shadow: var(--shadow-xs); }
.tile:active { background: var(--hover); }
.tile-ic {
  width: 46px; height: 46px; display: flex; align-items: center; justify-content: center;
  border-radius: 14px; color: #fff; font-size: 16px; font-weight: 700;
  box-shadow: var(--shadow-xs);
}
.tile-ic.img { background: var(--surface); border: 1px solid var(--border); }
.tile-ic img { width: 22px; height: 22px; border-radius: 5px; }
.tile-t { font-size: 12px; font-weight: 600; text-align: center; white-space: nowrap; overflow: hidden; max-width: 100%; }
.tile-d { font-size: 10.5px; color: var(--text-3); white-space: nowrap; overflow: hidden; max-width: 100%; }

.bento { display: grid; grid-template-columns: 1.05fr 1fr; gap: 14px; margin-bottom: 28px; }
@media (max-width: 740px) {
  .bento { grid-template-columns: 1fr; }
}
.bcard {
  padding: 18px 20px; border-radius: 16px;
  background: var(--surface); border: 1px solid var(--border); box-shadow: var(--shadow-xs);
}
.bcard-hdr { display: flex; align-items: center; justify-content: space-between; margin-bottom: 14px; }
.bcard-title { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 600; }
.bcard-ico { color: var(--text-2); display: inline-flex; }
.bcard-meta { display: flex; align-items: center; gap: 8px; }
.status-tag {
  font-size: 11px; font-weight: 600; padding: 2px 8px; border-radius: 999px;
  background: var(--green-bg); color: var(--green-text);
}
.status-tag.warn { background: var(--amber-bg); color: var(--amber-text); }
.stat-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 9px; margin-bottom: 12px; }
.stat-box { padding: 11px 13px; border-radius: 11px; background: var(--surface-2); border: 1px solid var(--divider); }
.stat-k { font-size: 11px; color: var(--text-2); margin-bottom: 3px; }
.stat-v { font-size: 18.5px; font-weight: 700; letter-spacing: -0.02em; }
.stat-v.green { color: var(--green); }
.mem-meter { padding: 10px 13px; border-radius: 11px; background: var(--surface-2); border: 1px solid var(--divider); }
.mem-top { display: flex; justify-content: space-between; font-size: 11px; color: var(--text-2); margin-bottom: 7px; }
.mem-track { height: 4px; border-radius: 999px; background: var(--surface-3); overflow: hidden; }
.mem-fill { height: 100%; border-radius: 999px; background: var(--ink); }

.hist-list { display: flex; flex-direction: column; gap: 3px; }
.hrow {
  display: flex; align-items: center; gap: 9px; padding: 7px 9px;
  border-radius: 9px; text-decoration: none; color: var(--text); font-size: 12.5px;
}
.hrow:hover { background: var(--hover); }
.hrow-t { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; font-weight: 500; }
.hrow-time { font-size: 11px; color: var(--text-3); flex-shrink: 0; }
.hempty { padding: 28px 12px; text-align: center; font-size: 12.5px; color: var(--text-3); }

.keys {
  display: flex; flex-wrap: wrap; justify-content: center; gap: 10px 18px;
  padding-top: 16px; border-top: 1px solid var(--divider);
  font-size: 11.5px; color: var(--text-3);
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
    let health_tag = if degraded_count > 0 {
        format!(r#"<span class="status-tag warn num">불완전 로드 {degraded_count}</span>"#)
    } else {
        r#"<span class="status-tag">정상</span>"#.to_string()
    };

    let mut h = String::with_capacity(14 * 1024);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>새 탭</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(HOME_CSS);
    let _ = write!(
        h,
        r#"</style></head><body><div class="wrap">
<div class="hero">
  <div class="hero-brand">{emblem}<div class="brand-name">Rust <span>Clean</span></div></div>
  <div class="hero-sub"><span class="hero-dot"></span><span class="num">Zero-JS · {working_set_mb:.0} MB 사용 중</span></div>
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
      <div class="bcard-meta">{health_tag}<span class="section-t num">북마크 {bm_count}</span></div>
    </div>
    <div class="stat-grid">
      <div class="stat-box"><div class="stat-k">현재 메모리</div><div class="stat-v green num">{working_set_mb:.0} MB</div></div>
      <div class="stat-box"><div class="stat-k">절전 / 전체 탭</div><div class="stat-v num">{unloaded_count} / {tab_count}</div></div>
      <div class="stat-box"><div class="stat-k">차단된 트래커</div><div class="stat-v num">{blocked_count}</div></div>
      <div class="stat-box"><div class="stat-k">방문 기록</div><div class="stat-v num">{hist_count}</div></div>
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
  <span class="key-item"><kbd>Ctrl+K</kbd> 명령 팔레트</span>
  <span class="key-item"><kbd>Ctrl+F</kbd> 찾기</span>
  <span class="key-item"><kbd>Alt+R</kbd> 리더 모드</span>
  <span class="key-item"><kbd>Ctrl+Shift+I</kbd> AI</span>
</div>
</div></body></html>"#,
        emblem = brand_emblem_svg(dark_mode),
        search_ico = svg(Icon::Search, 18),
        cpu_ico = svg(Icon::Cpu, 16),
        hist_ico = svg(Icon::History, 16),
        bm_count = bookmarks.total_count(),
        hist_count = history.len(),
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
  padding: 13px 24px; background: var(--surface); border-bottom: 1px solid var(--border);
  box-shadow: var(--shadow-xs);
}
.brand { display: flex; align-items: center; gap: 9px; font-size: 15px; font-weight: 700; letter-spacing: -0.015em; }
.brand-ico { color: var(--accent); display: inline-flex; }
.search { display: flex; align-items: center; gap: 8px; flex: 1; max-width: 560px; }
#bm-search {
  flex: 1; height: 36px; padding: 0 14px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--omni);
  color: var(--text); font-size: 13px; outline: none;
}
#bm-search:focus { background: var(--surface); border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-ring); }
.btn {
  display: inline-flex; align-items: center; gap: 6px; height: 34px; padding: 0 13px;
  border-radius: 999px; border: 1px solid var(--border); background: var(--surface);
  color: var(--text-2); font-size: 12.5px; font-weight: 500; text-decoration: none; white-space: nowrap;
}
.btn:hover { background: var(--hover); color: var(--text); border-color: var(--border-strong); }
.layout { display: flex; min-height: 600px; max-width: 1240px; margin: 0 auto; }
.side { width: 256px; flex-shrink: 0; padding: 16px 12px; border-right: 1px solid var(--divider); }
.sb {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  padding: 8px 12px; border-radius: 9px; text-decoration: none;
  color: var(--text); font-size: 13px; margin-bottom: 2px;
}
.sb:hover { background: var(--hover); }
.sb.active, .sb.drop {
  background: var(--surface-3); color: var(--text); font-weight: 600;
}
.sb-l { display: flex; align-items: center; gap: 8px; min-width: 0; overflow: hidden; }
.sb-n { font-size: 11px; color: var(--text-3); padding: 1px 6px; border-radius: 6px; background: var(--surface-3); }
.sb.active .sb-n { background: var(--surface); color: var(--accent-text); }
.main { flex: 1; padding: 22px 28px 56px; min-width: 0; }
.ptitle { margin: 0 0 14px; font-size: 18px; font-weight: 700; letter-spacing: -0.02em; }
.folders { display: flex; flex-wrap: wrap; gap: 9px; margin-bottom: 16px; }
.fcard {
  display: flex; align-items: center; gap: 8px; padding: 9px 13px;
  border-radius: 11px; background: var(--surface); border: 1px solid var(--border);
  text-decoration: none; color: var(--text); font-size: 13px; font-weight: 500;
  box-shadow: var(--shadow-xs);
}
.fcard:hover { border-color: var(--accent); background: var(--hover); }
.fcard.drop { background: var(--accent-bg); color: var(--accent-text); border-color: var(--accent); font-weight: 600; }
.fcnt { color: var(--text-3); font-size: 11px; }
.ico-f { color: var(--amber); opacity: 0.85; display: inline-flex; flex-shrink: 0; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 13px; overflow: hidden; box-shadow: var(--shadow-xs); }
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
.rdom { font-size: 11px; padding: 2px 7px; border-radius: 6px; background: var(--surface-3); color: var(--text-2); flex-shrink: 0; }
.rurl { color: var(--text-3); font-size: 11.5px; margin-left: auto; white-space: nowrap; overflow: hidden; max-width: 320px; }
.rdel {
  display: flex; align-items: center; justify-content: center;
  width: 30px; height: 30px; margin-right: 10px; border-radius: 7px;
  color: var(--text-3); text-decoration: none; flex-shrink: 0;
}
.rdel:hover { background: var(--danger-bg); color: var(--danger); }
.empty { padding: 36px; text-align: center; background: var(--surface); border: 1px solid var(--border); border-radius: 12px; color: var(--text-3); font-size: 13px; }
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
  padding: 13px 24px; background: var(--surface); border-bottom: 1px solid var(--border);
  box-shadow: var(--shadow-xs);
}
.brand { display: flex; align-items: center; gap: 9px; font-size: 15px; font-weight: 700; letter-spacing: -0.015em; }
.brand-ico { color: var(--accent); display: inline-flex; }
.search { display: flex; align-items: center; gap: 8px; flex: 1; max-width: 560px; }
#hist-search {
  flex: 1; height: 36px; padding: 0 14px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--omni);
  color: var(--text); font-size: 13px; outline: none;
}
#hist-search:focus { background: var(--surface); border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-ring); }
.btn {
  display: inline-flex; align-items: center; gap: 6px; height: 34px; padding: 0 13px;
  border-radius: 999px; border: 1px solid var(--border); background: var(--surface);
  color: var(--text-2); font-size: 12.5px; font-weight: 500; text-decoration: none; white-space: nowrap;
}
.btn:hover { background: var(--hover); color: var(--text); border-color: var(--border-strong); }
.btn.danger:hover { background: var(--danger-bg); color: var(--danger); border-color: transparent; }
.wrap { max-width: 920px; margin: 0 auto; padding: 24px 24px 64px; }
.group-t { font-size: 12.5px; font-weight: 700; color: var(--text-2); margin: 18px 0 8px 4px; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 13px; overflow: hidden; box-shadow: var(--shadow-xs); margin-bottom: 12px; }
.row-wrap { display: flex; align-items: center; border-bottom: 1px solid var(--divider); }
.row-wrap:last-child { border-bottom: none; }
.row-wrap:hover { background: var(--hover); }
.row { display: flex; align-items: center; gap: 11px; flex: 1; min-width: 0; padding: 10px 16px; text-decoration: none; color: var(--text); font-size: 13px; }
.htitle { font-weight: 500; white-space: nowrap; overflow: hidden; max-width: 360px; }
.hurl { flex: 1; min-width: 0; font-size: 11.5px; color: var(--text-3); white-space: nowrap; overflow: hidden; }
.hcnt { font-size: 11px; padding: 2px 6px; border-radius: 6px; background: var(--surface-3); color: var(--text-2); flex-shrink: 0; }
.htime { font-size: 11.5px; color: var(--text-3); flex-shrink: 0; }
.rdel { display: flex; align-items: center; justify-content: center; width: 30px; height: 30px; margin-right: 10px; border-radius: 7px; color: var(--text-3); text-decoration: none; flex-shrink: 0; }
.rdel:hover { background: var(--danger-bg); color: var(--danger); }
.empty { padding: 48px; text-align: center; background: var(--surface); border: 1px solid var(--border); border-radius: 12px; color: var(--text-3); font-size: 13px; }
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

pub(crate) fn is_loopback_http(url: &str) -> bool {
    let Some(after_scheme) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
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
  border-radius: 16px; background: var(--surface); border: 1px solid var(--border);
  border-top: 3px solid var(--danger);
  box-shadow: var(--shadow-md);
}
.ic { margin-bottom: 18px; }
h1 { font-size: 21px; font-weight: 700; margin-bottom: 10px; letter-spacing: -0.025em; }
p { color: var(--text-2); font-size: 14px; line-height: 1.6; margin-bottom: 12px; }
.url-chip {
  padding: 9px 12px; border-radius: 9px; background: var(--surface-2);
  border: 1px solid var(--divider); color: var(--text-2);
  font-family: "Cascadia Code", Consolas, "JetBrains Mono", monospace;
  font-size: 12px; word-break: break-all; margin-bottom: 22px;
}
.actions { display: flex; flex-wrap: wrap; gap: 10px; }
.btn {
  display: inline-flex; align-items: center; gap: 7px; height: 36px; padding: 0 17px;
  border-radius: 999px; font-size: 13px; font-weight: 600; text-decoration: none;
  border: 1px solid var(--border); color: var(--text); background: var(--surface);
}
.btn:hover { background: var(--hover); border-color: var(--border-strong); }
.btn.primary { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); box-shadow: var(--shadow-xs); }
.btn.primary:hover { opacity: 0.88; }
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
            motion: Default::default(),
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
            detaching_tab_id: None,
            downloads_in_progress: 0,
            downloads_unseen: 0,
            sidebar_open: false,
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
            },
            crate::chat::ChatMessage {
                role: crate::chat::ChatRole::Assistant,
                text: "## 요약\n- **빠른** 브라우저\n\n```rust\nprintln!(\"hi\");\n```".into(),
                page_title: None,
                provider_label: Some("Claude · Claude Sonnet".into()),
            },
            crate::chat::ChatMessage {
                role: crate::chat::ChatRole::Error,
                text: "로그아웃되었습니다.".into(),
                page_title: None,
                provider_label: Some("Claude".into()),
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
            custom_presets: &[crate::chat::AgentPreset {
                title: "표 수집".into(),
                prompt: "표 추출".into(),
            }],
            model_options: &model_options,
            agy_models_loading: false,
            messages: &messages,
            is_loading: false,
            can_retry: true,
            include_page: true,
            agent_control: true,
            agent_status: None,
            agent_log: &[],
            sidebar_wide: false,
            show_settings: true,
            active_title: "Rust",
            active_url: "https://www.rust-lang.org/",
            api_endpoint: "",
            api_model: "sonnet",
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
        let inp = doc.get_element_by_id("chat-input").expect("chat-input");
        let sz = doc.get_node(inp).unwrap().final_layout().size;
        assert!(sz.width > 120.0 && sz.height >= 24.0, "chat-input={sz:?}");

        let (loading_html, _) = render_sidebar_html(&SidebarRenderState {
            provider: crate::chat::ChatProvider::Claude,
            avail: crate::chat::CliAvailability::default(),
            picker_open: SidebarPicker::None,
            active_model_id: "sonnet",
            active_model_label: "Claude Sonnet",
            control_model_id: "haiku",
            custom_presets: &[],
            model_options: &model_options,
            agy_models_loading: false,
            messages: &messages,
            is_loading: true,
            can_retry: false,
            include_page: true,
            agent_control: true,
            agent_status: Some("2/10단계 · 요소 [1] 클릭"),
            agent_log: &["OK: 이동 완료".into(), "ERR: 요소 없음".into()],
            sidebar_wide: false,
            show_settings: false,
            active_title: "Rust",
            active_url: "https://www.rust-lang.org/",
            api_endpoint: "",
            api_model: "sonnet",
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
        assert!(loading_html.contains("sb-agent-log"));
        assert!(loading_html.contains("이동 완료"));
        assert!(loading_html.contains("sb-agent-step err"));
        assert!(loading_html.contains("agent-confirm:allow") && loading_html.contains("evil.test"));
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
        let sk = layout(&render_page_skeleton_html());
        assert!(sk.is_animating(), "page skeleton pulses");

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
}
