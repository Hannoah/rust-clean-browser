//! Command Palette (`Ctrl+K` / `Ctrl+Shift+P`): unified searchable launcher for
//! open tabs, browser commands, theme/UI toggles, bookmarks, and history.

use super::BrowserApp;
use super::helpers::normalize_url_input;
use super::popup::PopupKind;
use crate::chrome_ui::{
    PALETTE_WIDTH, PaletteEntry, PaletteIcon, TITLEBAR_HEIGHT, palette_popup_height,
    render_command_palette_list_html, render_command_palette_shell_html,
};
use blitz_dom::ScrollBehavior;
use std::collections::HashSet;
use winit::dpi::LogicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;

const KEY: &str = "palette";

impl BrowserApp {
    pub(super) fn open_command_palette(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.popup.as_ref().is_some_and(|p| p.key == KEY) {
            self.close_popup();
            return;
        }
        let items = self.build_palette_items("");
        let height = palette_height_for(&items);
        let scale = self.hidpi_scale.max(1.0) as f64;
        let win_w = (self.win_size.width as f64 / scale).max(336.0);
        let width = PALETTE_WIDTH.min((win_w - 16.0).round() as u32).max(320);
        let x = ((win_w - width as f64) * 0.5).max(8.0);
        let y = TITLEBAR_HEIGHT as f64 + 12.0;

        let kind = PopupKind::CommandPalette {
            query: String::new(),
            items,
            selected: 0,
        };
        self.show_popup(event_loop, KEY.to_string(), kind, x, y, width, true);
        if let Some(popup) = &mut self.popup {
            if let Some(id) = popup.doc.get_element_by_id("palette-input") {
                popup.doc.set_focus_to(id);
            }
            let _ = popup
                .window
                .request_surface_size(LogicalSize::new(width as f64, height as f64).into());
            popup.window.request_redraw();
        }
    }

    pub(super) fn update_palette_from_input(&mut self) {
        let Some(popup) = &self.popup else {
            return;
        };
        if !matches!(popup.kind, PopupKind::CommandPalette { .. }) {
            return;
        }
        let Some(input_id) = popup.doc.get_element_by_id("palette-input") else {
            return;
        };
        let Some(text) = popup
            .doc
            .get_node(input_id)
            .and_then(|n| n.element_data())
            .and_then(|e| e.text_input_data())
            .map(|ti| ti.editor.raw_text().to_string())
        else {
            return;
        };

        let cur_query = match &popup.kind {
            PopupKind::CommandPalette { query, .. } => query,
            _ => return,
        };
        if &text == cur_query {
            return;
        }

        let items = self.build_palette_items(&text);
        let list_html = render_command_palette_list_html(&items, 0, &text);
        let logical_h = palette_height_for(&items);

        let Some(popup) = &mut self.popup else {
            return;
        };
        if let PopupKind::CommandPalette {
            query,
            items: slot,
            selected,
        } = &mut popup.kind
        {
            *query = text;
            *slot = items;
            *selected = 0;
        }
        if let Some(list_id) = popup.doc.get_element_by_id("palette-list") {
            popup.doc.mutate().set_inner_html(list_id, &list_html);
        }
        self.resize_popup_height(logical_h);
    }

    pub(super) fn palette_key(&mut self, code: KeyCode, event_loop: &dyn ActiveEventLoop) -> bool {
        let (len, cur) = match self.popup.as_ref().map(|p| &p.kind) {
            Some(PopupKind::CommandPalette { items, selected, .. }) => (items.len(), *selected),
            _ => return false,
        };
        match code {
            KeyCode::Escape => {
                self.close_popup();
                return true;
            }
            KeyCode::ArrowDown => {
                if len > 0 {
                    self.set_palette_selected((cur + 1) % len);
                }
                return true;
            }
            KeyCode::ArrowUp => {
                if len > 0 {
                    self.set_palette_selected((cur + len - 1) % len);
                }
                return true;
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                self.execute_palette_index(cur, event_loop);
                return true;
            }
            _ => false,
        }
    }

    fn set_palette_selected(&mut self, next_sel: usize) {
        let anim_time = self.anim_time();
        let Some(popup) = &mut self.popup else {
            return;
        };
        let list_html = if let PopupKind::CommandPalette {
            query,
            items,
            selected,
        } = &mut popup.kind
        {
            *selected = next_sel.min(items.len().saturating_sub(1));
            render_command_palette_list_html(items, *selected, query)
        } else {
            return;
        };
        if let Some(list_id) = popup.doc.get_element_by_id("palette-list") {
            popup.doc.mutate().set_inner_html(list_id, &list_html);
            popup.doc.resolve(anim_time);
            if let (Some(list_node), Some(sel_id)) = (
                popup.doc.get_node(list_id),
                popup.doc.get_element_by_id("palette-sel"),
            ) {
                let list_top = list_node.absolute_position(0.0, 0.0).y as f64;
                let list_h = list_node.final_layout().size.height as f64;
                let cur_scroll = list_node.scroll_offset().y;
                if let Some(sel_node) = popup.doc.get_node(sel_id) {
                    let sel_top = sel_node.absolute_position(0.0, 0.0).y as f64 - list_top + cur_scroll;
                    let sel_h = sel_node.final_layout().size.height as f64;
                    if sel_top < cur_scroll {
                        popup.doc.scroll_to(list_id, 0.0, (sel_top - 6.0).max(0.0), ScrollBehavior::Instant);
                    } else if sel_top + sel_h > cur_scroll + list_h {
                        popup.doc.scroll_to(
                            list_id,
                            0.0,
                            (sel_top + sel_h - list_h + 6.0).max(0.0),
                            ScrollBehavior::Instant,
                        );
                    }
                }
            }
        }
        popup.window.request_redraw();
    }

