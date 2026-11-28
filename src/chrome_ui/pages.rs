use super::{
    THEME_CSS, favicon_html, format_relative_time, monogram_badge, short_host, strip_url_scheme, truncate_chars,
};
use crate::bookmarks::{BookmarkItem, BookmarkNode, BookmarkRootKind, BookmarkStore, count_urls, parse_folder_spec};
use crate::cleaner::html_escape;
use crate::history::{History, HistoryItem};
use crate::icons::{Icon, brand_emblem_svg, error_emblem_svg, svg};
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const WELCOME_CSS: &str = r#"
body { background: var(--surface-2); padding: 72px 24px; }
.wcard {
  max-width: 560px; margin: 0 auto; padding: 36px 34px; text-align: center;
  border-radius: 18px; background: var(--surface); border: 1px solid var(--border);
  box-shadow: var(--shadow-md);
}
.wemblem { display: flex; justify-content: center; margin-bottom: 16px; }
.wcard h1 { font-size: 22px; font-weight: 700; letter-spacing: -0.025em; margin-bottom: 10px; }
.wcard p { color: var(--text-2); font-size: 13.5px; line-height: 1.65; margin-bottom: 22px; }
.wsrcs { display: flex; flex-direction: column; gap: 8px; margin-bottom: 14px; }
.wbtn {
  display: flex; align-items: center; justify-content: center; gap: 8px;
  min-height: 40px; padding: 0 18px; border-radius: 999px;
  font-size: 13px; font-weight: 600; text-decoration: none;
  border: 1px solid var(--border); color: var(--text); background: var(--surface);
}
.wbtn:hover { background: var(--hover); border-color: var(--border-strong); }
.wbtn.primary { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); }
.wbtn.primary:hover { opacity: 0.88; }
.wskip { margin-top: 4px; font-size: 12px; color: var(--text-3); }
.wskip a { color: var(--text-2); }
"#;

/// First-run welcome (`rust://welcome`): offers importing bookmarks from the
/// detected browsers before the user starts browsing.
pub fn render_welcome_page(sources: &[String], dark_mode: bool) -> String {
    let mut buttons = String::new();
    for (i, label) in sources.iter().enumerate() {
        let _ = write!(
            buttons,
            r#"<a class="wbtn primary" href="rust://welcome?import={i}">{ico}<span>{lbl} 북마크 가져오기</span></a>"#,
            ico = svg(Icon::Bookmark, 14),
            lbl = html_escape(&truncate_chars(label, 40)),
        );
    }
    let mut h = String::with_capacity(4096);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>환영합니다</title><style>");
    h.push_str(THEME_CSS);
    h.push_str(WELCOME_CSS);
    let _ = write!(
        h,
        r#"</style></head><body><div class="wcard">
  <div class="wemblem">{emblem}</div>
  <h1>빠르고 가벼운 브라우저에 오신 것을 환영합니다</h1>
  <p>다른 브라우저의 북마크를 가져올까요? 지금 하지 않아도 나중에<br/>명령 팔레트에서 “Chrome 북마크 다시 동기화”로 가져올 수 있습니다.</p>
  <div class="wsrcs">{buttons}</div>
  <div class="wskip"><a href="rust://welcome?skip=1">건너뛰고 시작하기 →</a></div>
</div></body></html>"#,
        emblem = brand_emblem_svg(dark_mode),
    );
    h
}

// ── Built-in Page: Home Bento Dashboard (`rust://home`) ──

pub(super) const HOME_CSS: &str = r#"
body {
  background: var(--surface-2);
  padding: 56px 28px 48px;
}
.wrap { max-width: 820px; margin: 0 auto; }
.wrap > * { animation: rb-rise 140ms var(--ease-out) both; }
.wrap > *:nth-child(2) { animation-delay: 15ms; }
.wrap > *:nth-child(3) { animation-delay: 30ms; }
.wrap > *:nth-child(4) { animation-delay: 45ms; }
.wrap > *:nth-child(5) { animation-delay: 60ms; }
.wrap > *:nth-child(6) { animation-delay: 75ms; }
.tile, .bcard { transition: background 60ms ease, border-color 60ms ease, transform 80ms var(--ease-out); }
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
  <span class="key-item"><kbd>{mod_k}T</kbd> 새 탭</span>
  <span class="key-item"><kbd>{mod_k}K</kbd> 명령 팔레트</span>
  <span class="key-item"><kbd>{mod_k}F</kbd> 찾기</span>
  <span class="key-item"><kbd>{alt_k}R</kbd> 리더 모드</span>
  <span class="key-item"><kbd>{mod_k}Shift+I</kbd> AI</span>
</div>
</div></body></html>"#,
        mod_k = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" },
        alt_k = if cfg!(target_os = "macos") { "⌥" } else { "Alt+" },
        emblem = brand_emblem_svg(dark_mode),
        search_ico = svg(Icon::Search, 18),
        cpu_ico = svg(Icon::Cpu, 16),
        hist_ico = svg(Icon::History, 16),
        bm_count = bookmarks.total_count(),
        hist_count = history.len(),
    );
    h
}

// ── Built-in Page: Bookmark Manager (`rust://bookmarks`) ──

pub(super) const BOOKMARKS_CSS: &str = r#"
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

pub(super) fn canonical_folder_key(kind: BookmarkRootKind, path: &[usize]) -> String {
    let mut s = kind_prefix(kind).to_string();
    for &idx in path {
        let _ = write!(s, "-{idx}");
    }
    s
}

/// Chrome-style 2-column Bookmark Manager (`rust://bookmarks`, `?folder=...`, `?q=...`).
pub fn render_bookmarks_page(bookmarks: &BookmarkStore, current_url: &str, drop_target_folder: Option<&str>) -> String {
    let query = parse_query_param(current_url, "q");
    let folder_param = parse_query_param(current_url, "folder");

    let active_key = if query.is_some() {
        "__search__".to_string()
    } else if let Some(f) = folder_param {
        let (kind, path) = parse_folder_spec(&f);
        canonical_folder_key(kind, &path)
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
        let canon_key = canonical_folder_key(kind, &path);
        let (title, nodes) = bookmarks
            .resolve_folder(kind, &path)
            .unwrap_or_else(|| ("북마크바".to_string(), (&bookmarks.bar[..]).into()));
        for (i, node) in nodes.iter().enumerate() {
            match node {
                BookmarkNode::Folder(f) => {
                    let next_key = format!("{canon_key}-{i}");
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
                    let item_key = format!("{canon_key}-{i}");
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

pub(super) fn render_sidebar_folders(
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

pub(super) fn append_manager_url_row(out: &mut String, item: &BookmarkItem, src_spec: &str) {
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

pub(super) const HISTORY_CSS: &str = r#"
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

pub(super) fn kind_prefix(kind: BookmarkRootKind) -> &'static str {
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
pub(super) fn highlight_escaped(text: &str, query: &str) -> String {
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
