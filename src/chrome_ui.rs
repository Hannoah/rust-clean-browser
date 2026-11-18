use crate::bookmarks::{BookmarkItem, BookmarkNode, BookmarkRootKind, BookmarkStore, count_urls};
use crate::cleaner::html_escape;
use crate::tabs::Tab;
use std::fmt::Write;

/// Tab strip doubles as the (frameless) window title bar.
pub const TITLEBAR_HEIGHT: u32 = 40;
pub const CHROME_HEIGHT: u32 = TITLEBAR_HEIGHT + 44 + 32;
pub const POPUP_WIDTH: u32 = 320;
pub const POPUP_MAX_HEIGHT: u32 = 440;

/// Theme tokens shared by every built-in page. `prefers-color-scheme` follows the
/// viewport colour scheme, which tracks the OS light/dark setting.
const THEME_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --frame: #dfe3ea; --frame-hover: #e9ecf1; --surface: #ffffff; --surface-2: #f6f8fb;
  --omni: #edf0f4; --omni-hover: #e5e9ef; --border: #d5d9e0; --divider: #e6e9ee;
  --text: #1f1f1f; --text-2: #5f6368; --text-3: #8a8f96;
  --accent: #0b57d0; --accent-bg: #d3e3fd; --accent-text: #041e49;
  --hover: #0000000f; --press: #0000001c; --danger: #c42b1c; --shadow: #0000001f;
}
@media (prefers-color-scheme: dark) {
  :root {
    --frame: #1b1c1f; --frame-hover: #26272b; --surface: #2c2d31; --surface-2: #232428;
    --omni: #1b1c1f; --omni-hover: #222327; --border: #3c3d42; --divider: #37383d;
    --text: #e8eaed; --text-2: #b4b8bf; --text-3: #8b9097;
    --accent: #a8c7fa; --accent-bg: #0842a0; --accent-text: #d3e3fd;
    --hover: #ffffff14; --press: #ffffff24; --danger: #c42b1c; --shadow: #00000066;
  }
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body { font-family: "Segoe UI Variable Text", "Segoe UI", -apple-system, "SF Pro Text", "Helvetica Neue", "Malgun Gothic", "Apple SD Gothic Neo", "Noto Sans CJK KR", sans-serif; color: var(--text); }
/* Site icon: cached favicon <img>, or a coloured monogram badge. */
.fav { display: inline-block; width: 16px; height: 16px; line-height: 16px; text-align: center; border-radius: 4px; color: #fff; font-size: 10px; font-weight: 700; flex-shrink: 0; }
img.fav { border-radius: 3px; }
.ico { font-family: "Segoe Fluent Icons", "Segoe MDL2 Assets", "Apple Symbols", "Lucida Grande", "DejaVu Sans", sans-serif; font-weight: normal; font-style: normal; }
"#;

const CHROME_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--frame); user-select: none; }
.chrome { display: flex; flex-direction: column; width: 100%; height: 100%; }

.titlebar { display: flex; align-items: flex-end; height: 40px; padding-left: 8px; background: var(--frame); }
.tabs { display: flex; align-items: flex-end; flex: 0 1 auto; min-width: 0; overflow: hidden; height: 40px; padding-top: 6px; }
.tab { position: relative; display: flex; align-items: center; gap: 8px; width: 220px; flex: 0 1 auto; min-width: 56px; height: 34px;
  padding: 0 6px 0 12px; border-radius: 10px 10px 0 0; color: var(--text-2); font-size: 12px; }
.tab:hover { background: var(--frame-hover); color: var(--text); }
.tab.active { background: var(--surface); color: var(--text); }
.tab.asleep .tab-title { opacity: 0.62; }
.tab-sep { width: 1px; height: 18px; margin-bottom: 8px; background: var(--border); flex-shrink: 0; }
.fav.glyph { color: var(--text-2); font-size: 13px; }
.fav.loading { color: var(--accent); font-size: 13px; }
.tab-title { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; }
/* Filled chips, no 1px outlines: thin borders land between device pixels at 125%/150% scale
   and look broken on one side. */