    pub(super) fn execute_palette_index(&mut self, idx: usize, event_loop: &dyn ActiveEventLoop) {
        let action = match self.popup.as_ref().map(|p| &p.kind) {
            Some(PopupKind::CommandPalette { items, .. }) => items.get(idx).map(|i| i.action.clone()),
            _ => None,
        };
        let Some(action) = action else {
            return;
        };
        self.close_popup();
        self.handle_chrome_action(&action, event_loop);
    }

    pub(super) fn render_palette_initial(&self, query: &str, items: &[PaletteEntry], selected: usize) -> (String, u32) {
        let list_html = render_command_palette_list_html(items, selected, query);
        let html = render_command_palette_shell_html(query, &list_html);
        (html, palette_height_for(items))
    }

    pub(super) fn build_palette_items(&self, raw_query: &str) -> Vec<PaletteEntry> {
        let q = raw_query.trim().to_lowercase();
        let mut out = Vec::new();
        let active = &self.tabs[self.active_tab];
        let is_mac = cfg!(target_os = "macos");

        // 1. Open tabs
        for (idx, tab) in self.tabs.iter().enumerate() {
            if !q.is_empty()
                && !tab.title.to_lowercase().contains(&q)
                && !tab.url.to_lowercase().contains(&q)
            {
                continue;
            }
            let status = if idx == self.active_tab {
                "현재 탭"
            } else if tab.js_mode {
                "JS 모드"
            } else if !tab.is_loaded() {
                "절전 (0 MB)"
            } else {
                "활성"
            };
            out.push(PaletteEntry {
                action: format!("tab:{idx}"),
                category: "열린 탭",
                title: tab.title.clone(),
                subtitle: format!("{status} · {}", tab.url),
                shortcut: "",
                icon_kind: PaletteIcon::Tab,
            });
        }

        // 2. Commands
        let commands: Vec<(String, &'static str, &'static str, &'static str, PaletteIcon, &'static str)> = vec![
            ("new-tab".into(), "새 탭 열기", "빈 대시보드 및 주소창 포커스", if is_mac { "⌘T" } else { "Ctrl+T" }, PaletteIcon::Command, "new tab"),
            ("open-find".into(), "페이지 내 찾기", "현재 페이지에서 텍스트 검색", if is_mac { "⌘F" } else { "Ctrl+F" }, PaletteIcon::Search, "find search"),
            ("toggle-reader".into(), if active.reader_mode { "리더 모드 끄기" } else { "리더 모드 켜기" }, "방해 요소 없는 읽기 화면으로 전환", "Alt+R", PaletteIcon::Book, "reader article"),
            ("translate".into(), if active.translated.is_some() { "원문 보기 (번역 끄기)" } else { "한영 번역 (자동 방향 감지)" }, "한글 페이지는 영어로, 그 외는 한국어로 · 본문이 Google로 전송됨", "Alt+T", PaletteIcon::Book, "translate 번역 영어 한국어 korean english"),
            ("toggle-js".into(), if active.js_mode { "순수 Rust Clean 모드로 전환" } else { "JS 호환 모드 (WebView2)로 전환" }, "현재 탭의 렌더링 엔진 전환", "", PaletteIcon::Zap, "js webview blitz clean"),
            ("toggle-filter".into(), if self.net_provider.is_filter_enabled() { "트래커 차단 끄기" } else { "트래커 차단 켜기" }, "광고·추적 스크립트 및 iframe 차단 전환", "", PaletteIcon::Shield, "adblock tracker shield"),
            ("toggle-https-only".into(), if self.prefs.https_only { "HTTPS-Only 끄기" } else { "HTTPS-Only 켜기" }, "http 주소를 https로 자동 업그레이드", "", PaletteIcon::Shield, "https tls secure upgrade"),
            ("toggle-bookmark".into(), if self.bookmarks.contains_url(&active.url) { "현재 페이지 북마크 해제" } else { "현재 페이지 북마크 추가" }, "개인 북마크에 저장/삭제", if is_mac { "⌘D" } else { "Ctrl+D" }, PaletteIcon::Bookmark, "bookmark star"),
            ("toggle-bm-bar".into(), if self.prefs.show_bookmarks_bar { "북마크 바 숨기기" } else { "북마크 바 표시하기" }, "상단 북마크 툴바 공간 전환", if is_mac { "⌘⇧B" } else { "Ctrl+Shift+B" }, PaletteIcon::Command, "bookmark bar toggle"),
            ("cite:ieee".into(), "출처 복사 · IEEE", "현재 페이지 인용문을 클립보드로 복사", "", PaletteIcon::Book, "cite citation 출처 인용 reference ieee"),
            ("cite:apa".into(), "출처 복사 · APA 7", "저자. (연도, 월 일). 제목. 사이트. URL", "", PaletteIcon::Book, "cite citation 출처 인용 reference apa"),
            ("cite:slide".into(), "출처 복사 · 발표자료", "출처: 사이트, \"제목\", URL (접속일)", "", PaletteIcon::Book, "cite citation 출처 인용 reference slide ppt 발표"),
            ("cite:mla".into(), "출처 복사 · MLA 9", "저자. \"제목.\" 사이트, 날짜, URL.", "", PaletteIcon::Book, "cite citation 출처 인용 reference mla"),
            ("cycle-theme".into(), "테마 모드 전환", "시스템 / 라이트 / 다크 순환", "", PaletteIcon::Theme, "theme dark light mode"),
            ("nav:rust://history".into(), "방문 기록 보기", "최근 방문한 웹 페이지 타임라인", if is_mac { "⌘H" } else { "Ctrl+H" }, PaletteIcon::History, "history timeline"),
            ("nav:rust://bookmarks".into(), "북마크 관리자 열기", "폴더별 북마크 탐색 및 검색", if is_mac { "⌘⇧O" } else { "Ctrl+Shift+O" }, PaletteIcon::Bookmark, "bookmarks manager"),
            ("sync-chrome".into(), "Chrome 북마크 다시 동기화", "로컬 Chrome 프로필 북마크 가져오기", "", PaletteIcon::Command, "chrome sync import"),
            ("trim-memory".into(), "메모리 즉시 회수 (Trim Working Set)", "OS에 유휴 힙 메모리 반환", "", PaletteIcon::Memory, "memory ram trim gc"),
            ("reopen-tab".into(), "닫은 탭 다시 열기", "방금 닫은 페이지 복원", if is_mac { "⌘⇧T" } else { "Ctrl+Shift+T" }, PaletteIcon::Tab, "reopen undo close"),
            ("close-current-tab".into(), "현재 탭 닫기", "현재 보고 있는 탭 닫기", if is_mac { "⌘W" } else { "Ctrl+W" }, PaletteIcon::Tab, "close tab"),
            ("zoom-reset".into(), "화면 배율 100% 초기화", "확대/축소 기본값으로 되돌리기", if is_mac { "⌘0" } else { "Ctrl+0" }, PaletteIcon::Command, "zoom reset"),
        ];

        for (action, title, subtitle, shortcut, icon_kind, keywords) in commands {
            if !q.is_empty()
                && !title.to_lowercase().contains(&q)
                && !subtitle.to_lowercase().contains(&q)
                && !keywords.contains(&q)
            {
                continue;
            }
            out.push(PaletteEntry {
                action,
                category: "빠른 명령",
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                shortcut,
                icon_kind,
            });
        }

        // 3. Bookmarks & History when typing
        if !q.is_empty() {
            let mut seen = HashSet::new();
            for bm in self.bookmarks.search(&q).into_iter().take(5) {
                if seen.insert(bm.url.clone()) {
                    out.push(PaletteEntry {
                        action: format!("nav:{}", bm.url),
                        category: "북마크",
                        title: bm.title,
                        subtitle: bm.url,
                        shortcut: "",
                        icon_kind: PaletteIcon::Bookmark,
                    });
                }
            }
            for hist in self.history.search_history(&q, 6) {
                if seen.insert(hist.url.clone()) {
                    let title = if hist.title.trim().is_empty() {
                        hist.url.clone()
                    } else {
                        hist.title
                    };
                    out.push(PaletteEntry {
                        action: format!("nav:{}", hist.url),
                        category: "방문 기록",
                        title,
                        subtitle: hist.url,
                        shortcut: "",
                        icon_kind: PaletteIcon::History,
                    });
                }
            }

            let target = normalize_url_input(raw_query);
            out.push(PaletteEntry {
                action: format!("nav:{target}"),
                category: "웹 이동",
                title: if target.starts_with("https://www.google.com/search?") {
                    format!("Google에서 \"{}\" 검색", raw_query.trim())
                } else {
                    format!("{target} 열기")
                },
                subtitle: target,
                shortcut: "Enter",
                icon_kind: PaletteIcon::Search,
            });
        }

        out
    }
}

fn palette_height_for(items: &[PaletteEntry]) -> u32 {
    let mut cats = 0usize;
    let mut prev = "";
    for it in items {
        if it.category != prev {
            cats += 1;
            prev = it.category;
        }
    }
    palette_popup_height(items.len(), cats)
}
