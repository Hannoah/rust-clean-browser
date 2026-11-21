//! Find in Page (`Ctrl+F`): live in-page search for Blitz documents (using Parley
//! inline layout text + `set_text_selection` + smooth `scroll_into_view`) and JS-mode WebViews.

use super::BrowserApp;
use super::popup::PopupKind;
use crate::chrome_ui::{FIND_BAR_HEIGHT, FIND_BAR_WIDTH, render_find_bar_html};
use blitz_dom::{NodeId, ScrollBehavior, ScrollLogicalPosition};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;

const KEY: &str = "find";

#[derive(Debug, Clone, Copy)]
pub(super) struct FindMatch {
    pub node_id: NodeId,
    pub start: usize,
    pub end: usize,
}

impl BrowserApp {
    pub(super) fn open_find_bar(&mut self, event_loop: &dyn ActiveEventLoop) {
        if let Some(popup) = &mut self.popup
            && matches!(popup.kind, PopupKind::FindBar { .. })
        {
            if let Some(id) = popup.doc.get_element_by_id("find-input") {
                popup.doc.set_focus_to(id);
            }
            popup.window.focus_window();
            return;
        }
        let scale = self.hidpi_scale.max(1.0) as f64;
        let win_w = (self.win_size.width as f64 / scale).max(FIND_BAR_WIDTH as f64 + 16.0);
        let x = (win_w - FIND_BAR_WIDTH as f64 - 14.0).max(8.0);
        let y = self.chrome_height() as f64 + 8.0;

        let kind = PopupKind::FindBar {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        };
        self.show_popup(event_loop, KEY.to_string(), kind, x, y, FIND_BAR_WIDTH, true);
        if let Some(popup) = &mut self.popup {
            if let Some(id) = popup.doc.get_element_by_id("find-input") {
                popup.doc.set_focus_to(id);
            }
            popup.window.request_redraw();
        }
    }

    pub(super) fn find_bar_popup_html(&self, query: &str, matches: &[FindMatch], current: usize) -> (String, u32) {
        let (label, no_match) = find_count_status(query, matches.len(), current, self.active_webview.is_some());
        (render_find_bar_html(query, &label, no_match), FIND_BAR_HEIGHT)
    }

    pub(super) fn update_find_from_input(&mut self) {
        let Some(popup) = &self.popup else {
            return;
        };
        if !matches!(popup.kind, PopupKind::FindBar { .. }) {
            return;
        }
        let Some(input_id) = popup.doc.get_element_by_id("find-input") else {
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

        let changed = match &popup.kind {
            PopupKind::FindBar { query, .. } => query != &text,
            _ => false,
        };
        if !changed {
            return;
        }

        if let Some(wv) = &self.active_webview {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                let _ = wv.evaluate_script("window.getSelection()?.removeAllRanges();");
            } else {
                let js_q = serde_json::to_string(trimmed).unwrap_or_else(|_| "\"\"".to_string());
                let _ = wv.evaluate_script(&format!("window.find({js_q}, false, false, true);"));
            }
            if let Some(popup) = &mut self.popup {
                if let PopupKind::FindBar {
                    query,
                    matches,
                    current,
                } = &mut popup.kind
                {
                    *query = text.clone();
                    matches.clear();
                    *current = 0;
                }
                self.refresh_find_counter(&text, 0, 0, true);
            }
            return;
        }

        if self.active_tab >= self.tabs.len() {
            return;
        }
        let t = self.anim_time();
        let found = self.tabs[self.active_tab]
            .doc
            .as_mut()
            .map(|doc| {
                doc.resolve(t);
                collect_doc_matches(doc, &text)
            })
            .unwrap_or_default();

        let count = found.len();
        if let Some(popup) = &mut self.popup
            && let PopupKind::FindBar {
                query,
                matches,
                current,
            } = &mut popup.kind
        {
            *query = text.clone();
            *matches = found;
            *current = 0;
        }
        self.apply_active_find_match();
        self.refresh_find_counter(&text, count, 0, false);
    }

