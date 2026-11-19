//! Popup windows: bookmark folder popups, address-bar suggestions, Command Palette (`Ctrl+K`),
//! and Find-in-Page bar (`Ctrl+F`). Each is a borderless always-on-top window so it renders
//! cleanly above both Blitz documents and JS-mode WebViews.

use super::BrowserApp;
use super::bus::{BrowserShellProvider, PopupEventHandler};
use super::find::FindMatch;
use super::helpers::{mouse_button, plain_coords, pointer_event, popup_window_style, primary_modifier, wheel_delta};
use crate::bookmarks::{BookmarkNode, BookmarkRootKind};
use crate::chrome_ui::{
    POPUP_WIDTH, PaletteEntry, estimate_popup_height, render_bookmark_popup_html,
    render_suggest_popup_html, suggest_popup_height,
};
use crate::events::{winit_ime_to_blitz, winit_key_event_to_blitz};
use crate::history::Suggestion;
use anyrender::WindowRenderer;
use anyrender_vello_cpu::VelloCpuWindowRenderer;
use blitz_dom::{Document, EventDriver};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::events::{
    BlitzWheelEvent, MouseEventButtons, PointerCoords, PointerDetails, UiEvent,
};
use blitz_traits::shell::{ShellProvider, Viewport};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

pub(super) enum PopupKind {
    Bookmarks {
        root: BookmarkRootKind,
        path: Vec<usize>,
        base_depth: usize,
        root_title: String,
        index_offset: usize,
        selected: Option<usize>,
    },
    /// Address-bar suggestions; `selected` is what Enter opens.
    Suggest {
        rows: Vec<Suggestion>,
        selected: usize,
        query: String,
    },
    /// Searchable launcher for open tabs, commands, bookmarks, and history (`Ctrl+K`).
    CommandPalette {
        query: String,
        items: Vec<PaletteEntry>,
        selected: usize,
    },
    /// Floating Find-in-Page bar (`Ctrl+F`).
    FindBar {
        query: String,
        matches: Vec<FindMatch>,
        current: usize,
    },
}

pub(super) struct Popup {
    pub(super) key: String,
    pub(super) kind: PopupKind,
    /// Logical width.
    width: u32,
    pub(super) window: Arc<dyn Window>,
    renderer: VelloCpuWindowRenderer,
    pub(super) doc: HtmlDocument,
    win_size: PhysicalSize<u32>,
    hidpi_scale: f32,
    pointer_pos: PhysicalPosition<f64>,
    buttons: MouseEventButtons,
}

impl Popup {
    fn coords(&self, pos: PhysicalPosition<f64>) -> PointerCoords {
        let logical: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        let scroll = self.doc.viewport_scroll();
        plain_coords(logical.x, logical.y, scroll.x as f32, scroll.y as f32)
    }
}

/// Folder view data: (title, nodes, can_go_back, index_offset).
type PopupView<'a> = (String, Cow<'a, [BookmarkNode]>, bool, usize);

impl BrowserApp {
    pub(super) fn popup_owns(&self, window_id: WindowId) -> bool {
        self.popup.as_ref().is_some_and(|p| p.window.id() == window_id)
    }

    pub(super) fn has_find_bar(&self) -> bool {
        matches!(self.popup.as_ref().map(|p| &p.kind), Some(PopupKind::FindBar { .. }))
    }

