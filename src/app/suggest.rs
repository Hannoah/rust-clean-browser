//! Address-bar autocomplete: a suggestion dropdown under the URL bar, fed by history and
//! bookmarks. Up/Down picks a row, Enter opens it, Esc closes it. When the best match starts
//! with what was typed it is preselected, like inline autocomplete in other browsers.

use super::BrowserApp;
use super::helpers::normalize_url_input;
use super::popup::PopupKind;
use crate::history::{SuggestKind, Suggestion, inline_match};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;

const MAX_MATCHES: usize = 6;
const POPUP_KEY: &str = "suggest";

impl BrowserApp {
    fn suggest_open(&self) -> bool {
        matches!(self.popup.as_ref().map(|p| &p.kind), Some(PopupKind::Suggest { .. }))
    }

    /// Call after the URL bar text may have changed (typing / IME).
    pub(super) fn update_suggestions(&mut self, event_loop: &dyn ActiveEventLoop) {
        if !self.url_bar_focused() {
            return;
        }
        let Some(text) = self.read_url_bar_text() else {
            return;
        };
        let query = text.trim().to_string();
        if query == self.suggest_query {
            return; // caret movement etc.
        }
        let grew = query.len() > self.suggest_query.len() && query.starts_with(self.suggest_query.as_str());
        self.suggest_query = query.clone();

        let mut matches = Vec::new();
        let q_lower = query.to_lowercase();
        for tab in &self.tabs {
            if tab.url.starts_with("http")
                && (tab.title.to_lowercase().contains(&q_lower)
                    || tab.url.to_lowercase().contains(&q_lower))
            {
                matches.push(Suggestion {
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    kind: SuggestKind::OpenTab,
                });
                if matches.len() >= 2 {
                    break;
                }
            }
        }
        for s in self.history.suggest(&self.bookmarks, &query, MAX_MATCHES) {
            if !matches.iter().any(|m| m.url == s.url) {
                matches.push(s);
            }
        }
        matches.truncate(MAX_MATCHES);
        let typed = Suggestion {
            url: normalize_url_input(&query),
            title: query.clone(),
            kind: SuggestKind::Typed,
        };
        matches.retain(|m| m.url != typed.url);
        if query.is_empty() || matches.is_empty() {
            if self.suggest_open() {
                self.close_popup();
            }
            return;
        }
        // Inline autocomplete only while typing forward (not after Backspace), like Chrome.
        let rows: Vec<Suggestion> = if grew && inline_match(&query, &matches[0].url) {
            let top = matches.remove(0);
            [top, typed].into_iter().chain(matches).collect()
        } else {
            std::iter::once(typed).chain(matches).collect()
        };

        if let Some(popup) = self.popup.as_mut().filter(|p| matches!(p.kind, PopupKind::Suggest { .. })) {
            popup.kind = PopupKind::Suggest { rows, selected: 0, query };
            self.refresh_popup();
            return;
        }
        // While an IME syllable is actively being composed (preedit), never create a new top-level
        // popup HWND because `CreateWindowExW` on Windows interrupts IMM32 composition.
        if self.url_bar_composing() {
            return;
        }
        let Some((x, y, width)) = self.url_bar_rect() else {
            return;
        };
        let kind = PopupKind::Suggest { rows, selected: 0, query };
        self.show_popup(event_loop, POPUP_KEY.to_string(), kind, x, y + 4.0, width, false);
    }

    /// `Alt+Enter` in the address bar: opens the highlighted suggestion (if open) in a new tab.
    pub(super) fn suggest_open_new_tab(&mut self) -> bool {
        let url = match self.popup.as_ref().map(|p| &p.kind) {
            Some(PopupKind::Suggest { rows, selected, .. }) => {
                rows.get(*selected).map(|r| r.url.clone())
            }
            _ => None,
        };
        let Some(url) = url else {
            return false;
        };
        self.close_popup();
        self.open_new_tab(&url);
        true
    }

    /// Up / Down / Enter / Esc while the dropdown is open. Returns true when consumed.
    pub(super) fn suggest_key(&mut self, code: KeyCode) -> bool {
        let Some(PopupKind::Suggest { rows, selected, .. }) = self.popup.as_mut().map(|p| &mut p.kind) else {
            return false;
        };
        match code {
            KeyCode::ArrowDown => *selected = (*selected + 1) % rows.len(),
            KeyCode::ArrowUp => *selected = (*selected + rows.len() - 1) % rows.len(),
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let Some(row) = rows.get(*selected) else {
                    return false;
                };
                let url = row.url.clone();
                let is_open_tab = row.kind == SuggestKind::OpenTab;
                self.close_popup();
                if is_open_tab {
                    if let Some(idx) = self.tabs.iter().position(|t| t.url == url) {
                        self.switch_tab(idx);
                        return true;
                    }
                }
                self.navigate_current(&url);
                return true;
            }
            KeyCode::Escape => {
                self.close_popup();
                return true;
            }
            _ => return false,
        }
        self.refresh_popup();
        true
    }

    /// Omnibox container's left, bottom and width in main-window logical px.
    fn url_bar_rect(&self) -> Option<(f64, f64, u32)> {
        let id = self
            .chrome_doc
            .get_element_by_id("omni-box")
            .or_else(|| self.chrome_doc.get_element_by_id("url-bar"))?;
        let node = self.chrome_doc.get_node(id)?;
        let pos = node.absolute_position(0.0, 0.0);
        let size = node.final_layout().size;
        Some((pos.x as f64, (pos.y + size.height) as f64, size.width.round().max(260.0) as u32))
    }
}