    pub(super) fn step_find_match(&mut self, forward: bool) {
        let (query, len, next_idx) = match self.popup.as_mut().map(|p| &mut p.kind) {
            Some(PopupKind::FindBar {
                query,
                matches,
                current,
            }) => {
                if self.active_webview.is_some() {
                    let trimmed = query.trim();
                    if !trimmed.is_empty()
                        && let Some(wv) = &self.active_webview
                    {
                        let js_q = serde_json::to_string(trimmed).unwrap_or_else(|_| "\"\"".to_string());
                        let back = if forward { "false" } else { "true" };
                        let _ = wv.evaluate_script(&format!("window.find({js_q}, false, {back}, true);"));
                    }
                    return;
                }
                if matches.is_empty() {
                    return;
                }
                *current = if forward {
                    (*current + 1) % matches.len()
                } else {
                    (*current + matches.len() - 1) % matches.len()
                };
                (query.clone(), matches.len(), *current)
            }
            _ => return,
        };
        self.apply_active_find_match();
        self.refresh_find_counter(&query, len, next_idx, false);
    }

    pub(super) fn find_key(&mut self, code: KeyCode) -> bool {
        if !matches!(self.popup.as_ref().map(|p| &p.kind), Some(PopupKind::FindBar { .. })) {
            return false;
        }
        match code {
            KeyCode::Escape => {
                self.close_find_bar();
                true
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let shift = self.keyboard_modifiers.state().shift_key();
                self.step_find_match(!shift);
                true
            }
            KeyCode::F3 => {
                let shift = self.keyboard_modifiers.state().shift_key();
                self.step_find_match(!shift);
                true
            }
            _ => false,
        }
    }

    pub(super) fn close_find_bar(&mut self) {
        if let Some(wv) = &self.active_webview {
            let _ = wv.evaluate_script("window.getSelection()?.removeAllRanges();");
        }
        if self.active_tab < self.tabs.len()
            && let Some(doc) = &mut self.tabs[self.active_tab].doc
        {
            doc.clear_text_selection();
        }
        self.close_popup();
        self.request_redraw();
    }

    /// Drops stale `NodeId` matches before the active tab's document is unloaded or replaced.
    pub(super) fn clear_find_matches(&mut self) {
        let query = match self.popup.as_mut().map(|p| &mut p.kind) {
            Some(PopupKind::FindBar {
                query,
                matches,
                current,
            }) => {
                matches.clear();
                *current = 0;
                query.clone()
            }
            _ => return,
        };
        let is_wv = self.active_webview.is_some();
        self.refresh_find_counter(&query, 0, 0, is_wv);
    }

