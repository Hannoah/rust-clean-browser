//! Main-window input: keyboard shortcuts, pointer routing (chrome vs. content), frameless
//! window resize / drag.

use super::helpers::{
    RESIZE_EDGE, focus_main_window, is_primary_mouse_button_down, main_window_has_keyboard_focus, mouse_button,
    normalize_url_input, pointer_event, primary_modifier, resize_cursor, wheel_delta,
};
use super::{BookmarkDrag, BrowserApp, TabDrag, UiRegion};
use crate::chrome_ui::TITLEBAR_HEIGHT;
use crate::events::{
    button_source_to_blitz, pointer_kind_to_blitz, pointer_source_to_blitz, pointer_source_to_blitz_details,
    theme_to_color_scheme, winit_ime_to_blitz, winit_key_event_to_blitz,
};
use anyrender::WindowRenderer;
use blitz_dom::{Document, LocalName};
use blitz_traits::events::{
    BlitzPointerId, BlitzWheelDelta, BlitzWheelEvent, MouseEventButton, MouseEventButtons, PointerCoords,
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
                let mut need_chrome = false;
                if let Some(tdrag) = self.tab_drag.take()
                    && (tdrag.active || tdrag.detaching)
                {
                    need_chrome = true;
                }
                if self.sidebar_drag {
                    self.sidebar_drag = false;
                    super::helpers::release_mouse_capture();
                    self.prefs.save();
                }
                self.url_bar_select_all_on_click = None;
                self.url_bar_just_selected_all = None;
                if self.focus_region == UiRegion::Chrome {
                    self.focus_region = UiRegion::Content;
                    self.chrome_doc.clear_focus();
                    self.request_redraw();
                } else if self.focus_region == UiRegion::Sidebar {
                    self.focus_region = UiRegion::Content;
                    if let Some(sdoc) = &mut self.sidebar_doc {
                        sdoc.clear_focus();
                    }
                    self.request_redraw();
                }
                if let Some(drag) = self.bm_drag.take()
                    && drag.active
                {
                    need_chrome = true;
                }
                if need_chrome {
                    if let Some(win) = &self.window {
                        win.set_cursor(Cursor::Icon(CursorIcon::Default));
                    }
                    self.rebuild_chrome();
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
                if self.focus_region == UiRegion::Chrome {
                    self.update_suggestions(event_loop);
                }
                self.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed()
                    && let PhysicalKey::Code(code) = event.physical_key
                    && self.handle_shortcut(code, event_loop)
                {
                    return;
                }
                let key_data = winit_key_event_to_blitz(&event, self.keyboard_modifiers.state());
                self.dispatch_focused(if event.state.is_pressed() {
                    UiEvent::KeyDown(key_data)
                } else {
                    UiEvent::KeyUp(key_data)
                });
                self.process_app_events(event_loop);
                if self.focus_region == UiRegion::Chrome {
                    self.update_suggestions(event_loop);
                } else if self.focus_region == UiRegion::Sidebar {
                    self.sync_chat_input_height();
                }
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
                self.send_pointer_leave(id, primary, pos, UiRegion::Chrome);
                self.send_pointer_leave(id, primary, pos, UiRegion::Content);
                self.send_pointer_leave(id, primary, pos, UiRegion::Sidebar);
                self.pointer_region = None;
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
                if (self.tab_drag.is_some() || self.bm_drag.is_some() || self.sidebar_drag)
                    && !is_primary_mouse_button_down()
                {
                    self.buttons &= !MouseEventButtons::from(MouseEventButton::Main);
                    let mut need_chrome = false;
                    if let Some(tdrag) = self.tab_drag.take()
                        && (tdrag.active || tdrag.detaching)
                    {
                        need_chrome = true;
                    }
                    if let Some(drag) = self.bm_drag.take()
                        && drag.active
                    {
                        need_chrome = true;
                    }
                    if self.sidebar_drag {
                        self.sidebar_drag = false;
                        super::helpers::release_mouse_capture();
                        self.prefs.save();
                    }
                    if need_chrome {
                        if let Some(win) = &self.window {
                            win.set_cursor(Cursor::Icon(CursorIcon::Default));
                        }
                        self.rebuild_chrome();
                    }
                }
                if self.sidebar_drag && self.update_sidebar_drag(position) {
                    return;
                }
                if self.tab_drag.is_none() && self.bm_drag.is_none() {
                    let resize = self.resize_direction_at(position);
                    let sb_edge = resize.is_none() && self.is_sidebar_resize_edge(position);
                    let target_dir = if sb_edge { Some(ResizeDirection::West) } else { resize };
                    if target_dir != self.resize_hover {
                        self.resize_hover = target_dir;
                        if let Some(win) = &self.window {
                            win.set_cursor(Cursor::Icon(target_dir.map_or(CursorIcon::Default, resize_cursor)));
                        }
                    }
                    if resize.is_some() || sb_edge {
                        return;
                    }
                } else if self.resize_hover.take().is_some()
                    && let Some(win) = &self.window
                {
                    win.set_cursor(Cursor::Icon(CursorIcon::Default));
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
                    // Intentional drag inside `#url-bar`: cancel click-to-select-all and let
                    // the original `PointerDown` from `Pressed` extend character selection cleanly.
                    self.url_bar_select_all_on_click = None;
                }
                let id = pointer_source_to_blitz(&source);
                let now_region = self.region_at(position);
                if let Some(was_region) = self.pointer_region
                    && was_region != now_region
                {
                    self.send_pointer_leave(id, primary, position, was_region);
                }
                self.pointer_region = Some(now_region);
                let ev = pointer_event(
                    id,
                    primary,
                    self.coords_for_region(position, now_region),
                    Default::default(),
                    self.buttons,
                    self.mods(),
                    pointer_source_to_blitz_details(&source),
                );
                // No unconditional redraw: Blitz requests one when hover/selection changes.
                self.dispatch_to_region(now_region, UiEvent::PointerMove(ev));
                self.update_link_hover(now_region != UiRegion::Content);
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

                // Frameless window: edge resize, sidebar sash resize, and title-bar drag / maximize.
                if state == ElementState::Pressed && btn == MouseEventButton::Main {
                    if self.is_sidebar_resize_edge(position) {
                        self.sidebar_drag = true;
                        if let Some(win) = &self.window {
                            super::helpers::capture_mouse(&**win);
                        }
                        self.buttons |= btn.into();
                        return;
                    }
                    if self.frame_press(position) {
                        return;
                    }
                }

                match state {
                    ElementState::Pressed => self.buttons |= btn.into(),
                    ElementState::Released => self.buttons &= !MouseEventButtons::from(btn),
                }

                if state == ElementState::Released && btn == MouseEventButton::Main && self.sidebar_drag {
                    self.sidebar_drag = false;
                    super::helpers::release_mouse_capture();
                    self.prefs.save();
                    if let Some(win) = &self.window {
                        win.set_cursor(Cursor::Icon(CursorIcon::Default));
                    }
                    return;
                }

                let region = self.region_at(position);
                let in_chrome = region == UiRegion::Chrome;
                let mut force_url_caret_at_pos = false;
                if state == ElementState::Pressed {
                    let win_had_kbd_focus = self.window.as_deref().is_none_or(main_window_has_keyboard_focus);
                    let was_url_focused = self.url_bar_focused() && win_had_kbd_focus;
                    let hit_url_bar = in_chrome && self.is_url_bar_at(position);
                    let hit_exact_url_input = hit_url_bar && self.is_exact_url_input_at(position);
                    if !self.has_find_bar() {
                        self.close_popup();
                    }
                    if !hit_url_bar {
                        self.chrome_doc.clear_focus();
                        self.url_bar_select_all_on_click = None;
                        self.url_bar_just_selected_all = None;
                    }
                    if region != UiRegion::Content
                        && let Some(doc) = &mut self.tabs[self.active_tab].doc
                    {
                        doc.clear_focus();
                    }
                    if region != UiRegion::Sidebar
                        && let Some(sdoc) = &mut self.sidebar_doc
                    {
                        sdoc.clear_focus();
                    }
                    self.focus_region = if hit_url_bar {
                        UiRegion::Chrome
                    } else if region == UiRegion::Sidebar {
                        UiRegion::Sidebar
                    } else {
                        UiRegion::Content
                    };
                    if in_chrome {
                        self.last_chrome_click_x_logical = (position.x / (self.hidpi_scale.max(1.0) as f64)) as f32;
                    }
                    if let Some(win) = &self.window {
                        focus_main_window(&**win, region != UiRegion::Content || self.active_webview.is_none());
                        super::bus::enable_window_ime(&**win);
                    }
                    if btn == MouseEventButton::Main {
                        if hit_url_bar && !was_url_focused {
                            self.sync_suggest_query_to_url_bar();
                            self.url_bar_select_all_on_click = Some(position);
                            self.url_bar_just_selected_all = None;
                            if !hit_exact_url_input {
                                force_url_caret_at_pos = true;
                            }
                        } else if hit_url_bar && was_url_focused {
                            let just_selected = self
                                .url_bar_just_selected_all
                                .take()
                                .is_some_and(|t| t.elapsed().as_millis() < 600);
                            if just_selected || !hit_exact_url_input {
                                force_url_caret_at_pos = true;
                            }
                        } else {
                            self.url_bar_just_selected_all = None;
                        }
                        self.tab_drag = if in_chrome {
                            self.tab_index_at(position).map(|idx| TabDrag {
                                tab_id: self.tabs[idx].id,
                                start_pos: position,
                                active: false,
                                detaching: false,
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
                    if let Some(tdrag) = self.tab_drag.take()
                        && tdrag.active
                    {
                        if tdrag.detaching {
                            self.bm_drag = None;
                            if let Some(win) = &self.window {
                                win.set_cursor(Cursor::Icon(CursorIcon::Default));
                            }
                            self.detach_tab_to_new_window(tdrag.tab_id, position);
                            self.send_pointer_leave(button_source_to_blitz(&button), primary, position, region);
                            return;
                        }
                        let dropping_on_bm =
                            self.bm_drag.as_ref().is_some_and(|d| d.active) && self.bm_folder_at(position).is_some();
                        if !dropping_on_bm {
                            self.bm_drag = None;
                            if let Some(win) = &self.window {
                                win.set_cursor(Cursor::Icon(CursorIcon::Default));
                            }
                            self.rebuild_chrome();
                            self.request_redraw();
                            return;
                        }
                    }
                    if let Some(drag) = self.bm_drag.take()
                        && drag.active
                    {
                        self.finish_bm_drag(drag, position);
                        self.send_pointer_leave(button_source_to_blitz(&button), primary, position, region);
                        return;
                    }
                }

                let pev = pointer_event(
                    button_source_to_blitz(&button),
                    primary,
                    self.coords_for_region(position, region),
                    btn,
                    self.buttons,
                    self.mods(),
                    PointerDetails::default(),
                );
                self.dispatch_to_region(
                    region,
                    match state {
                        ElementState::Pressed => UiEvent::PointerDown(pev),
                        ElementState::Released => UiEvent::PointerUp(pev),
                    },
                );
                if force_url_caret_at_pos {
                    self.place_url_bar_caret_at(position);
                }
                if state == ElementState::Released
                    && btn == MouseEventButton::Main
                    && self.url_bar_select_all_on_click.take().is_some()
                {
                    self.select_all_url_bar();
                    self.url_bar_just_selected_all = Some(Instant::now());
                }
                self.process_app_events(event_loop);
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } if self.region_at(self.pointer_pos) != UiRegion::Chrome => {
                let region = self.region_at(self.pointer_pos);
                if region == UiRegion::Content && primary_modifier(self.keyboard_modifiers.state()) {
                    let notches = match wheel_delta(delta) {
                        BlitzWheelDelta::Lines(_, y) => y as f32,
                        BlitzWheelDelta::Pixels(_, y) => y as f32 / 120.0, // ~one notch; smooth wheels/pinch accumulate
                    };
                    self.zoom_by_notches(notches);
                    return;
                }
                // Mouse-wheel notches glide (like Chrome/Edge smooth scrolling); precision touchpads
                // already send fine-grained pixel deltas with OS inertia, so those pass through.
                if let BlitzWheelDelta::Lines(lx, ly) = wheel_delta(delta) {
                    let px = self.wheel_line_px(region);
                    self.queue_smooth_scroll(region, false, lx * px, ly * px);
                    return;
                }
                let ev = UiEvent::Wheel(BlitzWheelEvent {
                    delta: wheel_delta(delta),
                    coords: self.coords_for_region(self.pointer_pos, region),
                    buttons: self.buttons,
                    mods: self.mods(),
                    element: Default::default(),
                });
                self.dispatch_to_region(region, ev);
                self.request_redraw();
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
            KeyCode::Digit1 => self.switch_tab(0),
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
            KeyCode::KeyJ => self.toggle_downloads_popup(event_loop),
            KeyCode::KeyH => self.navigate_current("rust://history"),
            KeyCode::KeyB if shift => self.toggle_bookmarks_bar(),
            KeyCode::KeyI if shift => self.toggle_sidebar(),
            // Text dragged out of an AI reply: copy just that part.
            KeyCode::KeyC if !shift && self.focus_region == UiRegion::Sidebar && self.sidebar_has_selection() => {
                self.copy_sidebar_selection();
            }
            KeyCode::KeyQ if cfg!(target_os = "macos") => self.exit_app(event_loop),
            KeyCode::BracketLeft => self.history_step(false),
            KeyCode::BracketRight => self.history_step(true),
            KeyCode::KeyO if shift => self.navigate_current("rust://bookmarks"),
            KeyCode::KeyN => self.open_new_window(None),
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
            KeyCode::KeyE => self.toggle_pdf_editor(),
            KeyCode::Home => self.navigate_current("rust://home"),
            KeyCode::Enter | KeyCode::NumpadEnter if self.url_bar_focused() => {
                if !self.suggest_open_new_tab()
                    && let Some(text) = self.read_url_bar_text()
                {
                    let target = normalize_url_input(&text);
                    self.close_popup();
                    self.open_new_tab(&target);
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
            if self.tab_drag.take().is_some() || self.bm_drag.take().is_some() {
                if let Some(win) = &self.window {
                    win.set_cursor(Cursor::Icon(CursorIcon::Default));
                }
                self.rebuild_chrome();
                return true;
            }
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
            if self.focus_region == UiRegion::Chrome {
                self.focus_region = UiRegion::Content;
                self.chrome_doc.clear_focus();
                self.last_chrome_html.clear();
                self.rebuild_chrome();
                return true;
            }
            if self.focus_region == UiRegion::Sidebar {
                self.focus_region = UiRegion::Content;
                if let Some(sdoc) = &mut self.sidebar_doc {
                    sdoc.clear_focus();
                }
                if self.chat_show_settings || self.sidebar_picker != crate::chrome_ui::SidebarPicker::None {
                    self.chat_show_settings = false;
                    self.sidebar_picker = crate::chrome_ui::SidebarPicker::None;
                    self.last_sidebar_html.clear();
                    self.rebuild_sidebar(false);
                }
                self.request_redraw();
                return true;
            }
        }
        if no_mods && self.keyboard_scroll(code, shift) {
            return true;
        }
        if !matches!(code, KeyCode::Enter | KeyCode::NumpadEnter) {
            return false;
        }
        if self.focus_region == UiRegion::Sidebar {
            if self.sidebar_input_composing() {
                return false;
            }
            if self.is_sidebar_input_focused("chat-input") {
                // Shift+Enter: newline in the textarea (Blitz inserts it).
                if shift {
                    return false;
                }
                self.submit_from_sidebar_input();
                return true;
            }
            if self.is_sidebar_input_focused("chat-api-endpoint")
                || self.is_sidebar_input_focused("chat-api-model")
                || self.is_sidebar_input_focused("chat-api-key")
            {
                self.save_sidebar_settings();
                return true;
            }
            return false;
        }
        if self.focus_region == UiRegion::Chrome {
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
        if self.tabs[self.active_tab].url.starts_with("rust://bookmarks")
            && let Some(q) = self.focused_builtin_input("bm-search")
        {
            let next_url = if q.is_empty() {
                "rust://bookmarks".to_string()
            } else {
                let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
                format!("rust://bookmarks?q={encoded}")
            };
            self.navigate_current(&next_url);
            return true;
        }
        if self.tabs[self.active_tab].url.starts_with("rust://history")
            && let Some(q) = self.focused_builtin_input("hist-search")
        {
            let next_url = if q.is_empty() {
                "rust://history".to_string()
            } else {
                let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
                format!("rust://history?q={encoded}")
            };
            self.navigate_current(&next_url);
            return true;
        }
        false
    }

    /// Keyboard scrolling for the content document (Blitz has no built-in key scrolling).
    fn keyboard_scroll(&mut self, code: KeyCode, shift: bool) -> bool {
        if self.focus_region != UiRegion::Content {
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
            KeyCode::ArrowDown => 60.0,
            KeyCode::ArrowUp => -60.0,
            KeyCode::PageDown => page_h * 0.88,
            KeyCode::PageUp => -page_h * 0.88,
            KeyCode::Space if shift => -page_h * 0.88,
            KeyCode::Space => page_h * 0.88,
            KeyCode::Home => -doc.viewport_scroll().y,
            // Overshoot is clamped by Blitz; the animator stops once the viewport stops moving.
            KeyCode::End => 1.0e7,
            _ => return false,
        };
        // Blitz uses wheel-style deltas: negative scrolls the page down.
        self.queue_smooth_scroll(UiRegion::Content, true, 0.0, -dy);
        true
    }

    /// CSS px one wheel "line" scrolls in `region`: the OS lines-per-notch setting × 100/3 px
    /// (Chromium's step, ~100 px per notch at the Windows default of 3), kept the same physical
    /// distance at any page zoom.
    fn wheel_line_px(&self, region: UiRegion) -> f64 {
        let zoom = if region == UiRegion::Content {
            self.tabs.get(self.active_tab).map_or(1.0, |t| t.zoom.max(0.1) as f64)
        } else {
            1.0
        };
        system_wheel_lines() * (100.0 / 3.0) / zoom
    }

    /// Adds a scroll (wheel-style deltas) to the running glide, or starts one.
    pub(super) fn queue_smooth_scroll(&mut self, region: UiRegion, viewport: bool, dx: f64, dy: f64) {
        let tab_id = (region == UiRegion::Content).then(|| self.tabs[self.active_tab].id);
        match &mut self.smooth_scroll {
            Some(s) if s.region == region && s.viewport == viewport && s.tab_id == tab_id => {
                s.rem_x = accumulate_scroll(s.rem_x, dx);
                s.rem_y = accumulate_scroll(s.rem_y, dy);
            }
            _ => {
                self.smooth_scroll = Some(SmoothScroll {
                    region,
                    viewport,
                    tab_id,
                    rem_x: dx,
                    rem_y: dy,
                    last: Instant::now(),
                });
            }
        }
        self.request_redraw();
    }

    /// One frame of the glide (called from `redraw`). Returns whether more frames are needed.
    pub(super) fn step_smooth_scroll(&mut self) -> bool {
        let Some(mut s) = self.smooth_scroll.take() else {
            return false;
        };
        if s.tab_id
            .is_some_and(|id| self.tabs.get(self.active_tab).map(|t| t.id) != Some(id))
        {
            return false; // switched tabs mid-glide
        }
        let now = Instant::now();
        let dt_ms = now.saturating_duration_since(s.last).as_secs_f64() * 1000.0;
        s.last = now;
        let (px, py) = (glide_portion(s.rem_x, dt_ms), glide_portion(s.rem_y, dt_ms));
        s.rem_x -= px;
        s.rem_y -= py;
        let moved = if s.viewport {
            self.tabs
                .get_mut(self.active_tab)
                .and_then(|t| t.doc.as_mut())
                .is_some_and(|d| d.scroll_viewport_by_has_changed(px, py))
        } else {
            let ev = UiEvent::Wheel(BlitzWheelEvent {
                delta: BlitzWheelDelta::Pixels(px, py),
                coords: self.coords_for_region(self.pointer_pos, s.region),
                buttons: self.buttons,
                mods: self.mods(),
                element: Default::default(),
            });
            self.dispatch_to_region(s.region, ev);
            true
        };
        let more = moved && (s.rem_x != 0.0 || s.rem_y != 0.0);
        if more {
            self.smooth_scroll = Some(s);
        }
        more
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

    fn region_at(&self, pos: PhysicalPosition<f64>) -> UiRegion {
        let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        if l.y < self.chrome_height() as f32 {
            UiRegion::Chrome
        } else if self.sidebar_open && l.x >= self.sidebar_left_logical() {
            UiRegion::Sidebar
        } else {
            UiRegion::Content
        }
    }

    fn coords_for_region(&self, pos: PhysicalPosition<f64>, region: UiRegion) -> PointerCoords {
        match region {
            UiRegion::Chrome => {
                let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
                super::helpers::plain_coords(l.x, l.y, 0.0, 0.0)
            }
            UiRegion::Content => self.content_coords(pos),
            UiRegion::Sidebar => {
                let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
                let scroll = self
                    .sidebar_doc
                    .as_ref()
                    .map_or(blitz_dom::Point { x: 0.0, y: 0.0 }, |d| d.viewport_scroll());
                super::helpers::plain_coords(
                    l.x - self.sidebar_left_logical(),
                    l.y - self.chrome_height() as f32,
                    scroll.x as f32,
                    scroll.y as f32,
                )
            }
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

    /// Sends `ev` to the chrome UI, docked sidebar, or active page.
    fn dispatch_to_region(&mut self, region: UiRegion, ev: UiEvent) {
        match region {
            UiRegion::Chrome => self.dispatch_chrome_ui(ev),
            UiRegion::Sidebar => self.dispatch_sidebar_ui(ev),
            UiRegion::Content => {
                if self.active_tab < self.tabs.len()
                    && let Some(doc) = &mut self.tabs[self.active_tab].doc
                {
                    doc.handle_ui_event(ev);
                }
            }
        }
    }

    /// Keyboard / IME events go to whichever region has focus.
    fn dispatch_focused(&mut self, ev: UiEvent) {
        self.dispatch_to_region(self.focus_region, ev);
    }

    fn send_pointer_leave(&mut self, id: BlitzPointerId, primary: bool, pos: PhysicalPosition<f64>, region: UiRegion) {
        let ev = pointer_event(
            id,
            primary,
            self.coords_for_region(pos, region),
            Default::default(),
            self.buttons,
            self.mods(),
            PointerDetails::default(),
        );
        self.dispatch_to_region(region, UiEvent::PointerLeave(ev));
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
                    || el
                        .attr(LocalName::from("class"))
                        .is_some_and(|c| c.split_whitespace().any(|k| k == "omni"))
                {
                    return true;
                }
            }
            node_id = node.parent;
        }
        false
    }

    /// True when `pos` lands directly on the `<input id="url-bar">` box (not `.omni` padding/icons).
    fn is_exact_url_input_at(&self, pos: PhysicalPosition<f64>) -> bool {
        let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
        let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") else {
            return false;
        };
        self.chrome_doc.element_from_point(l.x, l.y) == Some(url_id)
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
                if let Some(el) = node.element_data()
                    && &*el.name.local == "a"
                    && let Some(raw) = el.attr(LocalName::from("href"))
                {
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
            if let Some(el) = node.element_data()
                && let Some(act) = el.attr(LocalName::from("data-action"))
            {
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
            node_id = node.parent;
        }
        None
    }

    /// Reorders tabs horizontally within the tab strip, or switches to tear-off mode when
    /// dragged outside the tab strip (and not over a bookmark folder) to detach into a new window.
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
        let scale = self.hidpi_scale.max(1.0) as f64;
        let lx = pos.x / scale;
        let ly = pos.y / scale;
        let win_w = self.win_size.width as f64 / scale;

        // Hovering a bookmark folder while dragging an http(s) tab: let bookmark D&D handle it.
        if self.bm_drag.is_some() && self.bm_folder_at(pos).is_some() {
            let was_detaching = self.tab_drag.as_ref().is_some_and(|d| d.detaching);
            if let Some(d) = self.tab_drag.as_mut() {
                d.active = true;
                d.detaching = false;
            }
            if was_detaching {
                self.rebuild_chrome();
            }
            return false;
        }

        // Dragged outside the tab strip: tear off into a new window (or move window for a lone tab).
        if is_tab_detach_pos(lx, ly, win_w) {
            if self.tabs.len() <= 1 {
                self.tab_drag = None;
                self.bm_drag = None;
                self.buttons = MouseEventButtons::None;
                if let Some(win) = &self.window {
                    win.set_cursor(Cursor::Icon(CursorIcon::Default));
                    let _ = win.drag_window();
                }
                return true;
            }
            let bm_had_hover = self.bm_drag.as_mut().is_some_and(|b| b.hover_folder.take().is_some());
            let was_detaching = self.tab_drag.as_ref().is_some_and(|d| d.detaching);
            if let Some(d) = self.tab_drag.as_mut() {
                d.active = true;
                d.detaching = true;
            }
            if let Some(win) = &self.window {
                win.set_cursor(Cursor::Icon(CursorIcon::Move));
            }
            if !was_detaching || bm_had_hover {
                if bm_had_hover && self.tabs[self.active_tab].url.starts_with("rust://bookmarks") {
                    let scroll = self.current_scroll_y();
                    self.load_active_tab(false, Some(scroll));
                } else {
                    self.rebuild_chrome();
                }
            }
            return true;
        }

        let tab_id = drag.tab_id;
        let was_detaching = drag.detaching;
        if let Some(d) = self.tab_drag.as_mut() {
            d.active = true;
            d.detaching = false;
        }
        let bm_had_hover = self.bm_drag.as_mut().is_some_and(|b| b.hover_folder.take().is_some());
        if was_detaching || bm_had_hover {
            if let Some(win) = &self.window {
                win.set_cursor(Cursor::Icon(CursorIcon::Default));
            }
            self.rebuild_chrome();
        }
        let Some(cur_idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return true;
        };
        if self.active_tab != cur_idx {
            self.switch_tab(cur_idx);
        }
        if let Some((over_idx, tab_x, tab_w)) = self.tab_hit_at_x(lx as f32)
            && over_idx != cur_idx
            && over_idx < self.tabs.len()
        {
            let should_swap = if over_idx > cur_idx {
                (lx as f32) > tab_x + tab_w * 0.35
            } else {
                (lx as f32) < tab_x + tab_w * 0.65
            };
            if should_swap {
                let tab = self.tabs.remove(cur_idx);
                self.tabs.insert(over_idx, tab);
                self.active_tab = over_idx;
                self.rebuild_chrome();
            }
        }
        true
    }

    /// Finds a draggable bookmark/tab (`data-bm-src`) under `pos`.
    fn bm_source_at(&self, pos: PhysicalPosition<f64>) -> Option<(String, String)> {
        match self.region_at(pos) {
            UiRegion::Chrome => {
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
                            let title = el.attr(LocalName::from("data-bm-title")).unwrap_or(src).to_string();
                            return Some((src.to_string(), title));
                        }
                    }
                    node_id = node.parent;
                }
                None
            }
            UiRegion::Content => {
                if self.active_webview.is_some() {
                    return None;
                }
                let doc = self.tabs[self.active_tab].doc.as_ref()?;
                let c = self.content_coords(pos);
                let mut node_id = doc.element_from_point(c.client_x, c.client_y);
                while let Some(id) = node_id {
                    let node = doc.get_node(id)?;
                    if let Some(el) = node.element_data()
                        && let Some(src) = el.attr(LocalName::from("data-bm-src"))
                    {
                        let title = el.attr(LocalName::from("data-bm-title")).unwrap_or(src).to_string();
                        return Some((src.to_string(), title));
                    }
                    node_id = node.parent;
                }
                None
            }
            UiRegion::Sidebar => None,
        }
    }

    /// Finds a drop-target bookmark folder (`data-bm-folder`) under `pos`.
    fn bm_folder_at(&self, pos: PhysicalPosition<f64>) -> Option<String> {
        if pos.x < 0.0 || pos.y < 0.0 || pos.x >= self.win_size.width as f64 || pos.y >= self.win_size.height as f64 {
            return None;
        }
        match self.region_at(pos) {
            UiRegion::Chrome => {
                let l: LogicalPosition<f32> = pos.to_logical(self.hidpi_scale as f64);
                let mut node_id = self.chrome_doc.element_from_point(l.x, l.y);
                while let Some(id) = node_id {
                    let node = self.chrome_doc.get_node(id)?;
                    if let Some(el) = node.element_data()
                        && let Some(folder) = el.attr(LocalName::from("data-bm-folder"))
                    {
                        return Some(folder.to_string());
                    }
                    node_id = node.parent;
                }
                None
            }
            UiRegion::Content => {
                if self.active_webview.is_some() {
                    return None;
                }
                let doc = self.tabs[self.active_tab].doc.as_ref()?;
                let c = self.content_coords(pos);
                let mut node_id = doc.element_from_point(c.client_x, c.client_y);
                while let Some(id) = node_id {
                    let node = doc.get_node(id)?;
                    if let Some(el) = node.element_data()
                        && let Some(folder) = el.attr(LocalName::from("data-bm-folder"))
                    {
                        return Some(folder.to_string());
                    }
                    node_id = node.parent;
                }
                None
            }
            UiRegion::Sidebar => None,
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

/// True when a dragged tab's logical pointer position `(lx, ly)` has left the tab strip
/// far enough to detach into a new browser window.
fn is_tab_detach_pos(lx: f64, ly: f64, win_w: f64) -> bool {
    ly > (TITLEBAR_HEIGHT + 24) as f64 || ly < -24.0 || lx < -24.0 || lx > win_w + 24.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_detach_zone_triggers_only_outside_tab_strip() {
        assert!(!is_tab_detach_pos(200.0, 20.0, 1280.0));
        assert!(!is_tab_detach_pos(200.0, 56.0, 1280.0));
        assert!(!is_tab_detach_pos(0.0, -10.0, 1280.0));
        assert!(is_tab_detach_pos(200.0, 70.0, 1280.0));
        assert!(is_tab_detach_pos(200.0, -30.0, 1280.0));
        assert!(is_tab_detach_pos(-30.0, 20.0, 1280.0));
        assert!(is_tab_detach_pos(1310.0, 20.0, 1280.0));
    }
}

/// A wheel-notch or keyboard scroll gliding to its target over a few frames.
#[derive(Debug, Clone)]
pub(super) struct SmoothScroll {
    region: UiRegion,
    /// Keyboard scrolls move the page viewport; wheel glides go to whatever is under the pointer.
    viewport: bool,
    /// Content glides belong to one tab and stop if the user switches away.
    tab_id: Option<u64>,
    rem_x: f64,
    rem_y: f64,
    last: Instant,
}

/// Glide time constant: ~95% of a notch lands within 3τ ≈ 120 ms, then it eases to a stop.
const GLIDE_TAU_MS: f64 = 40.0;

/// Portion of `remaining` to scroll this frame (exponential ease-out); the tail under half a
/// pixel is taken at once so the glide ends exactly on target.
fn glide_portion(remaining: f64, dt_ms: f64) -> f64 {
    let dt = dt_ms.clamp(1.0, 50.0); // a stalled frame shouldn't jump the whole way
    let step = remaining * (1.0 - (-dt / GLIDE_TAU_MS).exp());
    if (remaining - step).abs() < 0.5 {
        remaining
    } else {
        step
    }
}

/// Repeated notches in one direction add up (fast wheel spins accelerate); reversing direction
/// drops what was left, so the page turns around immediately.
fn accumulate_scroll(remaining: f64, delta: f64) -> f64 {
    if delta != 0.0 && remaining != 0.0 && remaining.signum() != delta.signum() {
        delta
    } else {
        remaining + delta
    }
}

/// Lines per wheel notch from the OS ("마우스 → 한 번에 스크롤할 줄 수"); "one screen at a time"
/// maps to a long jump. Non-Windows wheels already report accelerated line counts.
fn system_wheel_lines() -> f64 {
    #[cfg(windows)]
    {
        const SPI_GETWHEELSCROLLLINES: u32 = 0x0068;
        const WHEEL_PAGESCROLL: u32 = u32::MAX;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SystemParametersInfoW(action: u32, param: u32, pv: *mut core::ffi::c_void, win_ini: u32) -> i32;
        }
        let mut lines: u32 = 3;
        // SAFETY: SPI_GETWHEELSCROLLLINES writes one UINT to the provided pointer.
        let ok = unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0) };
        match (ok, lines) {
            (0, _) | (_, 0) => 3.0,
            (_, WHEEL_PAGESCROLL) => 20.0,
            (_, n) => f64::from(n.min(100)),
        }
    }
    #[cfg(not(windows))]
    {
        1.0
    }
}

#[cfg(test)]
mod smooth_scroll_tests {
    use super::*;

    #[test]
    fn a_notch_glides_to_exactly_its_target_in_about_120ms() {
        let (mut remaining, mut scrolled, mut t) = (100.0_f64, 0.0_f64, 0.0_f64);
        let mut first = None;
        while remaining != 0.0 {
            let step = glide_portion(remaining, 16.0);
            first.get_or_insert(step);
            assert!(step.abs() <= remaining.abs() + 1e-9 && step.signum() == remaining.signum());
            scrolled += step;
            remaining -= step;
            t += 16.0;
            assert!(t < 400.0, "glide must finish");
        }
        assert!((scrolled - 100.0).abs() < 1e-9, "lands on target: {scrolled}");
        assert!(
            first.unwrap() > 25.0,
            "responds immediately: first frame moves a big chunk"
        );
        assert!(t <= 240.0, "finished in {t} ms");
    }

    #[test]
    fn notches_accumulate_and_reversal_turns_around() {
        assert_eq!(accumulate_scroll(-60.0, -100.0), -160.0);
        assert_eq!(accumulate_scroll(-60.0, 100.0), 100.0);
        assert_eq!(accumulate_scroll(0.0, 100.0), 100.0);
        assert_eq!(
            accumulate_scroll(-60.0, 0.0),
            -60.0,
            "an axis without input keeps gliding"
        );
    }

    #[test]
    fn stalled_frame_does_not_jump_to_the_end() {
        assert!(glide_portion(1000.0, 5000.0) < 1000.0);
    }

    #[test]
    fn wheel_lines_setting_is_sane() {
        let n = system_wheel_lines();
        assert!((1.0..=100.0).contains(&n), "{n}");
    }
}
