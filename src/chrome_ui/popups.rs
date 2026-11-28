use super::pages::highlight_escaped;
use super::{
    DOWNLOADS_POPUP_MAX_HEIGHT, DownloadItem, DownloadStatus, PALETTE_MAX_HEIGHT, POPUP_MAX_HEIGHT, THEME_CSS,
    favicon_html, format_relative_time, short_host, strip_url_scheme, truncate_chars,
};
use crate::bookmarks::BookmarkNode;
use crate::cleaner::html_escape;
use crate::history::{SuggestKind, Suggestion};
use crate::icons::{Icon, svg};
use std::fmt::Write;

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
