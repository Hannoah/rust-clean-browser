//! Main-window input: keyboard shortcuts, pointer routing (chrome vs. content), frameless
//! window resize / drag.

use super::{BookmarkDrag, BrowserApp, TabDrag};
use super::helpers::{
    RESIZE_EDGE, focus_main_window, is_primary_mouse_button_down, mouse_button, normalize_url_input,
    pointer_event, primary_modifier, resize_cursor, wheel_delta,
};
use crate::chrome_ui::TITLEBAR_HEIGHT;
use crate::events::{
    button_source_to_blitz, pointer_kind_to_blitz, pointer_source_to_blitz,
    pointer_source_to_blitz_details, theme_to_color_scheme, winit_ime_to_blitz,
    winit_key_event_to_blitz,
};
use anyrender::WindowRenderer;
use blitz_dom::{Document, LocalName};
use blitz_traits::events::{
    BlitzPointerId, BlitzWheelEvent, MouseEventButton, MouseEventButtons, PointerCoords,
    PointerDetails, UiEvent,
};
use std::time::Instant;
use winit::cursor::{Cursor, CursorIcon};
use winit::dpi::{LogicalPosition, PhysicalPosition};
use winit::event::{ButtonSource, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::ResizeDirection;

impl BrowserApp {
    pub(super) fn handle_main_window_event(&mut self, event_loop: &dyn ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.exit_app(event_loop),
            WindowEvent::Focused(false) => {
                self.buttons = MouseEventButtons::None;
                self.tab_drag = None;
                self.url_bar_select_all_on_click = None;
                if let Some(drag) = self.bm_drag.take() {
                    if drag.active {
                        if let Some(win) = &self.window {
                            win.set_cursor(Cursor::Icon(CursorIcon::Default));
                        }
                        self.rebuild_chrome();
                    }
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                self.os_color_scheme = theme_to_color_scheme(theme);
                self.apply_effective_color_scheme();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Moved(_) => {
                if self.popup.is_some() {
                    self.close_popup();
                }
            }
            WindowEvent::SurfaceResized(size) => {
                self.win_size = size;
                let maximized = self.window.as_ref().is_some_and(|win| win.is_maximized());
                if maximized != self.maximized {
                    self.maximized = maximized;
                    self.rebuild_chrome();
                }
                if self.renderer.is_active() {
                    self.renderer.set_size(size.width.max(1), size.height.max(1));
                }
                self.sync_viewports();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.hidpi_scale = scale_factor as f32;
                self.sync_viewports();
            }
            WindowEvent::ModifiersChanged(mods) => self.keyboard_modifiers = mods,
            WindowEvent::Ime(ime_ev) => {
                let ev = UiEvent::Ime(winit_ime_to_blitz(ime_ev));
                self.dispatch_focused(ev);
                self.update_suggestions(event_loop);
                self.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        if self.handle_shortcut(code, event_loop) {
                            return;
                        }
                    }
                }
                let key_data = winit_key_event_to_blitz(&event, self.keyboard_modifiers.state());
                self.dispatch_focused(if event.state.is_pressed() {
                    UiEvent::KeyDown(key_data)
                } else {
                    UiEvent::KeyUp(key_data)
                });
                self.process_app_events(event_loop);
                self.update_suggestions(event_loop);
                self.request_redraw();
            }
            WindowEvent::PointerLeft {
                position,
                primary,
                kind,
                ..
            } => {
                let id = pointer_kind_to_blitz(&kind);
                let pos = position.unwrap_or(self.pointer_pos);
                self.send_pointer_leave(id, primary, pos, true);
                self.send_pointer_leave(id, primary, pos, false);
                self.pointer_in_chrome = None;
                self.set_hover_link(None);
                self.request_redraw();
            }
            WindowEvent::PointerMoved {
                position,
                source,
                primary,
                ..
            } => {
                self.pointer_pos = position;
                if (self.tab_drag.is_some() || self.bm_drag.is_some()) && !is_primary_mouse_button_down() {
                    self.buttons &= !MouseEventButtons::from(MouseEventButton::Main);
                    self.tab_drag = None;
                    if let Some(drag) = self.bm_drag.take() {
                        if drag.active {
                            if let Some(win) = &self.window {
                                win.set_cursor(Cursor::Icon(CursorIcon::Default));
                            }
                            self.rebuild_chrome();
                        }
                    }
                }
                let resize = self.resize_direction_at(position);
                if resize != self.resize_hover {
                    self.resize_hover = resize;
                    if let Some(win) = &self.window {
                        win.set_cursor(Cursor::Icon(resize.map_or(CursorIcon::Default, resize_cursor)));
                    }
                }
                if resize.is_some() {
                    return;
                }
                if self.update_tab_drag(position) {
                    return;
                }
                if self.update_bm_drag(position) {
                    return;
                }
                if let Some(start_pos) = self.url_bar_select_all_on_click {
                    let dx = position.x - start_pos.x;
                    let dy = position.y - start_pos.y;
                    let threshold = 4.0 * (self.hidpi_scale as f64);
                    if (dx * dx + dy * dy) < threshold * threshold {
                        return;
                    }
                    // Intentional drag inside `#url-bar`: anchor selection at `start_pos` and let
                    // normal pointer-move drag selection proceed.
                    self.url_bar_select_all_on_click = None;
                    let id = pointer_source_to_blitz(&source);
                    let down_ev = pointer_event(
                        id,
                        primary,
                        self.coords_for(start_pos, true),
                        MouseEventButton::Main,
                        self.buttons,
                        self.mods(),
                        pointer_source_to_blitz_details(&source),
                    );
                    self.dispatch_to(true, UiEvent::PointerDown(down_ev));
                }
                let id = pointer_source_to_blitz(&source);
                let now_in_chrome = self.is_in_chrome(position);
                // Crossing the chrome/content boundary: the side we left gets a PointerLeave.
                if let Some(was_in_chrome) = self.pointer_in_chrome {
                    if was_in_chrome != now_in_chrome {
                        self.send_pointer_leave(id, primary, position, was_in_chrome);
                    }
                }
                self.pointer_in_chrome = Some(now_in_chrome);
                let ev = pointer_event(
                    id,
                    primary,
                    self.coords_for(position, now_in_chrome),
                    Default::default(),
                    self.buttons,
                    self.mods(),
                    pointer_source_to_blitz_details(&source),
                );
                // No unconditional redraw: Blitz requests one when hover/selection changes.
                self.dispatch_to(now_in_chrome, UiEvent::PointerMove(ev));
                self.update_link_hover(now_in_chrome);
            }
            WindowEvent::PointerButton {
                button,
                state,
                primary,
                position,
                ..
            } => {
                self.pointer_pos = position;
                if state == ElementState::Pressed {
                    match button {
                        ButtonSource::Mouse(MouseButton::Back) => {
                            self.history_step(false);
                            return;
                        }
                        ButtonSource::Mouse(MouseButton::Forward) => {
                            self.history_step(true);
                            return;
                        }
                        _ => {}
                    }
                }
                let btn = mouse_button(&button);

                // Frameless window: edge resize and title-bar drag / double-click maximize.
                if state == ElementState::Pressed && btn == MouseEventButton::Main && self.frame_press(position) {
                    return;
                }

                match state {
                    ElementState::Pressed => self.buttons |= btn.into(),
                    ElementState::Released => self.buttons &= !MouseEventButtons::from(btn),
                }

                let in_chrome = self.is_in_chrome(position);
                let mut select_all_url_bar_now = false;
                if state == ElementState::Pressed {
                    let was_url_focused = self.url_bar_focused();
                    let hit_url_bar = in_chrome && self.is_url_bar_at(position);
                    if !self.has_find_bar() {
                        self.close_popup();
                    }
                    if !hit_url_bar {
                        self.chrome_doc.clear_focus();
                        self.url_bar_select_all_on_click = None;
                    }
                    if in_chrome {
                        if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                            doc.clear_focus();
                        }
                    }
                    self.chrome_focused = hit_url_bar;
                    if in_chrome {
                        self.last_chrome_click_x_logical = (position.x / (self.hidpi_scale.max(1.0) as f64)) as f32;
                    }
                    if let Some(win) = &self.window {
                        focus_main_window(&**win, in_chrome);
                        super::bus::enable_window_ime(&**win);
                    }
                    if btn == MouseEventButton::Main {
                        if hit_url_bar && !was_url_focused {
                            self.suggest_query.clear();
                            self.url_bar_select_all_on_click = Some(position);
                            select_all_url_bar_now = true;
                        }
                        self.tab_drag = if in_chrome {
                            self.tab_index_at(position).map(|idx| TabDrag {
                                tab_id: self.tabs[idx].id,
                                start_pos: position,
                                active: false,
                            })
                        } else {
                            None
                        };
                        self.bm_drag = self.bm_source_at(position).map(|(src_spec, title)| BookmarkDrag {
                            src_spec,
                            title,
                            start_pos: position,
                            active: false,
                            hover_folder: None,
                        });
                    }
                } else if btn == MouseEventButton::Main {
                    if let Some(tdrag) = self.tab_drag.take() {
                        if tdrag.active && position.y / (self.hidpi_scale as f64) <= (TITLEBAR_HEIGHT + 16) as f64 {
                            self.bm_drag = None;
                            if let Some(win) = &self.window {
                                win.set_cursor(Cursor::Icon(CursorIcon::Default));
                            }
                            self.request_redraw();
                            return;
                        }
                    }
                    if let Some(drag) = self.bm_drag.take() {
                        if drag.active {
                            self.finish_bm_drag(drag, position);
                            self.send_pointer_leave(
                                button_source_to_blitz(&button),
                                primary,
                                position,
                                in_chrome,
                            );
                            return;
                        }
                    }
                }

                let pev = pointer_event(
                    button_source_to_blitz(&button),
                    primary,
                    self.coords_for(position, in_chrome),
                    btn,
                    self.buttons,
                    self.mods(),
                    PointerDetails::default(),
                );
                self.dispatch_to(
                    in_chrome,
                    match state {
                        ElementState::Pressed => UiEvent::PointerDown(pev),
                        ElementState::Released => UiEvent::PointerUp(pev),
                    },
                );
                if select_all_url_bar_now
                    || (state == ElementState::Released
                        && btn == MouseEventButton::Main
                        && self.url_bar_select_all_on_click.take().is_some())
                {
                    self.select_all_url_bar();
                }
                self.process_app_events(event_loop);
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.is_in_chrome(self.pointer_pos) {
                    let ev = UiEvent::Wheel(BlitzWheelEvent {
                        delta: wheel_delta(delta),
                        coords: self.content_coords(self.pointer_pos),
                        buttons: self.buttons,
                        mods: self.mods(),
                        element: Default::default(),
                    });
                    self.dispatch_to(false, ev);
                }
            }
            _ => {}
        }
    }

    /// Browser-level shortcuts. Returns true when the key was consumed.
    fn handle_shortcut(&mut self, code: KeyCode, event_loop: &dyn ActiveEventLoop) -> bool {
        let state = self.keyboard_modifiers.state();
        // Cmd on macOS (Ctrl+Tab keeps working there too), Ctrl elsewhere.
        let ctrl = primary_modifier(state) || (code == KeyCode::Tab && state.control_key());
        let shift = state.shift_key();
        let alt = state.alt_key();
        (ctrl && self.ctrl_shortcut(code, shift, event_loop))
            || (alt && self.alt_shortcut(code))
            || self.plain_key(code, !ctrl && !alt, shift)
    }

    fn ctrl_shortcut(&mut self, code: KeyCode, shift: bool, event_loop: &dyn ActiveEventLoop) -> bool {
        let n = self.tabs.len();
        if n == 0 {
            return false;
        }
        match code {
                KeyCode::KeyT if shift => self.reopen_closed_tab(),
                KeyCode::Tab if shift => self.switch_tab((self.active_tab + n - 1) % n),
                KeyCode::Tab => self.switch_tab((self.active_tab + 1) % n),
                KeyCode::PageUp => self.switch_tab((self.active_tab + n - 1) % n),
                KeyCode::PageDown => self.switch_tab((self.active_tab + 1) % n),
                KeyCode::Digit1 => self.switch_tab(0.min(n - 1)),
                KeyCode::Digit2 => self.switch_tab(1.min(n - 1)),
                KeyCode::Digit3 => self.switch_tab(2.min(n - 1)),
                KeyCode::Digit4 => self.switch_tab(3.min(n - 1)),
                KeyCode::Digit5 => self.switch_tab(4.min(n - 1)),
                KeyCode::Digit6 => self.switch_tab(5.min(n - 1)),
                KeyCode::Digit7 => self.switch_tab(6.min(n - 1)),
                KeyCode::Digit8 => self.switch_tab(7.min(n - 1)),
                KeyCode::Digit9 => self.switch_tab(n - 1),
                KeyCode::KeyD => self.handle_chrome_action("toggle-bookmark", event_loop),
                KeyCode::KeyK => self.open_command_palette(event_loop),
                KeyCode::KeyP if shift => self.open_command_palette(event_loop),
                KeyCode::KeyF => self.open_find_bar(event_loop),
                KeyCode::KeyH => self.navigate_current("rust://history"),
                KeyCode::KeyB if shift => self.toggle_bookmarks_bar(),
                KeyCode::KeyQ if cfg!(target_os = "macos") => self.exit_app(event_loop),
                KeyCode::BracketLeft => self.history_step(false),
                KeyCode::BracketRight => self.history_step(true),
                KeyCode::KeyO if shift => self.navigate_current("rust://bookmarks"),
                KeyCode::KeyT => {
                    self.open_new_tab("rust://home");
                    self.focus_url_bar();
                }
                KeyCode::KeyW | KeyCode::F4 => self.close_tab(self.active_tab, Some(event_loop)),
                KeyCode::KeyL => self.focus_url_bar(),
                KeyCode::KeyR => self.reload_active_tab(),
                KeyCode::Equal | KeyCode::NumpadAdd => self.set_zoom(self.tabs[self.active_tab].zoom + 0.1),
                KeyCode::Minus | KeyCode::NumpadSubtract => self.set_zoom(self.tabs[self.active_tab].zoom - 0.1),
                KeyCode::Digit0 | KeyCode::Numpad0 => self.set_zoom(1.0),
                _ => return false,
        }
        true
    }

    fn alt_shortcut(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::ArrowLeft => self.history_step(false),
            KeyCode::ArrowRight => self.history_step(true),
            KeyCode::KeyD => self.focus_url_bar(),
            KeyCode::KeyR => self.toggle_reader_mode(),
            KeyCode::KeyT => self.toggle_translate(),
            KeyCode::Home => self.navigate_current("rust://home"),
            KeyCode::Enter | KeyCode::NumpadEnter if self.url_bar_focused() => {
                if !self.suggest_open_new_tab() {
                    if let Some(text) = self.read_url_bar_text() {
                        let target = normalize_url_input(&text);
                        self.close_popup();
                        self.open_new_tab(&target);
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// F5 / F6 / Esc / scrolling keys / Enter in the URL bar or a built-in page's search box.
    fn plain_key(&mut self, code: KeyCode, no_mods: bool, shift: bool) -> bool {
        if self.url_bar_focused() && self.suggest_key(code) {
            return true;
        }
        if code == KeyCode::F5 {
            self.reload_active_tab();
            return true;
        }
        if code == KeyCode::F6 {
            self.focus_url_bar();
            return true;
        }
        if code == KeyCode::Escape {
            if self.has_find_bar() {
                self.close_find_bar();
                return true;
            }
            let tab = &mut self.tabs[self.active_tab];
            if tab.is_loading && !tab.js_mode {
                tab.cancel_inflight();
                self.rebuild_chrome();
                return true;
            }
            if self.chrome_focused {
                self.chrome_focused = false;
                self.chrome_doc.clear_focus();
                self.last_chrome_html.clear();
                self.rebuild_chrome();
                return true;
            }
        }
        if no_mods && self.keyboard_scroll(code, shift) {
            return true;
        }
        if !matches!(code, KeyCode::Enter | KeyCode::NumpadEnter) {
            return false;
        }
        if self.chrome_focused {
            if !self.url_bar_focused() || self.url_bar_composing() {
                return false;
            }
            if let Some(text) = self.read_url_bar_text() {
                self.navigate_current(&text);
            }
            return true;
        }
        if let Some(q) = self.focused_builtin_input("home-search") {
            if !q.is_empty() {
                self.navigate_current(&q);
            }
            return true;
        }
        if self.tabs[self.active_tab].url.starts_with("rust://bookmarks") {
            if let Some(q) = self.focused_builtin_input("bm-search") {
                let next_url = if q.is_empty() {
                    "rust://bookmarks".to_string()
                } else {
                    let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
                    format!("rust://bookmarks?q={encoded}")
                };
                self.navigate_current(&next_url);
                return true;
            }
        }
        if self.tabs[self.active_tab].url.starts_with("rust://history") {
            if let Some(q) = self.focused_builtin_input("hist-search") {
                let next_url = if q.is_empty() {
                    "rust://history".to_string()
                } else {
                    let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
                    format!("rust://history?q={encoded}")
                };
                self.navigate_current(&next_url);
                return true;
            }
        }
        false
    }

    /// Keyboard scrolling for the content document (Blitz has no built-in key scrolling).
    fn keyboard_scroll(&mut self, code: KeyCode, shift: bool) -> bool {
        if self.chrome_focused {
            return false;
        }
        let page_h = {
            let (_, ch) = self.content_phys_size();
            ch as f64 / self.hidpi_scale.max(1.0) as f64 / self.tabs[self.active_tab].zoom.max(0.1) as f64
        };
        if self.active_tab >= self.tabs.len() {
            return false;
        }
        let Some(doc) = &mut self.tabs[self.active_tab].doc else {
            return false;
        };
        let focused = doc.get_focussed_node_id().and_then(|id| doc.get_node(id));
        let typing = focused
            .as_ref()
            .and_then(|n| n.element_data())
            .is_some_and(|el| el.text_input_data().is_some());
        if typing {
            return false;
        }
        // Space activates focused buttons/links/checkboxes; only scroll when
        // focus is on the page body (no element focused).
        if matches!(code, KeyCode::Space) && focused.is_some() {
            return false;
        }
        let dy = match code {
            KeyCode::ArrowDown => 48.0,
            KeyCode::ArrowUp => -48.0,
            KeyCode::PageDown => page_h * 0.88,
            KeyCode::PageUp => -page_h * 0.88,
            KeyCode::Space if shift => -page_h * 0.88,
            KeyCode::Space => page_h * 0.88,
            KeyCode::Home => -1.0e9,
            KeyCode::End => 1.0e9,
            _ => return false,
        };
        // Blitz uses wheel-style deltas: negative scrolls the page down.
        doc.scroll_viewport_by(0.0, -dy);
        self.request_redraw();
        true
    }

    /// Mouse-down on the window frame: edge resize or title-bar drag / double-click maximize.
    /// Returns true when consumed.
    fn frame_press(&mut self, position: PhysicalPosition<f64>) -> bool {
        if let Some(dir) = self.resize_direction_at(position) {
            if let Some(win) = &self.window {
                let _ = win.drag_resize_window(dir);
            }
            return true;
        }
        if !self.is_titlebar_drag_area(position) {
            return false;
        }
        self.close_popup();
        let double = self.last_titlebar_press.is_some_and(|t| t.elapsed().as_millis() < 450);
        self.last_titlebar_press = if double { None } else { Some(Instant::now()) };
        if double {
            self.toggle_maximize();
        } else if let Some(win) = &self.window {
            let _ = win.drag_window();
        }
        self.buttons = MouseEventButtons::None;
        true
    }

    /// Resize direction when the pointer is on the frameless window's edge.
    fn resize_direction_at(&self, pos: PhysicalPosition<f64>) -> Option<ResizeDirection> {
        // Only the frameless Windows window needs hand-rolled edge resizing.
        if self.maximized || !cfg!(target_os = "windows") {
            return None;
        }
        let edge = (RESIZE_EDGE as f64) * self.hidpi_scale as f64;
        let w = self.win_size.width as f64;
        let h = self.win_size.height as f64;
        let (l, r, t, b) = (pos.x < edge, pos.x >= w - edge, pos.y < edge, pos.y >= h - edge);
        Some(match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    /// True when the pointer is over empty title-bar space (not a tab or button).
    fn is_titlebar_drag_area(&self, pos: PhysicalPosition<f64>) -> bool {
        let logical: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        if logical.y >= TITLEBAR_HEIGHT as f32 {
            return false;
        }
        let mut node_id = self.chrome_doc.element_from_point(logical.x, logical.y);
        while let Some(id) = node_id {
            let Some(node) = self.chrome_doc.get_node(id) else {
                break;
            };
            if let Some(el) = node.element_data() {
                if el.attr(LocalName::from("data-action")).is_some() {
                    return false;
                }
                if el.attr(LocalName::from("data-drag")).is_some() {
                    return true;
                }
            }
            node_id = node.parent;
        }
        false
    }

    fn is_in_chrome(&self, pos: PhysicalPosition<f64>) -> bool {
        pos.y / (self.hidpi_scale as f64) < self.chrome_height() as f64
    }

    fn coords_for(&self, pos: PhysicalPosition<f64>, in_chrome: bool) -> PointerCoords {
        if in_chrome {
            let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
            super::helpers::plain_coords(l.x, l.y, 0.0, 0.0)
        } else {
            self.content_coords(pos)
        }
    }

    fn content_coords(&self, pos: PhysicalPosition<f64>) -> PointerCoords {
        let logical: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        let doc = self.tabs.get(self.active_tab).and_then(|t| t.doc.as_ref());
        let zoom = doc.map_or(1.0, |d| d.viewport().zoom);
        let scroll = doc.map_or(blitz_dom::Point { x: 0.0, y: 0.0 }, |d| d.viewport_scroll());
        let client_x = logical.x / zoom;
        let client_y = (logical.y - self.chrome_height() as f32) / zoom;
        PointerCoords {
            screen_x: logical.x,
            screen_y: logical.y,
            client_x,
            client_y,
            page_x: client_x + scroll.x as f32,
            page_y: client_y + scroll.y as f32,
        }
    }

    /// Sends `ev` to the browser UI (`to_chrome`) or the active page.
    fn dispatch_to(&mut self, to_chrome: bool, ev: UiEvent) {
        if to_chrome {
            self.dispatch_chrome_ui(ev);
        } else if self.active_tab < self.tabs.len() {
            if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                doc.handle_ui_event(ev);
            }
        }
    }

    /// Keyboard / IME events go to whichever side has focus.
    fn dispatch_focused(&mut self, ev: UiEvent) {
        self.dispatch_to(self.chrome_focused, ev);
    }

    fn send_pointer_leave(&mut self, id: BlitzPointerId, primary: bool, pos: PhysicalPosition<f64>, chrome: bool) {
        let ev = pointer_event(
            id,
            primary,
            self.coords_for(pos, chrome),
            Default::default(),
            self.buttons,
            self.mods(),
            PointerDetails::default(),
        );
        self.dispatch_to(chrome, UiEvent::PointerLeave(ev));
    }

    /// True when `pos` is over `#url-bar` (or its leading area inside `.omni`, excluding inline action buttons).
    fn is_url_bar_at(&self, pos: PhysicalPosition<f64>) -> bool {
        let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        let mut node_id = self.chrome_doc.element_from_point(l.x, l.y);
        while let Some(id) = node_id {
            let Some(node) = self.chrome_doc.get_node(id) else {
                break;
            };
            if let Some(el) = node.element_data() {
                if el.attr(LocalName::from("data-action")).is_some() {
                    return false;
                }
                if el.attr(LocalName::from("id")) == Some("url-bar")
                    || el.attr(LocalName::from("class")).is_some_and(|c| c.split_whitespace().any(|k| k == "omni"))
                {
                    return true;
                }
            }
            node_id = node.parent;
        }
        false
    }

    fn update_link_hover(&mut self, in_chrome: bool) {
        if in_chrome || self.active_webview.is_some() {
            self.set_hover_link(None);
            return;
        }
        let href = self.tabs[self.active_tab].doc.as_ref().and_then(|doc| {
            let mut cur = doc.get_hover_node_id();
            while let Some(id) = cur {
                let node = doc.get_node(id)?;
                if let Some(el) = node.element_data() {
                    if &*el.name.local == "a" {
                        if let Some(raw) = el.attr(LocalName::from("href")) {
                            let trimmed = raw.trim();
                            if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("javascript:") {
                                return Some(
                                    url::Url::parse(&self.tabs[self.active_tab].url)
                                        .and_then(|base| base.join(trimmed))
                                        .map(String::from)
                                        .unwrap_or_else(|_| trimmed.to_string()),
                                );
                            }
                        }
                    }
                }
                cur = node.parent;
            }
            None
        });
        self.set_hover_link(href);
    }

    /// Tab index at `pos` when pressing on a `.tab` (ignoring `.tab-close`).
    fn tab_index_at(&self, pos: PhysicalPosition<f64>) -> Option<usize> {
        let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        if l.y >= TITLEBAR_HEIGHT as f32 {
            return None;
        }
        self.tab_hit_at_x(l.x).map(|(idx, _, _)| idx)
    }

    /// Returns `(tab_idx, tab_left_x, tab_width)` for the tab at logical `x` in the title bar.
    fn tab_hit_at_x(&self, lx: f32) -> Option<(usize, f32, f32)> {
        let mut node_id = self.chrome_doc.element_from_point(lx, (TITLEBAR_HEIGHT as f32) * 0.55);
        while let Some(id) = node_id {
            let node = self.chrome_doc.get_node(id)?;
            if let Some(el) = node.element_data() {
                if let Some(act) = el.attr(LocalName::from("data-action")) {
                    if act.starts_with("close-tab:") {
                        return None;
                    }
                    if let Some(idx_str) = act.strip_prefix("tab:") {
                        let idx = idx_str.parse::<usize>().ok()?;
                        let pos = node.absolute_position(0.0, 0.0);
                        let w = node.final_layout().size.width;
                        return Some((idx, pos.x, w));
                    }
                }
            }
            node_id = node.parent;
        }
        None
    }

    /// Reorders tabs in real time when dragging a tab horizontally across the tab strip.
    fn update_tab_drag(&mut self, pos: PhysicalPosition<f64>) -> bool {
        let Some(drag) = &self.tab_drag else {
            return false;
        };
        let dx = pos.x - drag.start_pos.x;
        let dy = pos.y - drag.start_pos.y;
        let threshold = 5.0 * (self.hidpi_scale as f64);
        if !drag.active && (dx * dx + dy * dy) < threshold * threshold {
            return false;
        }
        let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        if l.y > (TITLEBAR_HEIGHT + 16) as f32 {
            // Dragged below the title bar: let bookmark drag-and-drop handle dropping onto a folder.
            return false;
        }
        let tab_id = drag.tab_id;
        self.tab_drag.as_mut().unwrap().active = true;
        let Some(cur_idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return true;
        };
        if self.active_tab != cur_idx {
            self.switch_tab(cur_idx);
        }
        if let Some((over_idx, tab_x, tab_w)) = self.tab_hit_at_x(l.x) {
            if over_idx != cur_idx && over_idx < self.tabs.len() {
                let should_swap = if over_idx > cur_idx {
                    l.x > tab_x + tab_w * 0.35
                } else {
                    l.x < tab_x + tab_w * 0.65
                };
                if should_swap {
                    let tab = self.tabs.remove(cur_idx);
                    self.tabs.insert(over_idx, tab);
                    self.active_tab = over_idx;
                    self.rebuild_chrome();
                }
            }
        }
        true
    }

    /// Finds a draggable bookmark/tab (`data-bm-src`) under `pos`.
    fn bm_source_at(&self, pos: PhysicalPosition<f64>) -> Option<(String, String)> {
        if self.is_in_chrome(pos) {
            let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
            let mut node_id = self.chrome_doc.element_from_point(l.x, l.y);
            while let Some(id) = node_id {
                let node = self.chrome_doc.get_node(id)?;
                if let Some(el) = node.element_data() {
                    if el
                        .attr(LocalName::from("data-action"))
                        .is_some_and(|a| a.starts_with("close-tab:"))
                    {
                        return None;
                    }
                    if let Some(src) = el.attr(LocalName::from("data-bm-src")) {
                        let title = el
                            .attr(LocalName::from("data-bm-title"))
                            .unwrap_or(src)
                            .to_string();
                        return Some((src.to_string(), title));
                    }
                }
                node_id = node.parent;
            }
            None
        } else {
            if self.active_webview.is_some() {
                return None;
            }
            let doc = self.tabs[self.active_tab].doc.as_ref()?;
            let c = self.content_coords(pos);
            let mut node_id = doc.element_from_point(c.client_x, c.client_y);
            while let Some(id) = node_id {
                let node = doc.get_node(id)?;
                if let Some(el) = node.element_data() {
                    if let Some(src) = el.attr(LocalName::from("data-bm-src")) {
                        let title = el
                            .attr(LocalName::from("data-bm-title"))
                            .unwrap_or(src)
                            .to_string();
                        return Some((src.to_string(), title));
                    }
                }
                node_id = node.parent;
            }
            None
        }
    }

    /// Finds a drop-target bookmark folder (`data-bm-folder`) under `pos`.
    fn bm_folder_at(&self, pos: PhysicalPosition<f64>) -> Option<String> {
        if self.is_in_chrome(pos) {
            let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
            let mut node_id = self.chrome_doc.element_from_point(l.x, l.y);
            while let Some(id) = node_id {
                let node = self.chrome_doc.get_node(id)?;
                if let Some(el) = node.element_data() {
                    if let Some(folder) = el.attr(LocalName::from("data-bm-folder")) {
                        return Some(folder.to_string());
                    }
                }
                node_id = node.parent;
            }
            None
        } else {
            if self.active_webview.is_some() {
                return None;
            }
            let doc = self.tabs[self.active_tab].doc.as_ref()?;
            let c = self.content_coords(pos);
            let mut node_id = doc.element_from_point(c.client_x, c.client_y);
            while let Some(id) = node_id {
                let node = doc.get_node(id)?;
                if let Some(el) = node.element_data() {
                    if let Some(folder) = el.attr(LocalName::from("data-bm-folder")) {
                        return Some(folder.to_string());
                    }
                }
                node_id = node.parent;
            }
            None
        }
    }

    /// Updates bookmark drag state on pointer move; returns `true` when an active drag consumed the move.
    fn update_bm_drag(&mut self, pos: PhysicalPosition<f64>) -> bool {
        let Some(drag) = &self.bm_drag else {
            return false;
        };
        let dx = pos.x - drag.start_pos.x;
        let dy = pos.y - drag.start_pos.y;
        let threshold = 6.0 * (self.hidpi_scale as f64);
        if !drag.active && (dx * dx + dy * dy) < threshold * threshold {
            return false;
        }
        let target_folder = self.bm_folder_at(pos);
        let drag = self.bm_drag.as_mut().unwrap();
        drag.active = true;
        let changed_folder = drag.hover_folder != target_folder;
        drag.hover_folder = target_folder.clone();

        if let Some(win) = &self.window {
            let icon = if target_folder.is_some() {
                CursorIcon::Copy
            } else {
                CursorIcon::Grabbing
            };
            win.set_cursor(Cursor::Icon(icon));
        }

        if changed_folder {
            if self.tabs[self.active_tab].url.starts_with("rust://bookmarks") {
                let scroll = self.current_scroll_y();
                self.load_active_tab(false, Some(scroll));
            } else {
                self.rebuild_chrome();
            }
        }
        true
    }

    fn finish_bm_drag(&mut self, drag: BookmarkDrag, pos: PhysicalPosition<f64>) {
        if let Some(win) = &self.window {
            win.set_cursor(Cursor::Icon(CursorIcon::Default));
        }
        if let Some(folder_spec) = self.bm_folder_at(pos) {
            let _ = self.bookmarks.move_to_folder(&drag.src_spec, &drag.title, &folder_spec);
        }
        let active_url = self.tabs[self.active_tab].url.clone();
        if active_url.starts_with("rust://bookmarks") || active_url == "rust://home" {
            let scroll = self.current_scroll_y();
            self.load_active_tab(false, Some(scroll));
        } else {
            self.rebuild_chrome();
        }
    }
}