    /// Recomputes `FindBar` matches against the active tab's current document (or WebView)
    /// so `NodeId`s never point into a replaced/dropped `HtmlDocument`.
    pub(super) fn sync_find_bar_after_doc_change(&mut self) {
        let query = match self.popup.as_ref().map(|p| &p.kind) {
            Some(PopupKind::FindBar { query, .. }) => query.clone(),
            _ => return,
        };
        if self.active_webview.is_some() {
            self.clear_find_matches();
            return;
        }
        let anim_time = self.anim_time();
        let found = if self.active_tab < self.tabs.len() {
            if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                doc.resolve(anim_time);
                collect_doc_matches(doc, &query)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let count = found.len();
        if let Some(popup) = &mut self.popup
            && let PopupKind::FindBar { matches, current, .. } = &mut popup.kind
        {
            *matches = found;
            *current = 0;
        }
        self.apply_active_find_match();
        self.refresh_find_counter(&query, count, 0, false);
    }

    fn apply_active_find_match(&mut self) {
        let target = match self.popup.as_ref().map(|p| &p.kind) {
            Some(PopupKind::FindBar { matches, current, .. }) => matches.get(*current).copied(),
            _ => None,
        };
        if self.active_tab >= self.tabs.len() {
            return;
        }
        if let Some(doc) = &mut self.tabs[self.active_tab].doc {
            if let Some(m) = target {
                doc.set_text_selection(m.node_id, m.start, m.node_id, m.end);
                doc.scroll_into_view(
                    m.node_id,
                    ScrollBehavior::Smooth,
                    ScrollLogicalPosition::Center,
                    ScrollLogicalPosition::Nearest,
                );
            } else {
                doc.clear_text_selection();
            }
        }
        self.request_redraw();
    }

    fn refresh_find_counter(&mut self, query: &str, total: usize, current: usize, is_webview: bool) {
        let (label, no_match) = find_count_status(query, total, current, is_webview);
        let Some(popup) = &mut self.popup else {
            return;
        };
        if let Some(cnt_id) = popup.doc.get_element_by_id("find-count") {
            let class_val = if no_match { "num none" } else { "num" };
            popup
                .doc
                .mutate()
                .set_attribute(cnt_id, blitz_dom::qual_name!("class"), class_val);
            popup
                .doc
                .mutate()
                .set_inner_html(cnt_id, &crate::cleaner::html_escape(&label));
        }
        popup.window.request_redraw();
    }
}

fn find_count_status(query: &str, total: usize, current: usize, is_webview: bool) -> (String, bool) {
    if query.trim().is_empty() {
        return ("0 / 0".to_string(), false);
    }
    if is_webview {
        return ("JS".to_string(), false);
    }
    if total == 0 {
        ("0 / 0".to_string(), true)
    } else {
        (format!("{} / {}", current + 1, total), false)
    }
}

fn collect_doc_matches(doc: &blitz_html::HtmlDocument, query: &str) -> Vec<FindMatch> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(i32, i32, FindMatch)> = Vec::new();
    for (node_id, node) in doc.tree().iter() {
        let Some(el) = node.element_data() else {
            continue;
        };
        let Some(inline) = el.inline_layout_data.as_ref() else {
            continue;
        };
        let ranges = find_ci_ranges(&inline.text, &q);
        if ranges.is_empty() {
            continue;
        }
        let pos = node.absolute_position(0.0, 0.0);
        let y_key = (pos.y * 10.0) as i32;
        let x_key = (pos.x * 10.0) as i32;
        for (start, end) in ranges {
            hits.push((y_key, x_key, FindMatch { node_id, start, end }));
        }
    }
    hits.sort_by_key(|(y, x, m)| (*y, *x, m.start));
    hits.into_iter().map(|(_, _, m)| m).collect()
}

/// Finds all case-insensitive byte ranges `(start, end)` in `haystack` where `needle_lower` matches.
fn find_ci_ranges(haystack: &str, needle_lower: &str) -> Vec<(usize, usize)> {
    if needle_lower.is_empty() || haystack.is_empty() {
        return Vec::new();
    }
    let lower = haystack.to_lowercase();
    if lower.len() == haystack.len() {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        while let Some(rel) = lower[cursor..].find(needle_lower) {
            let start = cursor + rel;
            let end = start + needle_lower.len();
            if haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
                out.push((start, end));
            }
            cursor = end.max(cursor + 1);
        }
        return out;
    }

    // Unicode fallback when `to_lowercase()` changes byte length.
    let needle_chars: Vec<char> = needle_lower.chars().collect();
    let char_starts: Vec<(usize, char)> = haystack
        .char_indices()
        .flat_map(|(b, ch)| ch.to_lowercase().map(move |lc| (b, lc)))
        .collect();
    let mut out = Vec::new();
    if char_starts.len() < needle_chars.len() {
        return out;
    }
    let mut i = 0;
    while i + needle_chars.len() <= char_starts.len() {
        let matches = char_starts[i..i + needle_chars.len()]
            .iter()
            .zip(&needle_chars)
            .all(|((_, a), b)| a == b);
        if matches {
            let start = char_starts[i].0;
            let end = if i + needle_chars.len() < char_starts.len() {
                char_starts[i + needle_chars.len()].0
            } else {
                haystack.len()
            };
            if start < end && haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
                out.push((start, end));
            }
            i += needle_chars.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::DocumentConfig;
    use blitz_traits::shell::{ColorScheme, Viewport};

    #[test]
    fn finds_matches_in_blitz_document_and_scrolls() {
        let mut doc = blitz_html::HtmlDocument::from_html(
            "<html><body><h1>Rust Browser</h1><p>Fast rust engine and RUST ui.</p></body></html>",
            DocumentConfig {
                viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        let hits = collect_doc_matches(&doc, "rust");
        assert_eq!(hits.len(), 3);
    }
}