.tab-badge { font-size: 9px; font-weight: 700; color: var(--accent); background: var(--accent-bg); border-radius: 4px; padding: 1px 5px; line-height: 12px; }
.tab-close { width: 20px; height: 20px; line-height: 20px; text-align: center; border-radius: 10px; font-size: 9px; color: var(--text-2); flex-shrink: 0; }
.tab-close:hover { background: var(--press); color: var(--text); }
.newtab { width: 28px; height: 28px; line-height: 28px; margin: 0 0 3px 6px; text-align: center; border-radius: 14px; font-size: 12px; color: var(--text); flex-shrink: 0; }
.newtab:hover { background: var(--hover); }
.drag { flex: 1 1 0; min-width: 32px; height: 40px; }
.winctl { display: flex; align-self: flex-start; height: 40px; flex-shrink: 0; }
.wbtn { width: 46px; height: 40px; line-height: 40px; text-align: center; font-size: 10px; color: var(--text); }
.wbtn:hover { background: var(--hover); }
.wbtn.close:hover { background: var(--danger); color: #ffffff; }

.toolbar { display: flex; align-items: center; gap: 2px; height: 44px; padding: 0 8px; background: var(--surface); }
.ib { position: relative; width: 34px; height: 34px; line-height: 34px; text-align: center; border-radius: 17px; font-size: 15px; color: var(--text); flex-shrink: 0; }
.ib:hover { background: var(--hover); }
.ib.off { color: var(--text-3); }
.ib.off:hover { background: transparent; }
.ib.on { color: var(--accent); background: var(--accent-bg); }
.ib .cnt { position: absolute; right: 2px; bottom: 3px; min-width: 14px; height: 13px; line-height: 13px; padding: 0 3px; border-radius: 7px;
  font-family: "Segoe UI", sans-serif; font-size: 8px; font-weight: 700; background: var(--accent); color: var(--surface); }
.omni { position: relative; flex: 1; min-width: 120px; height: 34px; margin: 0 6px; }
#url-bar { width: 100%; height: 34px; padding: 0 40px 0 38px; border-radius: 17px; border: 2px solid var(--omni); background: var(--omni);
  color: var(--text); font-size: 14px; outline: none; }
#url-bar:hover { background: var(--omni-hover); border-color: var(--omni-hover); }
#url-bar:focus { background: var(--surface); border-color: var(--accent); }
.omni-lead { position: absolute; left: 13px; top: 0; height: 34px; line-height: 34px; font-size: 13px; color: var(--text-2); }
.omni-star { position: absolute; right: 4px; top: 3px; width: 28px; height: 28px; line-height: 28px; text-align: center; border-radius: 14px; font-size: 14px; color: var(--text-2); }
.omni-star:hover { background: var(--hover); }
.omni-star.on { color: var(--accent); }
.pill { height: 26px; line-height: 26px; padding: 0 11px; margin: 0 2px; border-radius: 13px; background: var(--omni); font-size: 11px; color: var(--text-2); white-space: nowrap; flex-shrink: 0; }
.pill:hover { background: var(--omni-hover); color: var(--text); }
.pill.accent { background: var(--accent-bg); color: var(--accent); }
.vsep { width: 1px; height: 20px; margin: 0 4px; background: var(--divider); flex-shrink: 0; }

.bmbar { display: flex; align-items: center; gap: 2px; height: 32px; padding: 0 8px 2px; background: var(--surface); border-bottom: 1px solid var(--divider); overflow: hidden; }
.bm-items { display: flex; align-items: center; gap: 2px; flex: 1; min-width: 0; overflow: hidden; }
.chip { display: flex; align-items: center; gap: 6px; height: 26px; padding: 0 8px; border-radius: 13px; color: var(--text); font-size: 12px; white-space: nowrap; flex-shrink: 0; }
.chip:hover { background: var(--hover); }
.chip .fav { font-size: 9px; }
.chip .fico { font-size: 13px; color: var(--text-2); }
.bm-right { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.bm-empty { font-size: 12px; color: var(--text-3); padding-left: 6px; }
"#;

/// Windows: frameless window with our own min/max/close buttons.
/// macOS: transparent native title bar, so leave room for the traffic lights.
/// Other platforms: native decorations.
const CUSTOM_WINDOW_CONTROLS: bool = cfg!(target_os = "windows");
const TRAFFIC_LIGHT_INSET: u32 = if cfg!(target_os = "macos") { 78 } else { 8 };

/// Segoe Fluent/MDL2 icon code points only exist on Windows; elsewhere swap them for
/// plain Unicode symbols available in default system fonts (no emoji font load).
pub fn platform_icons(html: String) -> String {
    if cfg!(target_os = "windows") {
        html
    } else {
        substitute_icon_entities(&html)
    }
}

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
            out.push_str("&");
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
    pub working_set_mb: f64,
    pub blocked_count: usize,
    pub filter_enabled: bool,
    pub status_note: Option<&'a str>,
    pub maximized: bool,
    /// Text the user is typing in the URL bar, kept across chrome rebuilds.
    pub url_override: Option<&'a str>,
    /// Tab whose JS-mode WebView is kept alive in the background (not shown as asleep).
    pub parked_tab: Option<u64>,
}

