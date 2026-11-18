//! Glue between Blitz/wry callbacks and the event loop: everything funnels into `AppEvent`s.

use super::AppEvent;
use super::helpers::ACTION_MOD;
use blitz_dom::{Document, EventHandler, LocalName, NodeId};
use blitz_traits::events::{DomEvent, DomEventData, EventState, MouseEventButton};
use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::NetWaker;
use blitz_traits::shell::{ClipboardError, ShellProvider};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use winit::cursor::{Cursor, CursorIcon};
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

#[derive(Clone)]
pub(super) struct AppBus {
    pub(super) tx: Sender<AppEvent>,
    pub(super) winit_proxy: EventLoopProxy,
}

impl AppBus {
    pub(super) fn send(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.winit_proxy.wake_up();
    }
}

impl NetWaker for AppBus {
    fn wake(&self, client_id: usize) {
        self.send(AppEvent::RedrawDoc { doc_id: client_id });
    }
}

pub(super) struct BrowserNavProvider {
    pub(super) bus: AppBus,
}

impl NavigationProvider for BrowserNavProvider {
    fn navigate_to(&self, options: NavigationOptions) {
        let source_doc_id = options.source_document;
        let url = options.url.clone();
        let request = (options.method != http::Method::GET).then(|| options.into_request());
        self.bus.send(AppEvent::Navigate {
            source_doc_id,
            url,
            request,
        });
    }
}

pub(super) struct BrowserShellProvider {
    pub(super) window: Arc<dyn Window>,
}

impl ShellProvider for BrowserShellProvider {
    fn request_redraw(&self) {
        self.window.request_redraw();
    }

    fn set_cursor(&self, icon: Option<CursorIcon>) {
        // Blitz reports `None` both for `cursor: none` and for "no element under the pointer"
        // (e.g. the empty area below a short page). Hiding the cursor made it vanish near the
        // bottom of the window, so `None` shows the default arrow instead.
        self.window.set_cursor(Cursor::Icon(icon.unwrap_or(CursorIcon::Default)));
    }

    fn set_window_title(&self, _title: String) {}

    fn set_ime_enabled(&self, is_enabled: bool) {
        use winit::window::{ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData};
        if is_enabled {
            let _ = self.window.request_ime_update(ImeRequest::Enable(
                ImeEnableRequest::new(ImeCapabilities::new(), ImeRequestData::default()).unwrap(),
            ));
        } else {
            let _ = self.window.request_ime_update(ImeRequest::Disable);
        }
    }

    fn set_ime_cursor_area(&self, x: f32, y: f32, width: f32, height: f32) {
        use winit::window::{ImeRequest, ImeRequestData};
        let _ = self.window.request_ime_update(ImeRequest::Update(
            ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(x, y).into(),
                LogicalSize::new(width, height).into(),
            ),
        ));
    }

    fn get_clipboard_text(&self) -> Result<String, ClipboardError> {
        let mut cb = arboard::Clipboard::new().map_err(|_| ClipboardError)?;
        cb.get_text().map_err(|_| ClipboardError)
    }

    fn set_clipboard_text(&self, text: String) -> Result<(), ClipboardError> {
        let mut cb = arboard::Clipboard::new().map_err(|_| ClipboardError)?;
        cb.set_text(text).map_err(|_| ClipboardError)
    }
}

/// First `data-action` attribute on the event's target chain (innermost first).
fn find_action(chain: &[NodeId], doc: &dyn Document) -> Option<String> {
    let inner = doc.inner();
    chain.iter().find_map(|&id| {
        inner
            .get_node(id)?
            .element_data()?
            .attr(LocalName::from("data-action"))
            .map(str::to_string)
    })
}

/// Browser UI (tab strip / toolbar / bookmark bar): clicks on `data-action` elements.
/// Enter in the URL bar is handled before dispatch, in the keyboard shortcut code.
pub(super) struct ChromeEventHandler {
    pub(super) bus: AppBus,
}

impl EventHandler for ChromeEventHandler {
    fn handle_event(
        &mut self,
        chain: &[NodeId],
        event: &mut DomEvent,
        doc: &mut dyn Document,
        event_state: &mut EventState,
    ) {
        if let DomEventData::Click(click) = &event.data {
            if click.button == MouseEventButton::Main {
                if let Some(action) = find_action(chain, doc) {
                    self.bus.send(AppEvent::ChromeAction(action));
                    event_state.stop_propagation();
                }
            }
        }
    }
}

/// Bookmark folder popup: click opens in place, Ctrl/Cmd+click or middle-click opens a tab.
pub(super) struct PopupEventHandler {
    pub(super) bus: AppBus,
}

impl EventHandler for PopupEventHandler {
    fn handle_event(
        &mut self,
        chain: &[NodeId],
        event: &mut DomEvent,
        doc: &mut dyn Document,
        event_state: &mut EventState,
    ) {
        let new_tab = match &event.data {
            DomEventData::Click(click) if click.button == MouseEventButton::Main => {
                click.mods.contains(ACTION_MOD)
            }
            DomEventData::PointerUp(ptr) if ptr.button == MouseEventButton::Auxiliary => true,
            _ => return,
        };
        if let Some(action) = find_action(chain, doc) {
            self.bus.send(AppEvent::PopupAction { action, new_tab });
            event_state.stop_propagation();
        }
    }
}
