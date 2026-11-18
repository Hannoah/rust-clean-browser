//! Main-window input: keyboard shortcuts, pointer routing (chrome vs. content), frameless
//! window resize / drag.

use super::BrowserApp;
use super::helpers::{
    RESIZE_EDGE, focus_main_window, mouse_button, pointer_event, primary_modifier, resize_cursor, wheel_delta,
};
use crate::chrome_ui::{CHROME_HEIGHT, TITLEBAR_HEIGHT};
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
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::ResizeDirection;

impl BrowserApp {
    pub(super) fn handle_main_window_event(&mut self, event_loop: &dyn ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.exit_app(event_loop),
            WindowEvent::ThemeChanged(theme) => {
                self.color_scheme = theme_to_color_scheme(theme);
                self.sync_viewports();
            }
            WindowEvent::RedrawRequested => self.redraw(),
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
                self.request_redraw();
            }
            WindowEvent::PointerMoved {
                position,
                source,
                primary,
                ..
            } => {
                self.pointer_pos = position;
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
            }
            WindowEvent::PointerButton {
                button,
                state,
                primary,
                position,
                ..
            } => {
                self.pointer_pos = position;
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
                if state == ElementState::Pressed {
                    self.close_popup();
                    self.chrome_focused = in_chrome;
                    if in_chrome {
                        self.last_chrome_click_x_logical = (position.x / (self.hidpi_scale.max(1.0) as f64)) as f32;
                    }
                    if let Some(win) = &self.window {
                        focus_main_window(&**win, in_chrome);
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
        match code {
                KeyCode::KeyT if shift => self.reopen_closed_tab(),
                KeyCode::Tab if shift => self.switch_tab((self.active_tab + n - 1) % n),
                KeyCode::Tab => self.switch_tab((self.active_tab + 1) % n),
                KeyCode::Digit1
                | KeyCode::Digit2
                | KeyCode::Digit3
                | KeyCode::Digit4
                | KeyCode::Digit5
                | KeyCode::Digit6
                | KeyCode::Digit7
                | KeyCode::Digit8 => {
                    let i = code as usize - KeyCode::Digit1 as usize;
                    self.switch_tab(i.min(n - 1));
                }
                KeyCode::Digit9 => self.switch_tab(n - 1),
                KeyCode::KeyD => self.handle_chrome_action("toggle-bookmark", event_loop),
                KeyCode::KeyQ if cfg!(target_os = "macos") => self.exit_app(event_loop),
                KeyCode::BracketLeft => self.history_step(false),
                KeyCode::BracketRight => self.history_step(true),
                KeyCode::KeyO if shift => self.navigate_current("rust://bookmarks"),
                KeyCode::KeyT => {
                    self.open_new_tab("rust://home");
                    self.focus_url_bar();
                }
                KeyCode::KeyW => self.close_tab(self.active_tab, Some(event_loop)),
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
            KeyCode::Home => self.navigate_current("rust://home"),
            _ => return false,
        }
        true
    }

    /// F5 / Esc / scrolling keys / Enter in the URL bar or a built-in page's search box.
    fn plain_key(&mut self, code: KeyCode, no_mods: bool, shift: bool) -> bool {
        if code == KeyCode::F5 {
            self.reload_active_tab();
            return true;
        }
        if code == KeyCode::Escape {
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
            if !self.url_bar_focused() {
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
        false
    }

    /// Keyboard scrolling for the content document (Blitz has no built-in key scrolling).
    fn keyboard_scroll(&mut self, code: KeyCode, shift: bool) -> bool {
        if self.chrome_focused {
            return false;
        }
        let page_h = {
            let (_, ch) = self.content_phys_size();
            ch as f64 / self.hidpi_scale as f64 / self.tabs[self.active_tab].zoom as f64
        };
        let Some(doc) = &mut self.tabs[self.active_tab].doc else {
            return false;
        };
        let typing = doc
            .get_focussed_node_id()
            .and_then(|id| doc.get_node(id))
            .and_then(|n| n.element_data())
            .is_some_and(|el| el.text_input_data().is_some());
        if typing {
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
        pos.y / (self.hidpi_scale as f64) < CHROME_HEIGHT as f64
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
        let doc = self.tabs[self.active_tab].doc.as_ref();
        let zoom = doc.map_or(1.0, |d| d.viewport().zoom);
        let scroll = doc.map_or(blitz_dom::Point { x: 0.0, y: 0.0 }, |d| d.viewport_scroll());
        let client_x = logical.x / zoom;
        let client_y = (logical.y - CHROME_HEIGHT as f32) / zoom;
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
        } else if let Some(doc) = &mut self.tabs[self.active_tab].doc {
            doc.handle_ui_event(ev);
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
}