    pub(super) fn close_popup(&mut self) {
        if let Some(mut popup) = self.popup.take() {
            let had_focus = matches!(
                popup.kind,
                PopupKind::Bookmarks { .. } | PopupKind::CommandPalette { .. } | PopupKind::FindBar { .. }
            );
            if matches!(popup.kind, PopupKind::FindBar { .. }) {
                if let Some(wv) = &self.active_webview {
                    let _ = wv.evaluate_script("window.getSelection()?.removeAllRanges();");
                }
                if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.clear_text_selection();
                }
            }
            self.recently_closed_popup = Some((popup.key.clone(), Instant::now()));
            popup.renderer.suspend();
            drop(popup);
            if had_focus {
                if let Some(win) = &self.window {
                    super::helpers::focus_main_window(&**win, false);
                }
            }
        }
    }

    fn build_popup_view_data(
        &self,
        root: BookmarkRootKind,
        path: &[usize],
        base_depth: usize,
        index_offset: usize,
        root_title: &str,
    ) -> Option<PopupView<'_>> {
        let (folder_title, nodes) = self.bookmarks.resolve_folder(root, path)?;
        let at_base = path.len() <= base_depth;
        if at_base && index_offset > 0 {
            let sliced = match nodes {
                Cow::Borrowed(n) => Cow::Borrowed(&n[index_offset.min(n.len())..]),
                Cow::Owned(v) => Cow::Owned(v.into_iter().skip(index_offset).collect()),
            };
            Some((root_title.to_string(), sliced, false, index_offset))
        } else {
            let title = if path.is_empty() { root_title.to_string() } else { folder_title };
            Some((title, nodes, !at_base, 0))
        }
    }

    fn popup_doc(&self, html: &str, window: &Arc<dyn Window>, w: u32, h: u32, scale: f32) -> HtmlDocument {
        let shell: Arc<dyn ShellProvider> = Arc::new(BrowserShellProvider {
            window: Arc::clone(window),
        });
        let vp = Viewport::new(w, h, scale, self.color_scheme);
        HtmlDocument::from_html(html, self.doc_config(vp, Some(shell)))
    }

    /// HTML and logical height for a popup's current content.
    fn popup_content(&self, kind: &PopupKind) -> Option<(String, u32)> {
        match kind {
            PopupKind::Bookmarks { root, path, base_depth, root_title, index_offset, selected } => {
                let (title, nodes, can_go_back, eff_offset) =
                    self.build_popup_view_data(*root, path, *base_depth, *index_offset, root_title)?;
                Some((
                    render_bookmark_popup_html(&title, &nodes, can_go_back, eff_offset, *selected),
                    estimate_popup_height(nodes.len(), can_go_back),
                ))
            }
            PopupKind::Suggest { rows, selected, query } => Some((
                render_suggest_popup_html(rows, *selected, query),
                suggest_popup_height(rows.len()),
            )),
            PopupKind::CommandPalette { query, items, selected } => {
                Some(self.render_palette_initial(query, items, *selected))
            }
            PopupKind::FindBar { query, matches, current } => {
                Some(self.find_bar_popup_html(query, matches, *current))
            }
        }
    }

    /// Opens a popup window at main-window logical (`x`, `y`). `focus`: take keyboard focus
    /// (the suggestion dropdown must leave it in the address bar).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_popup(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        key: String,
        kind: PopupKind,
        x: f64,
        y: f64,
        width: u32,
        focus: bool,
    ) {
        self.close_popup();
        let Some((html, logical_h)) = self.popup_content(&kind) else {
            return;
        };
        let Some(main_win) = &self.window else {
            return;
        };
        let scale = main_win.scale_factor().max(1.0);
        let surf_pos = super::helpers::client_origin_on_screen(&**main_win);
        let attrs = WindowAttributes::default()
            .with_title("Popup")
            .with_decorations(false)
            .with_resizable(false)
            .with_active(focus)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_surface_size(LogicalSize::new(width as f64, logical_h as f64))
            .with_position(PhysicalPosition::new(
                surf_pos.x + (x * scale).round() as i32,
                surf_pos.y + (y * scale).round() as i32,
            ));
        let Ok(popup_box) = event_loop.create_window(popup_window_style(attrs)) else {
            return;
        };
        let popup_win: Arc<dyn Window> = Arc::from(popup_box);
        if focus {
            super::bus::enable_window_ime(&*popup_win);
        }
        let popup_scale = popup_win.scale_factor() as f32;
        let popup_size = popup_win.surface_size();
        let w = popup_size.width.max(1);
        let h = popup_size.height.max(1);
        let doc = self.popup_doc(&html, &popup_win, w, h, popup_scale);

        let mut renderer = VelloCpuWindowRenderer::new();
        renderer.resume(Arc::new(Arc::clone(&popup_win)), w, h, || {});
        renderer.complete_resume();
        renderer.set_size(w, h);

        popup_win.request_redraw();
        if focus {
            popup_win.focus_window();
        }

        self.popup = Some(Popup {
            key,
            kind,
            width,
            window: popup_win,
            renderer,
            doc,
            win_size: popup_size,
            hidpi_scale: popup_scale,
            pointer_pos: PhysicalPosition::new(0.0, 0.0),
            buttons: MouseEventButtons::None,
        });
    }

    pub(super) fn resize_popup_height(&mut self, logical_h: u32) {
        let Some(popup) = &mut self.popup else {
            return;
        };
        let scale = popup.hidpi_scale;
        let phys_w = ((popup.width as f32) * scale).round().max(1.0) as u32;
        let phys_h = ((logical_h as f32) * scale).round().max(1.0) as u32;
        let _ = popup
            .window
            .request_surface_size(LogicalSize::new(popup.width as f64, logical_h as f64).into());
        popup.win_size = PhysicalSize::new(phys_w, phys_h);
        popup
            .doc
            .set_viewport(Viewport::new(phys_w, phys_h, scale, self.color_scheme));
        if popup.renderer.is_active() {
            popup.renderer.set_size(phys_w, phys_h);
        }
        popup.window.request_redraw();
    }

    pub(super) fn open_bookmark_popup(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        key: String,
        root: BookmarkRootKind,
        path: Vec<usize>,
        index_offset: usize,
        root_title: String,
    ) {
        // Clicking the currently open (or just focus-closed) bookmark chip toggles it shut.
        if self.popup.as_ref().is_some_and(|p| p.key == key) {
            self.close_popup();
            return;
        }
        if self
            .recently_closed_popup
            .as_ref()
            .is_some_and(|(k, t)| k == &key && t.elapsed().as_millis() < 250)
        {
            self.recently_closed_popup = None;
            return;
        }

        let base_depth = path.len();
        let Some((_, nodes, _, _)) =
            self.build_popup_view_data(root, &path, base_depth, index_offset, &root_title)
        else {
            return;
        };
        let icon_urls: Vec<String> = nodes
            .iter()
            .take(40)
            .filter_map(|n| match n {
                BookmarkNode::Url(item) => Some(item.url.clone()),
                BookmarkNode::Folder(_) => None,
            })
            .collect();
        drop(nodes);
        for url in &icon_urls {
            self.request_favicon(url);
        }
        self.pump_favicons();

        let scale = self.hidpi_scale.max(1.0) as f64;
        let win_w_logical = (self.win_size.width as f64 / scale).max(POPUP_WIDTH as f64);
        let x = (self.last_chrome_click_x_logical as f64 - 24.0)
            .clamp(8.0, (win_w_logical - POPUP_WIDTH as f64 - 8.0).max(8.0));
        let kind = PopupKind::Bookmarks {
            root,
            path,
            base_depth,
            root_title,
            index_offset,
            selected: None,
        };
        self.show_popup(event_loop, key, kind, x, self.chrome_height() as f64 - 1.0, POPUP_WIDTH, true);
    }

    /// Re-renders the open popup after its content changed (resizing it to fit).
    pub(super) fn refresh_popup(&mut self) {
        let Some(popup) = &self.popup else {
            return;
        };
        let Some((html, logical_h)) = self.popup_content(&popup.kind) else {
            return;
        };
        let scale = popup.hidpi_scale;
        let phys_w = ((popup.width as f32) * scale).round().max(1.0) as u32;
        let phys_h = ((logical_h as f32) * scale).round().max(1.0) as u32;
        let doc = self.popup_doc(&html, &popup.window, phys_w, phys_h, scale);

        let Some(popup) = &mut self.popup else {
            return;
        };
        let _ = popup
            .window
            .request_surface_size(LogicalSize::new(popup.width as f64, logical_h as f64).into());
        popup.win_size = PhysicalSize::new(phys_w, phys_h);
        popup.doc = doc;
        if popup.renderer.is_active() {
            popup.renderer.set_size(phys_w, phys_h);
        }
        popup.window.request_redraw();
    }

    pub(super) fn handle_popup_action(
        &mut self,
        action: &str,
        new_tab: bool,
        event_loop: &dyn ActiveEventLoop,
    ) {
        if let Some(rest) = action.strip_prefix("palette-exec:") {
            if let Ok(idx) = rest.parse::<usize>() {
                self.execute_palette_index(idx, event_loop);
            }
            return;
        }
        match action {
            "find-prev" => {
                self.step_find_match(false);
                return;
            }
            "find-next" => {
                self.step_find_match(true);
                return;
            }
            "find-close" => {
                self.close_find_bar();
                return;
            }
            _ => {}
        }

        if action == "popup-back" {
            let changed = match self.popup.as_mut().map(|p| &mut p.kind) {
                Some(PopupKind::Bookmarks { path, base_depth, selected, .. })
                    if path.len() > *base_depth =>
                {
                    path.pop();
                    *selected = None;
                    true
                }
                _ => false,
            };
            if changed {
                self.refresh_popup();
            }
        } else if let Some(rest) = action.strip_prefix("popup-enter:") {
            if let Ok(idx) = rest.parse::<usize>() {
                if let Some(PopupKind::Bookmarks { path, selected, .. }) =
                    self.popup.as_mut().map(|p| &mut p.kind)
                {
                    path.push(idx);
                    *selected = None;
                }
                self.refresh_popup();
            }
        } else {
            let url = match action.strip_prefix("popup-open:") {
                Some(url) => url.to_string(),
                None if action == "popup-manager" => "rust://bookmarks".to_string(),
                None => return,
            };
            self.close_popup();
            if new_tab {
                self.open_new_tab(&url);
            } else {
                self.navigate_current(&url);
            }
        }
    }

    fn dispatch_popup_ui(&mut self, event: UiEvent) {
        let Some(popup) = &mut self.popup else {
            return;
        };
        let handler = PopupEventHandler {
            bus: self.bus.clone(),
        };
        EventDriver::new(&mut popup.doc, handler).handle_ui_event(event);
    }

    fn redraw_popup(&mut self) {
        let anim_time = self.anim_time();
        let Some(popup) = &mut self.popup else {
            return;
        };
        if !popup.renderer.is_active() {
            return;
        }
        let scale = popup.hidpi_scale as f64;
        let w = popup.win_size.width.max(1);
        let h = popup.win_size.height.max(1);
        popup.doc.poll(None);
        popup.doc.resolve(anim_time);
        let doc_inner = &mut *popup.doc;
        popup.renderer.render(|scene| {
            paint_scene(scene, doc_inner, scale, w, h, 0, 0);
        });
    }

    fn bookmark_popup_key(&mut self, code: KeyCode) -> bool {
        let Some(PopupKind::Bookmarks {
            root,
            path,
            base_depth,
            root_title,
            index_offset,
            selected,
        }) = self.popup.as_ref().map(|p| &p.kind)
        else {
            return false;
        };
        let Some((_, nodes, can_go_back, eff_offset)) =
            self.build_popup_view_data(*root, path, *base_depth, *index_offset, root_title)
        else {
            return false;
        };
        let len = nodes.len();
        let cur_sel = *selected;
        let chosen_node = cur_sel.and_then(|i| nodes.get(i).cloned());
        drop(nodes);

        match code {
            KeyCode::Escape => {
                self.close_popup();
                true
            }
            KeyCode::Backspace if can_go_back => {
                if let Some(PopupKind::Bookmarks { path, selected, .. }) =
                    self.popup.as_mut().map(|p| &mut p.kind)
                {
                    path.pop();
                    *selected = None;
                }
                self.refresh_popup();
                true
            }
            KeyCode::ArrowDown if len > 0 => {
                let next = cur_sel.map_or(0, |i| (i + 1) % len);
                if let Some(PopupKind::Bookmarks { selected, .. }) =
                    self.popup.as_mut().map(|p| &mut p.kind)
                {
                    *selected = Some(next);
                }
                self.refresh_popup();
                true
            }
            KeyCode::ArrowUp if len > 0 => {
                let next = cur_sel.map_or(len - 1, |i| (i + len - 1) % len);
                if let Some(PopupKind::Bookmarks { selected, .. }) =
                    self.popup.as_mut().map(|p| &mut p.kind)
                {
                    *selected = Some(next);
                }
                self.refresh_popup();
                true
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                if let (Some(idx), Some(node)) = (cur_sel, chosen_node) {
                    match node {
                        BookmarkNode::Folder(_) => {
                            if let Some(PopupKind::Bookmarks { path, selected, .. }) =
                                self.popup.as_mut().map(|p| &mut p.kind)
                            {
                                path.push(eff_offset + idx);
                                *selected = None;
                            }
                            self.refresh_popup();
                        }
                        BookmarkNode::Url(item) => {
                            let new_tab = primary_modifier(self.keyboard_modifiers.state());
                            self.close_popup();
                            if new_tab {
                                self.open_new_tab(&item.url);
                            } else {
                                self.navigate_current(&item.url);
                            }
                        }
                    }
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn handle_popup_window_event(&mut self, event_loop: &dyn ActiveEventLoop, event: WindowEvent) {
        let mods = self.mods();
        match event {
            WindowEvent::CloseRequested => {
                if self.has_find_bar() {
                    self.close_find_bar();
                } else {
                    self.close_popup();
                }
            }
            WindowEvent::Focused(false) => {
                // FindBar stays open while the user reads/scrolls the page; other popups dismiss.
                if !self.has_find_bar() {
                    self.close_popup();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw_popup();
            }
            WindowEvent::SurfaceResized(size) => {
                if let Some(popup) = &mut self.popup {
                    popup.win_size = size;
                    let w = size.width.max(1);
                    let h = size.height.max(1);
                    popup.doc.set_viewport(Viewport::new(w, h, popup.hidpi_scale, self.color_scheme));
                    if popup.renderer.is_active() {
                        popup.renderer.set_size(w, h);
                    }
                    popup.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.keyboard_modifiers = m,
            WindowEvent::Ime(ime_ev) => {
                self.dispatch_popup_ui(UiEvent::Ime(winit_ime_to_blitz(ime_ev)));
                self.update_palette_from_input();
                self.update_find_from_input();
                if let Some(p) = &self.popup {
                    p.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        let ctrl = primary_modifier(self.keyboard_modifiers.state());
                        if ctrl && code == KeyCode::KeyK && matches!(self.popup.as_ref().map(|p| &p.kind), Some(PopupKind::CommandPalette { .. })) {
                            self.close_popup();
                            return;
                        }
                        if ctrl && code == KeyCode::KeyF && self.has_find_bar() {
                            if let Some(p) = &mut self.popup {
                                if let Some(id) = p.doc.get_element_by_id("find-input") {
                                    p.doc.set_focus_to(id);
                                }
                            }
                            return;
                        }
                        if self.palette_key(code, event_loop)
                            || self.find_key(code)
                            || self.bookmark_popup_key(code)
                        {
                            return;
                        }
                        if code == KeyCode::Escape {
                            self.close_popup();
                            return;
                        }
                    }
                }
                let is_text_popup = matches!(
                    self.popup.as_ref().map(|p| &p.kind),
                    Some(PopupKind::CommandPalette { .. } | PopupKind::FindBar { .. })
                );
                if is_text_popup {
                    let key_data = winit_key_event_to_blitz(&event, self.keyboard_modifiers.state());
                    self.dispatch_popup_ui(if event.state.is_pressed() {
                        UiEvent::KeyDown(key_data)
                    } else {
                        UiEvent::KeyUp(key_data)
                    });
                    self.update_palette_from_input();
                    self.update_find_from_input();
                    if let Some(p) = &self.popup {
                        p.window.request_redraw();
                    }
                }
            }
            WindowEvent::PointerMoved {
                position,
                primary,
                source,
                ..
            } => {
                let Some(popup) = &mut self.popup else {
                    return;
                };
                popup.pointer_pos = position;
                let ev = pointer_event(
                    crate::events::pointer_source_to_blitz(&source),
                    primary,
                    popup.coords(position),
                    Default::default(),
                    popup.buttons,
                    mods,
                    crate::events::pointer_source_to_blitz_details(&source),
                );
                self.dispatch_popup_ui(UiEvent::PointerMove(ev));
            }
            WindowEvent::PointerButton {
                button,
                state,
                primary,
                position,
                ..
            } => {
                let Some(popup) = &mut self.popup else {
                    return;
                };
                popup.pointer_pos = position;
                let btn = mouse_button(&button);
                match state {
                    ElementState::Pressed => popup.buttons |= btn.into(),
                    ElementState::Released => popup.buttons &= !MouseEventButtons::from(btn),
                }
                let pev = pointer_event(
                    crate::events::button_source_to_blitz(&button),
                    primary,
                    popup.coords(position),
                    btn,
                    popup.buttons,
                    mods,
                    PointerDetails::default(),
                );
                self.dispatch_popup_ui(match state {
                    ElementState::Pressed => UiEvent::PointerDown(pev),
                    ElementState::Released => UiEvent::PointerUp(pev),
                });
                self.process_app_events(event_loop);
                if let Some(p) = &self.popup {
                    p.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(popup) = &mut self.popup else {
                    return;
                };
                let coords = popup.coords(popup.pointer_pos);
                let buttons = popup.buttons;
                popup.doc.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
                    delta: wheel_delta(delta),
                    coords,
                    buttons,
                    mods,
                    element: Default::default(),
                }));
            }
            _ => {}
        }
    }
}