fn icon(cp: &str) -> String {
    format!(r#"<span class="ico">&#x{cp};</span>"#)
}

fn favicon_html(title: &str, url: &str) -> String {
    if url.starts_with("rust://") {
        let cp = if url.starts_with("rust://bookmarks") { "E728" } else { "E80F" };
        return format!(r#"<span class="fav glyph ico">&#x{cp};</span>"#);
    }
    if let Some(src) = crate::favicon::cached_icon_url(url) {
        return format!(r#"<img class="fav img" src="{}" />"#, html_escape(&src));
    }
    let (initial, color) = monogram_badge(title, url);
    format!(r#"<span class="fav" style="background:{color}">{initial}</span>"#)
}

pub fn render_chrome_html(state: &ChromeRenderState<'_>) -> String {
    let active = &state.tabs[state.active_idx];
    let mut h = String::with_capacity(12 * 1024);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(CHROME_CSS);
    h.push_str("</style></head><body><div class=\"chrome\">");

    // ── Title bar: tabs + drag area + window controls ──
    let _ = write!(
        h,
        r#"<div class="titlebar" data-drag="1" style="padding-left:{TRAFFIC_LIGHT_INSET}px"><div class="tabs" data-drag="1">"#
    );
    for (idx, tab) in state.tabs.iter().enumerate() {
        let is_active = idx == state.active_idx;
        let next_active = idx + 1 == state.active_idx;
        let asleep = !is_active && !tab.is_loaded() && state.parked_tab != Some(tab.id);
        let cls = match (is_active, asleep) {
            (true, _) => "tab active",
            (false, true) => "tab asleep",
            _ => "tab",
        };
        let fav = if tab.is_loading {
            r#"<span class="fav loading ico">&#xE895;</span>"#.to_string()
        } else {
            favicon_html(&tab.title, &tab.url)
        };
        let badge = if is_active && tab.js_mode {
            r#"<span class="tab-badge">JS</span>"#
        } else {
            ""
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="tab:{idx}">{fav}<span class="tab-title">{title}</span>{badge}<span class="tab-close ico" data-action="close-tab:{idx}">&#xE8BB;</span></div>"#,
            title = html_escape(&truncate_chars(&tab.title, 26)),
        );
        if !is_active && !next_active && idx + 1 < state.tabs.len() {
            h.push_str(r#"<div class="tab-sep"></div>"#);
        }
    }
    h.push_str(r#"</div><div class="newtab ico" data-action="new-tab">&#xE710;</div><div class="drag" data-drag="1"></div>"#);
    if CUSTOM_WINDOW_CONTROLS {
        let _ = write!(
            h,
            r#"<div class="winctl"><div class="wbtn ico" data-action="win-min">&#xE921;</div><div class="wbtn ico" data-action="win-max">&#x{max_cp};</div><div class="wbtn close ico" data-action="win-close">&#xE8BB;</div></div>"#,
            max_cp = if state.maximized { "E923" } else { "E922" },
        );
    }
    h.push_str("</div>");

    // ── Toolbar ──
    let can_back = active.js_mode || !active.history_back.is_empty();
    let can_forward = active.js_mode || !active.history_forward.is_empty();
    let is_bookmarked = state.bookmarks.contains_url(&active.url);
    let lead_cp = if active.url.starts_with("https://") {
        "E72E" // lock
    } else if active.url.starts_with("rust://") {
        "E721" // search
    } else {
        "E946" // info
    };
    let url_val = match state.url_override {
        Some(t) => t,
        None if active.url == "rust://home" => "",
        None => active.url.as_str(),
    };
    let _ = write!(
        h,
        r#"<div class="toolbar">
<div class="ib{back_off}" data-action="back">{back}</div>
<div class="ib{fwd_off}" data-action="forward">{fwd}</div>
<div class="ib" data-action="reload">{reload}</div>
<div class="ib" data-action="nav:rust://home">{home}</div>
<div class="omni"><input id="url-bar" type="text" value="{url_val}" placeholder="검색어 또는 URL 입력" /><span class="omni-lead ico">&#x{lead_cp};</span><div class="omni-star ico{star_on}" data-action="toggle-bookmark">&#x{star_cp};</div></div>"#,
        back_off = if can_back { "" } else { " off" },
        fwd_off = if can_forward { "" } else { " off" },
        back = icon("E72B"),
        fwd = icon("E72A"),
        reload = icon(if active.is_loading { "E711" } else { "E72C" }),
        home = icon("E80F"),
        url_val = html_escape(url_val),
        star_on = if is_bookmarked { " on" } else { "" },
        star_cp = if is_bookmarked { "E735" } else { "E734" },
    );

    if (active.zoom - 1.0).abs() > 0.001 {
        let _ = write!(
            h,
            r#"<div class="pill accent" data-action="zoom-reset">{:.0}%</div>"#,
            active.zoom * 100.0
        );
    }
    let blocked_badge = if state.filter_enabled && state.blocked_count > 0 {
        format!(r#"<span class="cnt">{}</span>"#, compact_count(state.blocked_count))
    } else {
        String::new()
    };
    let _ = write!(
        h,
        r#"<div class="ib{reader_on}" data-action="toggle-reader">{reader}</div>
<div class="ib{js_on}" data-action="toggle-js">{js}</div>
<div class="ib{shield_on}" data-action="toggle-filter">{shield}{blocked_badge}</div>
<div class="ib" data-action="nav:rust://bookmarks">{bm}</div>
<div class="vsep"></div>
<div class="pill" data-action="nav:rust://home">{ram}</div>
</div>"#,
        reader_on = if active.reader_mode { " on" } else { "" },
        js_on = if active.js_mode { " on" } else { "" },
        shield_on = if state.filter_enabled { " on" } else { "" },
        reader = icon("E736"),
        js = icon("E943"),
        shield = icon("EA18"),
        bm = icon("E728"),
        ram = match state.status_note {
            Some(note) => format!("{} · {:.0} MB", html_escape(note), state.working_set_mb),
            None => format!("{:.0} MB", state.working_set_mb),
        },
    );

    // ── Bookmarks bar ──
    h.push_str(r#"<div class="bmbar"><div class="bm-items">"#);
    const MAX_CHIPS: usize = 14;
    let mut rendered = 0usize;
    for item in state.bookmarks.custom.iter().take(MAX_CHIPS) {
        push_url_chip(&mut h, item);
        rendered += 1;
    }
    let mut shown_bar = 0usize;
    for (i, node) in state.bookmarks.bar.iter().enumerate() {
        if rendered >= MAX_CHIPS {
            break;
        }
        shown_bar = i + 1;
        rendered += 1;
        match node {
            BookmarkNode::Url(item) => push_url_chip(&mut h, item),
            BookmarkNode::Folder(folder) => {
                let _ = write!(
                    h,
                    r#"<div class="chip" data-action="bm-popup:bar:{i}"><span class="fico ico">&#xE8B7;</span><span>{}</span></div>"#,
                    html_escape(&truncate_chars(&folder.title, 16)),
                );
            }
        }
    }
    if rendered == 0 {
        h.push_str(r#"<span class="bm-empty">북마크를 추가하려면 주소창의 ☆를 누르세요</span>"#);
    }
    h.push_str(r#"</div><div class="bm-right">"#);
    if state.bookmarks.bar.len() > shown_bar {
        let _ = write!(
            h,
            r#"<div class="chip" data-action="bm-popup:overflow:{shown_bar}"><span class="fico ico">&#xE76C;</span><span>{}</span></div>"#,
            state.bookmarks.bar.len() - shown_bar
        );
    }
    if !state.bookmarks.other.is_empty() {
        h.push_str(r#"<div class="chip" data-action="bm-popup:other"><span class="fico ico">&#xE8B7;</span><span>기타 북마크</span></div>"#);
    }
    let _ = write!(
        h,
        r#"<div class="chip" data-action="sync-chrome"><span class="fico ico">&#xE895;</span><span>{}</span></div></div></div></div></body></html>"#,
        html_escape(&state.bookmarks.source_label),
    );
    platform_icons(h)
}

fn push_url_chip(h: &mut String, item: &BookmarkItem) {
    let _ = write!(
        h,
        r#"<div class="chip" data-action="nav:{url}">{fav}<span>{title}</span></div>"#,
        url = html_escape(&item.url),
        fav = favicon_html(&item.title, &item.url),
        title = html_escape(&truncate_chars(&item.title, 18)),
    );
}

fn compact_count(n: usize) -> String {
    if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

/// Ideal logical popup height (40px header + 32px rows) for the number of items shown.
pub fn estimate_popup_height(item_count: usize) -> u32 {
    (40 + (item_count.max(1) as u32) * 32 + 12).clamp(84, POPUP_MAX_HEIGHT)
}

const POPUP_CSS: &str = r#"
html, body { width: 100%; min-height: 100%; background: var(--surface); user-select: none; }
.shell { min-height: 100vh; border: 1px solid var(--border); background: var(--surface); }
.hdr { display: flex; align-items: center; gap: 6px; height: 40px; padding: 0 8px 0 10px; border-bottom: 1px solid var(--divider); }
.back { width: 28px; height: 28px; line-height: 28px; text-align: center; border-radius: 14px; font-size: 12px; color: var(--text); }
.back:hover { background: var(--hover); }
.hdr-title { flex: 1; font-size: 13px; font-weight: 600; white-space: nowrap; overflow: hidden; }
.mgr { font-size: 12px; color: var(--accent); padding: 4px 8px; border-radius: 12px; }
.mgr:hover { background: var(--hover); }
.list { padding: 6px; }
.row { display: flex; align-items: center; gap: 10px; height: 32px; padding: 0 10px; border-radius: 8px; font-size: 13px; }
.row:hover { background: var(--hover); }
.fico { font-size: 14px; color: var(--text-2); flex-shrink: 0; }
.title { flex: 1; white-space: nowrap; overflow: hidden; }
.host { color: var(--text-3); font-size: 11px; white-space: nowrap; flex-shrink: 0; }
.arrow { color: var(--text-2); font-size: 10px; }
.empty { padding: 18px; text-align: center; color: var(--text-3); font-size: 12px; }
"#;

pub fn render_bookmark_popup_html(
    title: &str,
    nodes: &[BookmarkNode],
    can_go_back: bool,
    index_offset: usize,
) -> String {
    let mut h = String::with_capacity(4096 + nodes.len() * 256);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(POPUP_CSS);
    h.push_str(r#"</style></head><body><div class="shell"><div class="hdr">"#);
    if can_go_back {
        h.push_str(r#"<div class="back ico" data-action="popup-back">&#xE72B;</div>"#);
    }
    let _ = write!(
        h,
        r#"<div class="hdr-title">{}</div><div class="mgr" data-action="popup-manager">관리자</div></div><div class="list">"#,
        html_escape(title)
    );
    for (idx, node) in nodes.iter().enumerate() {
        let abs_idx = index_offset + idx;
        match node {
            BookmarkNode::Folder(folder) => {
                let _ = write!(
                    h,
                    r#"<div class="row" data-action="popup-enter:{abs_idx}"><span class="fico ico">&#xE8B7;</span><span class="title">{}</span><span class="arrow ico">&#xE76C;</span></div>"#,
                    html_escape(&truncate_chars(&folder.title, 32)),
                );
            }
            BookmarkNode::Url(item) => {
                let _ = write!(
                    h,
                    r#"<div class="row" data-action="popup-open:{url}">{fav}<span class="title">{name}</span><span class="host">{host}</span></div>"#,
                    url = html_escape(&item.url),
                    fav = favicon_html(&item.title, &item.url),
                    name = html_escape(&truncate_chars(&item.title, 30)),
                    host = html_escape(&short_host(&item.url)),
                );
            }
        }
    }
    if nodes.is_empty() {
        h.push_str(r#"<div class="empty">비어 있는 폴더입니다.</div>"#);
    }
    h.push_str("</div></div></body></html>");
    platform_icons(h)
}

const HOME_CSS: &str = r#"
body { background: var(--surface-2); padding: 72px 28px 64px; }
.wrap { max-width: 860px; margin: 0 auto; }
.brand { text-align: center; font-size: 30px; font-weight: 600; letter-spacing: -0.5px; margin-bottom: 22px; }
.brand span { color: var(--accent); }
#home-search { display: block; width: 100%; max-width: 640px; height: 48px; margin: 0 auto 10px; padding: 0 24px; border-radius: 24px;
  border: 1px solid var(--border); background: var(--surface); color: var(--text); font-size: 16px; outline: none; }
#home-search:focus { border-color: var(--accent); }
.hint { text-align: center; font-size: 12px; color: var(--text-3); margin-bottom: 40px; }
.grid { display: flex; flex-wrap: wrap; justify-content: center; gap: 8px; margin-bottom: 40px; }
.tile { display: flex; flex-direction: column; align-items: center; gap: 8px; width: 104px; padding: 14px 6px 10px; border-radius: 12px;
  text-decoration: none; color: var(--text); }
.tile:hover { background: var(--hover); }
.tile-ic { width: 44px; height: 44px; line-height: 44px; text-align: center; border-radius: 22px; color: #fff; font-size: 17px; font-weight: 700; }
.tile-ic.img { display: flex; align-items: center; justify-content: center; background: var(--surface); border: 1px solid var(--divider); }
.tile-ic img { width: 24px; height: 24px; }
.tile-t { font-size: 12px; text-align: center; white-space: nowrap; overflow: hidden; max-width: 96px; }
.stats { display: flex; justify-content: center; flex-wrap: wrap; gap: 10px; }
.stat { min-width: 150px; padding: 12px 16px; border-radius: 12px; background: var(--surface); border: 1px solid var(--divider); }
.stat-k { font-size: 11px; color: var(--text-2); margin-bottom: 4px; }
.stat-v { font-size: 18px; font-weight: 600; }
.keys { margin-top: 28px; text-align: center; font-size: 12px; line-height: 1.9; color: var(--text-3); }
.keys b { color: var(--text-2); font-weight: 600; }
a { color: var(--accent); }
"#;

pub fn render_home_page(
    bookmarks: &BookmarkStore,
    working_set_mb: f64,
    tab_count: usize,
    unloaded_count: usize,
    blocked_count: usize,
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
            Some(src) => format!(r#"<span class="tile-ic img"><img src="{}" /></span>"#, html_escape(&src)),
            None => {
                let (initial, color) = monogram_badge(&item.title, &item.url);
                format!(r#"<span class="tile-ic" style="background:{color}">{initial}</span>"#)
            }
        };
        let _ = write!(
            tiles,
            r#"<a class="tile" href="{url}">{icon}<span class="tile-t">{title}</span></a>"#,
            url = html_escape(&item.url),
            title = html_escape(&truncate_chars(&item.title, 14)),
        );
    }
    if tiles.is_empty() {
        for (title, url) in [
            ("Wikipedia", "https://en.wikipedia.org/"),
            ("DuckDuckGo", "https://html.duckduckgo.com/html/"),
            ("Rust Book", "https://doc.rust-lang.org/book/"),
            ("Hacker News", "https://news.ycombinator.com/"),
        ] {
            let (initial, color) = monogram_badge(title, url);
            let _ = write!(
                tiles,
                r#"<a class="tile" href="{url}"><span class="tile-ic" style="background:{color}">{initial}</span><span class="tile-t">{title}</span></a>"#
            );
        }
    }

    let mut h = String::with_capacity(8192);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>새 탭</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(HOME_CSS);
    let _ = write!(
        h,
        r#"</style></head><body><div class="wrap">
<div class="brand">Rust <span>Clean</span> Browser</div>
<input id="home-search" type="text" placeholder="Google 검색 또는 URL 입력" />
<div class="hint">Enter로 이동 · Ctrl+L 주소창 · Ctrl+T 새 탭</div>
<div class="grid">{tiles}</div>
<div class="stats">
<div class="stat"><div class="stat-k">프로세스 메모리</div><div class="stat-v">{working_set_mb:.0} MB</div></div>
<div class="stat"><div class="stat-k">탭 (절전 / 전체)</div><div class="stat-v">{unloaded_count} / {tab_count}</div></div>
<div class="stat"><div class="stat-k">차단된 트래커</div><div class="stat-v">{blocked_count}</div></div>
<div class="stat"><div class="stat-k">북마크</div><div class="stat-v">{bm_count}</div></div>
</div>
<div class="keys"><b>Ctrl+T</b> 새 탭 · <b>Ctrl+W</b> 닫기 · <b>Ctrl+Shift+T</b> 닫은 탭 복원 · <b>Ctrl+Tab</b> 다음 탭 · <b>Ctrl+1~9</b> 탭 이동<br/>
<b>Alt+←/→</b> 뒤로/앞으로 · <b>Alt+R</b> 리더 모드 · <b>Ctrl+D</b> 북마크 · <b>Ctrl+Shift+O</b> <a href="rust://bookmarks">북마크 관리자</a> · <b>Ctrl+클릭</b> 새 탭에서 열기</div>
</div></body></html>"#,
        bm_count = bookmarks.total_count(),
    );
    if cfg!(target_os = "macos") {
        // Shortcut hints use Cmd on macOS (Ctrl+Tab stays Ctrl).
        h.replace("Ctrl+", "⌘").replace("⌘Tab", "Ctrl+Tab").replace("⌘클릭", "⌘+클릭")
    } else {
        h
    }
}

const BOOKMARKS_CSS: &str = r#"
body { background: var(--surface-2); }
.top { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 14px 24px; background: var(--surface); border-bottom: 1px solid var(--divider); }
.brand { font-size: 18px; font-weight: 600; }
.sub { font-size: 12px; color: var(--text-2); }
.search { display: flex; align-items: center; gap: 8px; flex: 1; max-width: 480px; }
#bm-search { flex: 1; height: 36px; padding: 0 16px; border-radius: 18px; border: 1px solid var(--omni); background: var(--omni); color: var(--text); font-size: 13px; outline: none; }
#bm-search:focus { background: var(--surface); border-color: var(--accent); }
.btn { height: 32px; line-height: 30px; padding: 0 14px; border-radius: 16px; border: 1px solid var(--border); color: var(--text); font-size: 12px; text-decoration: none; }
.btn:hover { background: var(--hover); }
.layout { display: flex; min-height: 600px; }
.side { width: 256px; flex-shrink: 0; padding: 12px 8px; }
.sb { display: flex; align-items: center; justify-content: space-between; gap: 6px; padding: 8px 14px; border-radius: 18px; text-decoration: none; color: var(--text); font-size: 13px; margin-bottom: 1px; }
.sb:hover { background: var(--hover); }
.sb.active { background: var(--accent-bg); color: var(--accent-text); font-weight: 600; }
.sb-n { font-size: 11px; color: var(--text-3); }
.main { flex: 1; padding: 18px 28px 48px; min-width: 0; }
.ptitle { margin: 0 0 14px; font-size: 18px; font-weight: 600; }
.folders { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 16px; }
.fcard { display: flex; align-items: center; gap: 8px; padding: 9px 14px; border-radius: 10px; background: var(--surface); border: 1px solid var(--divider); text-decoration: none; color: var(--text); font-size: 13px; }
.fcard:hover { border-color: var(--accent); }
.fcnt { color: var(--text-3); font-size: 11px; }
.ico.f { color: var(--text-2); }
.card { background: var(--surface); border: 1px solid var(--divider); border-radius: 12px; overflow: hidden; }
.row { display: flex; align-items: center; gap: 12px; padding: 10px 16px; border-bottom: 1px solid var(--divider); text-decoration: none; color: var(--text); font-size: 13px; }
.row:hover { background: var(--hover); }
.rurl { color: var(--text-3); font-size: 11px; margin-left: auto; white-space: nowrap; }
.empty { padding: 28px; background: var(--surface); border: 1px solid var(--divider); border-radius: 12px; color: var(--text-3); font-size: 13px; }
"#;

/// Chrome-style 2-column Bookmark Manager (`rust://bookmarks`, `?folder=...`, `?q=...`).
pub fn render_bookmarks_page(bookmarks: &BookmarkStore, current_url: &str) -> String {
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
    let sb_cls = |key: &str| if active_key == key { "sb active" } else { "sb" };
    let _ = write!(
        side,
        r#"<a class="{}" href="rust://bookmarks?folder=custom"><span><span class="ico f">&#xE734;</span> 이 브라우저에서 추가</span><span class="sb-n">{}</span></a>"#,
        sb_cls("custom"),
        bookmarks.custom.len(),
    );
    let _ = write!(
        side,
        r#"<a class="{}" href="rust://bookmarks?folder=bar"><span><span class="ico f">&#xE8B7;</span> 북마크바</span><span class="sb-n">{}</span></a>"#,
        sb_cls("bar"),
        count_urls(&bookmarks.bar),
    );
    render_sidebar_folders(&bookmarks.bar, "bar", 1, &active_key, &mut side);
    if !bookmarks.other.is_empty() {
        let _ = write!(
            side,
            r#"<a class="{}" href="rust://bookmarks?folder=other"><span><span class="ico f">&#xE8B7;</span> 기타 북마크</span><span class="sb-n">{}</span></a>"#,
            sb_cls("other"),
            count_urls(&bookmarks.other),
        );
        render_sidebar_folders(&bookmarks.other, "other", 1, &active_key, &mut side);
    }

    let mut folders = String::new();
    let mut rows = String::new();
    let heading = if let Some(q) = &query {
        let matches = bookmarks.search(q);
        for item in &matches {
            append_manager_url_row(&mut rows, item);
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
                    let _ = write!(
                        folders,
                        r#"<a class="fcard" href="rust://bookmarks?folder={next_key}"><span class="ico f">&#xE8B7;</span><span>{name}</span><span class="fcnt">{}</span></a>"#,
                        count_urls(&f.children),
                        name = html_escape(&f.title),
                    );
                }
                BookmarkNode::Url(item) => append_manager_url_row(&mut rows, item),
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

    let mut h = String::with_capacity(8192 + side.len() + body.len());
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>북마크</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(BOOKMARKS_CSS);
    let _ = write!(
        h,
        r#"</style></head><body>
<div class="top"><div><div class="brand">북마크 ({total})</div><div class="sub">{source}</div></div>
<div class="search"><input id="bm-search" type="text" value="{search_val}" placeholder="북마크 검색 (제목 또는 URL) 후 Enter" /><a class="btn" href="rust://bookmarks">전체</a></div></div>
<div class="layout"><div class="side">{side}</div><div class="main"><h2 class="ptitle">{heading}</h2>{body}</div></div>
</body></html>"#,
        total = bookmarks.total_count(),
        source = html_escape(&bookmarks.source_label),
        search_val = html_escape(query.as_deref().unwrap_or("")),
        heading = html_escape(&heading),
    );
    platform_icons(h)
}

fn render_sidebar_folders(
    nodes: &[BookmarkNode],
    key_prefix: &str,
    depth: usize,
    active_key: &str,
    out: &mut String,
) {
    if depth > 4 {
        return;
    }
    let pad = 14 + depth * 14;
    for (i, node) in nodes.iter().enumerate() {
        if let BookmarkNode::Folder(folder) = node {
            let key = format!("{key_prefix}-{i}");
            let cls = if active_key == key { "sb active" } else { "sb" };
            let _ = write!(
                out,
                r#"<a class="{cls}" style="padding-left:{pad}px" href="rust://bookmarks?folder={key}"><span><span class="ico f">&#xE8B7;</span> {name}</span><span class="sb-n">{}</span></a>"#,
                count_urls(&folder.children),
                name = html_escape(&truncate_chars(&folder.title, 18)),
            );
            render_sidebar_folders(&folder.children, &key, depth + 1, active_key, out);
        }
    }
}

fn append_manager_url_row(out: &mut String, item: &BookmarkItem) {
    let _ = write!(
        out,
        r#"<a class="row" href="{url}">{fav}<span>{title}</span><span class="rurl">{url_short}</span></a>"#,
        url = html_escape(&item.url),
        fav = favicon_html(&item.title, &item.url),
        title = html_escape(&truncate_chars(&item.title, 56)),
        url_short = html_escape(&truncate_chars(&item.url, 56)),
    );
}

fn kind_prefix(kind: BookmarkRootKind) -> &'static str {
    match kind {
        BookmarkRootKind::Bar => "bar",
        BookmarkRootKind::Other => "other",
        BookmarkRootKind::Custom => "custom",
    }
}

fn parse_folder_spec(spec: &str) -> (BookmarkRootKind, Vec<usize>) {
    let mut parts = spec.split('-');
    let kind = match parts.next() {
        Some("other") => BookmarkRootKind::Other,
        Some("custom") => BookmarkRootKind::Custom,
        _ => BookmarkRootKind::Bar,
    };
    let path = parts.filter_map(|p| p.parse::<usize>().ok()).collect();
    (kind, path)
}

fn parse_query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    let query = query.split('#').next().unwrap_or(query);
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

pub fn render_error_page(url: &str, message: &str) -> String {
    let mut h = String::with_capacity(4096);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>오류</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(
        r#"body { background: var(--surface-2); padding: 96px 24px; }
.box { max-width: 600px; margin: 0 auto; }
.ic { font-size: 40px; color: var(--text-3); margin-bottom: 18px; }
h1 { font-size: 22px; font-weight: 600; margin-bottom: 12px; }
p { color: var(--text-2); font-size: 14px; line-height: 1.6; margin-bottom: 8px; word-break: break-all; }
.btn { display: inline-block; margin-top: 18px; padding: 8px 20px; border-radius: 18px; background: var(--accent); color: var(--surface); text-decoration: none; font-size: 13px; font-weight: 600; }
"#,
    );
    let _ = write!(
        h,
        r#"</style></head><body><div class="box"><div class="ic ico">&#xE783;</div><h1>페이지를 열 수 없습니다</h1><p>{message}</p><p>{url}</p><a class="btn" href="{url}">다시 시도</a></div></body></html>"#,
        url = html_escape(url),
        message = html_escape(message),
    );
    platform_icons(h)
}

fn short_host(url: &str) -> String {
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    truncate_chars(host.strip_prefix("www.").unwrap_or(host), 18)
}

fn monogram_badge(title: &str, url: &str) -> (char, &'static str) {
    const PALETTE: &[&str] = &[
        "#1a73e8", "#188038", "#e37400", "#9334e6", "#d01884", "#007b83", "#d93025", "#4e5bd6",
    ];
    let host = short_host(url);
    let source = if !host.is_empty() { host.as_str() } else { title };
    let ch = source
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('•');
    let hash: usize = url.bytes().fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
    (ch, PALETTE[hash % PALETTE.len()])
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
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
            working_set_mb: 42.4,
            blocked_count: 1500,
            filter_enabled: true,
            status_note: None,
            maximized: true,
            url_override: Some("typed"),
            parked_tab: None,
        });
        assert!(html.contains(r#"value="typed""#));
        assert!(html.contains("T&amp;T"));
        if cfg!(target_os = "windows") {
            assert!(html.contains("&#xE923;")); // restore glyph when maximized
        }
        assert!(html.contains(">1k<"));
        assert!(html.contains("42 MB"));
    }
}
