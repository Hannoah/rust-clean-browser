use crate::bookmarks::{BookmarkNode, BookmarkRootKind, BookmarkStore};
use crate::chrome_ui::{
    CHROME_HEIGHT, ChromeRenderState, POPUP_WIDTH, TITLEBAR_HEIGHT, estimate_popup_height,
    render_bookmark_popup_html, render_bookmarks_page, render_chrome_html, render_error_page,
    render_home_page,
};
use crate::cleaner::{build_reader_html, extract_title, process_fetched_document};
use crate::events::{
    button_source_to_blitz, pointer_kind_to_blitz, pointer_source_to_blitz,
    pointer_source_to_blitz_details, theme_to_color_scheme, winit_ime_to_blitz,
    winit_key_event_to_blitz, winit_modifiers_to_kbt_modifiers,
};
use crate::memory::{current_process_memory, trim_process_working_set};
use crate::net::{CleanNetProvider, PersistentCookieJar};
use crate::page_cache::DiskPageCache;
use crate::tabs::{HistoryEntry, HtmlCache, Tab, short_title_for_url};

use anyrender::{PaintScene, WindowRenderer};
use anyrender_vello_cpu::VelloCpuWindowRenderer;
use blitz_dom::{
    Document, DocumentConfig, EventDriver, EventHandler, FontContext, LocalName, NodeId,
    StyleThreading,
};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::events::{
    BlitzKeyEvent, BlitzPointerEvent, BlitzWheelDelta, BlitzWheelEvent, DomEvent, DomEventData,
    EventState, KeyState, MouseEventButton, MouseEventButtons, PointerCoords, PointerDetails,
    UiEvent,
};
use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::{AbortController, NetWaker, Request};
use blitz_traits::shell::{ClipboardError, ColorScheme, ShellProvider, Viewport};
use keyboard_types::{Code, Key, Location, Modifiers as KbtModifiers, NamedKey};
use kurbo::{Affine, Rect};
use peniko::Fill;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;
use url::Url;
use winit::application::ApplicationHandler;
use winit::cursor::{Cursor, CursorIcon};
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ButtonSource, ElementState, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{ResizeDirection, Theme, Window, WindowAttributes, WindowId, WindowLevel};
use wry::{NewWindowResponse, PageLoadEvent, Rect as WryRect, WebContext, WebView, WebViewBuilder};

#[derive(Debug)]
pub enum AppEvent {
    RedrawDoc {
        doc_id: usize,
    },
    PageLoaded {
        tab_id: u64,
        load_seq: u64,
        /// URL as requested (before redirects); also used as a disk-cache key.
        requested_url: String,
        /// False for form POST results, which must not be cached.
        cacheable: bool,
        final_url: String,
        clean_html: Arc<str>,
        is_spa_shell: bool,
        restore_scroll_y: Option<f64>,
    },
    PageLoadFailed {
        tab_id: u64,
        load_seq: u64,
        url: String,
        error: String,
    },
    Navigate {
        source_doc_id: usize,
        url: Url,
        /// Set for non-GET navigations (form POST).
        request: Option<Request>,
    },
    WebViewTitleChanged {
        tab_id: u64,
        title: String,
    },
    WebViewPageLoad {
        tab_id: u64,
        url: String,
        finished: bool,
    },
    OpenInNewTab(String),
    /// A queued site-icon download finished (`ok`: stored in the favicon cache).
    FaviconDone { ok: bool },
    ChromeAction(String),
    SubmitUrlBar,
    OpenBookmarkPopup {
        key: String,
        root: BookmarkRootKind,
        path: Vec<usize>,
        index_offset: usize,
        title: String,
    },
    PopupAction {
        action: String,
        new_tab: bool,
    },
}

#[derive(Clone)]
struct AppBus {
    tx: Sender<AppEvent>,
    winit_proxy: EventLoopProxy,
}

impl AppBus {
    fn send(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.winit_proxy.wake_up();
    }
}

impl NetWaker for AppBus {
    fn wake(&self, client_id: usize) {
        self.send(AppEvent::RedrawDoc { doc_id: client_id });
    }
}

struct BrowserNavProvider {
    bus: AppBus,
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

struct BrowserShellProvider {
    window: Arc<dyn Window>,
}

impl ShellProvider for BrowserShellProvider {
    fn request_redraw(&self) {
        self.window.request_redraw();
    }

    fn set_cursor(&self, icon: Option<CursorIcon>) {
        match icon {
            Some(icon) => {
                self.window.set_cursor_visible(true);
                self.window.set_cursor(Cursor::Icon(icon));
            }
            None => {
                self.window.set_cursor(Cursor::Icon(CursorIcon::Default));
                self.window.set_cursor_visible(false);
            }
        }
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

struct ChromeEventHandler {
    bus: AppBus,
}

impl EventHandler for ChromeEventHandler {
    fn handle_event(
        &mut self,
        chain: &[NodeId],
        event: &mut DomEvent,
        doc: &mut dyn Document,
        event_state: &mut EventState,
    ) {
        match &event.data {
            DomEventData::Click(click) if click.button == MouseEventButton::Main => {
                let inner = doc.inner();
                for &node_id in chain {
                    if let Some(el) = inner.get_node(node_id).and_then(|n| n.element_data()) {
                        if let Some(action) = el.attr(LocalName::from("data-action")) {
                            self.bus.send(AppEvent::ChromeAction(action.to_string()));
                            event_state.stop_propagation();
                            return;
                        }
                    }
                }
            }
            DomEventData::KeyDown(key_ev) if key_ev.key == Key::Named(NamedKey::Enter) => {
                let inner = doc.inner();
                if let Some(url_id) = inner.get_element_by_id("url-bar") {
                    if inner.get_focussed_node_id() == Some(url_id) {
                        self.bus.send(AppEvent::SubmitUrlBar);
                        event_state.stop_propagation();
                    }
                }
            }
            _ => {}
        }
    }
}

struct PopupEventHandler {
    bus: AppBus,
}

impl EventHandler for PopupEventHandler {
    fn handle_event(
        &mut self,
        chain: &[NodeId],
        event: &mut DomEvent,
        doc: &mut dyn Document,
        event_state: &mut EventState,
    ) {
        let mut emit_if_action = |new_tab: bool| {
            let inner = doc.inner();
            for &node_id in chain {
                if let Some(el) = inner.get_node(node_id).and_then(|n| n.element_data()) {
                    if let Some(action) = el.attr(LocalName::from("data-action")) {
                        self.bus.send(AppEvent::PopupAction {
                            action: action.to_string(),
                            new_tab,
                        });
                        event_state.stop_propagation();
                        return true;
                    }
                }
            }
            false
        };

        match &event.data {
            DomEventData::Click(click) if click.button == MouseEventButton::Main => {
                let new_tab = click.mods.contains(ACTION_MOD);
                emit_if_action(new_tab);
            }
            DomEventData::PointerUp(ptr) if ptr.button == MouseEventButton::Auxiliary => {
                emit_if_action(true);
            }
            _ => {}
        }
    }
}

struct BookmarkPopup {
    key: String,
    root: BookmarkRootKind,
    path: Vec<usize>,
    base_depth: usize,
    root_title: String,
    index_offset: usize,
    window: Arc<dyn Window>,
    renderer: VelloCpuWindowRenderer,
    doc: HtmlDocument,
    win_size: PhysicalSize<u32>,
    hidpi_scale: f32,
    pointer_pos: PhysicalPosition<f64>,
    buttons: MouseEventButtons,
}

pub struct BrowserApp {
    window: Option<Arc<dyn Window>>,
    renderer: VelloCpuWindowRenderer,
    bus: AppBus,
    rx: Receiver<AppEvent>,

    shared_font_ctx: FontContext,
    net_provider: Arc<CleanNetProvider>,
    nav_provider: Arc<BrowserNavProvider>,
    shell_provider: Option<Arc<dyn ShellProvider>>,

    chrome_doc: HtmlDocument,
    tabs: Vec<Tab>,
    active_tab: usize,
    next_tab_id: u64,
    bookmarks: BookmarkStore,
    html_cache: HtmlCache,
    page_cache: DiskPageCache,
    /// Pending favicon downloads, fetched one at a time while no page is loading.
    favicon_queue: std::collections::VecDeque<(String, Url)>,
    favicon_inflight: bool,

    win_size: PhysicalSize<u32>,
    hidpi_scale: f32,
    color_scheme: ColorScheme,
    pointer_pos: PhysicalPosition<f64>,
    pointer_in_chrome: Option<bool>,
    buttons: MouseEventButtons,
    keyboard_modifiers: Modifiers,
    chrome_focused: bool,
    start_instant: Instant,
    status_note: Option<String>,
    web_context: Option<WebContext>,
    active_webview: Option<WebView>,
    popup: Option<BookmarkPopup>,
    recently_closed_popup: Option<(String, Instant)>,
    last_chrome_click_x_logical: f32,
    /// Last chrome HTML; identical rebuilds are skipped (no re-parse / re-style).
    last_chrome_html: String,
    /// URLs of recently closed tabs (Ctrl+Shift+T), newest last.
    closed_tabs: Vec<String>,
    last_titlebar_press: Option<Instant>,
    resize_hover: Option<ResizeDirection>,
    maximized: bool,
}

impl BrowserApp {
    pub fn new(
        winit_proxy: EventLoopProxy,
        initial_url: Option<String>,
        bookmarks: std::thread::JoinHandle<BookmarkStore>,
    ) -> Self {
        let (tx, rx) = channel();
        let bus = AppBus { tx, winit_proxy };
        let shared_font_ctx = FontContext::default();
        let cookie_jar = Arc::new(PersistentCookieJar::load(
            crate::bookmarks::app_data_dir().map(|p| p.join("cookies.json")),
        ));
        let net_provider = Arc::new(CleanNetProvider::new(Some(Arc::new(bus.clone())), cookie_jar));
        let nav_provider = Arc::new(BrowserNavProvider { bus: bus.clone() });
        let bookmarks = bookmarks.join().unwrap_or_default();

        let start_url = initial_url
            .map(|u| {
                // Local files (e.g. opened via the .html file association) become file:// URLs.
                let path = std::path::Path::new(&u);
                match std::path::absolute(path).ok().filter(|p| p.is_file()) {
                    Some(abs) => Url::from_file_path(abs).map(String::from).unwrap_or(u),
                    None => normalize_url_input(&u),
                }
            })
            .unwrap_or_else(|| "rust://home".to_string());
        let first_tab = Tab::new(1, start_url);
        let tabs = vec![first_tab];

        let chrome_doc = HtmlDocument::from_html(
            "<html></html>",
            DocumentConfig {
                font_ctx: Some(shared_font_ctx.clone()),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );

        let mut app = Self {
            window: None,
            renderer: VelloCpuWindowRenderer::new(),
            bus,
            rx,
            shared_font_ctx,
            net_provider,
            nav_provider,
            shell_provider: None,
            chrome_doc,
            tabs,
            active_tab: 0,
            next_tab_id: 2,
            bookmarks,
            html_cache: HtmlCache::new(8, 2 * 1024 * 1024),
            page_cache: DiskPageCache::new(crate::bookmarks::app_data_dir().map(|p| p.join("pages"))),
            favicon_queue: std::collections::VecDeque::new(),
            favicon_inflight: false,
            win_size: PhysicalSize::new(1280, 820),
            hidpi_scale: 1.0,
            color_scheme: ColorScheme::Light,
            pointer_pos: PhysicalPosition::new(0.0, 0.0),
            pointer_in_chrome: None,
            buttons: MouseEventButtons::None,
            keyboard_modifiers: Modifiers::default(),
            chrome_focused: false,
            start_instant: Instant::now(),
            status_note: None,
            web_context: None,
            active_webview: None,
            popup: None,
            recently_closed_popup: None,
            last_chrome_click_x_logical: 24.0,
            last_chrome_html: String::new(),
            closed_tabs: Vec::new(),
            last_titlebar_press: None,
            resize_hover: None,
            maximized: false,
        };

        app.load_active_tab(false, None);
        app
    }

    fn anim_time(&self) -> f64 {
        self.start_instant.elapsed().as_secs_f64()
    }

    fn chrome_phys_height(&self) -> u32 {
        ((CHROME_HEIGHT as f32) * self.hidpi_scale).round() as u32
    }

    fn content_phys_size(&self) -> (u32, u32) {
        let ch = self.chrome_phys_height();
        (self.win_size.width.max(1), self.win_size.height.saturating_sub(ch).max(1))
    }

    fn chrome_viewport(&self) -> Viewport {
        let ch = self.chrome_phys_height();
        Viewport::new(self.win_size.width.max(1), ch.max(1), self.hidpi_scale, self.color_scheme)
    }

    fn content_viewport(&self, zoom: f32) -> Viewport {
        let (cw, ch) = self.content_phys_size();
        let mut vp = Viewport::new(cw, ch, self.hidpi_scale, self.color_scheme);
        vp.zoom = zoom;
        vp
    }

    fn url_bar_focused(&self) -> bool {
        self.chrome_focused
            && self.chrome_doc.get_element_by_id("url-bar").is_some()
            && self.chrome_doc.get_element_by_id("url-bar") == self.chrome_doc.get_focussed_node_id()
    }

    /// Queues a one-time favicon download for `page_url`'s host (skipped if cached or known
    /// to have no icon). Downloads run one at a time so they never compete with page loads.
    fn request_favicon(&mut self, page_url: &str, declared_icon: Option<&str>) {
        if let Some(job) = crate::favicon::begin_fetch(page_url, declared_icon) {
            self.favicon_queue.push_back(job);
        }
    }

    fn pump_favicons(&mut self) {
        if self.favicon_inflight || self.tabs[self.active_tab].is_loading {
            return;
        }
        let Some((host, icon_url)) = self.favicon_queue.pop_front() else {
            return;
        };
        self.favicon_inflight = true;
        let bus_ok = self.bus.clone();
        let bus_fail = self.bus.clone();
        let fail_host = host.clone();
        self.net_provider.fetch_document(
            Request::get(icon_url),
            AbortController::default().signal,
            move |_, bytes| {
                let ok = crate::favicon::store(&host, &bytes);
                if !ok {
                    crate::favicon::mark_failed(&host);
                }
                bus_ok.send(AppEvent::FaviconDone { ok });
            },
            move |_| {
                crate::favicon::mark_failed(&fail_host);
                bus_fail.send(AppEvent::FaviconDone { ok: false });
            },
        );
    }

    fn rebuild_chrome(&mut self) {
        // Icons for open tabs and the bookmark bar (deduplicated per host, fetched once).
        for i in 0..self.tabs.len() {
            let url = self.tabs[i].url.clone();
            self.request_favicon(&url, None);
        }
        let bar_urls: Vec<String> = self
            .bookmarks
            .custom
            .iter()
            .map(|b| b.url.clone())
            .chain(self.bookmarks.bar.iter().filter_map(|n| match n {
                BookmarkNode::Url(item) => Some(item.url.clone()),
                BookmarkNode::Folder(_) => None,
            }))
            .take(16)
            .collect();
        for url in &bar_urls {
            self.request_favicon(url, None);
        }
        self.pump_favicons();
        let mem = current_process_memory();
        // Keep whatever the user is typing (and focus) when background events rebuild the chrome.
        let typing = self.url_bar_focused().then(|| self.read_url_bar_text()).flatten();
        let html = render_chrome_html(&ChromeRenderState {
            tabs: &self.tabs,
            active_idx: self.active_tab,
            bookmarks: &self.bookmarks,
            working_set_mb: mem.working_set_mb,
            blocked_count: self.net_provider.blocked_total(),
            filter_enabled: self.net_provider.is_filter_enabled(),
            status_note: self.status_note.as_deref(),
            maximized: self.maximized,
            url_override: typing.as_deref(),
        });
        self.update_window_title();
        if html == self.last_chrome_html {
            return;
        }

        let mut cfg = DocumentConfig {
            viewport: Some(self.chrome_viewport()),
            font_ctx: Some(self.shared_font_ctx.clone()),
            net_provider: Some(self.net_provider.clone()),
            style_threading: StyleThreading::Sequential,
            ..Default::default()
        };
        if let Some(shell) = &self.shell_provider {
            cfg.shell_provider = Some(Arc::clone(shell));
        }
        let mut doc = HtmlDocument::from_html(&html, cfg);
        wait_for_images(&mut doc);
        self.chrome_doc = doc;
        self.last_chrome_html = html;
        if typing.is_some() {
            if let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") {
                self.chrome_doc.set_focus_to(url_id);
                self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(Key::Named(NamedKey::End), Code::End, KbtModifiers::empty())));
            }
        }
        self.request_redraw();
    }

    fn update_window_title(&self) {
        if let Some(win) = &self.window {
            let tab = &self.tabs[self.active_tab];
            win.set_title(&format!("{} — Rust Clean Browser", tab.title));
        }
    }

    fn request_redraw(&self) {
        if let Some(win) = &self.window {
            win.request_redraw();
        }
    }

    fn build_content_doc(&self, html: &str, base_url: &str, abort_ctrl: Option<&AbortController>, zoom: f32) -> HtmlDocument {
        let mut cfg = DocumentConfig {
            viewport: Some(self.content_viewport(zoom)),
            base_url: Some(base_url.to_string()),
            net_provider: Some(self.net_provider.clone()),
            navigation_provider: Some(self.nav_provider.clone()),
            font_ctx: Some(self.shared_font_ctx.clone()),
            style_threading: StyleThreading::Sequential,
            abort_signal: abort_ctrl.map(|c| c.signal.clone()),
            ..Default::default()
        };
        if let Some(shell) = &self.shell_provider {
            cfg.shell_provider = Some(Arc::clone(shell));
        }
        HtmlDocument::from_html(html, cfg)
    }

    fn apply_html_to_tab(
        &mut self,
        tab_idx: usize,
        final_url: String,
        clean_html: Arc<str>,
        restore_scroll_y: Option<f64>,
    ) {
        let title = extract_title(&clean_html).unwrap_or_else(|| short_title_for_url(&final_url));
        let zoom = self.tabs[tab_idx].zoom;
        // Drop the previous DOM before building the next one so peak memory holds one page.
        self.tabs[tab_idx].doc = None;
        let abort_ctrl = AbortController::default();
        let mut doc = if self.tabs[tab_idx].reader_mode {
            let reader = build_reader_html(&clean_html, &final_url);
            self.build_content_doc(&reader, &final_url, Some(&abort_ctrl), zoom)
        } else {
            self.build_content_doc(&clean_html, &final_url, Some(&abort_ctrl), zoom)
        };
        if let Some(y) = restore_scroll_y {
            doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y });
        } else if let Some((_, frag)) = final_url.split_once('#') {
            if !frag.is_empty() {
                doc.resolve(self.anim_time());
                doc.scroll_to_fragment(frag);
            }
        }

        let declared_icon = doc.favicon_url();
        self.request_favicon(&final_url, declared_icon.as_deref());
        let tab = &mut self.tabs[tab_idx];
        tab.url = final_url.clone();
        tab.title = title.clone();
        tab.is_loading = false;
        tab.abort_controller = Some(abort_ctrl);
        tab.raw_html = Some(Arc::clone(&clean_html));
        tab.doc = Some(doc);

        if !final_url.starts_with("rust://") {
            let cache_url = final_url.split('#').next().unwrap_or(&final_url).to_string();
            self.html_cache.insert(cache_url, title, clean_html);
        }

        self.rebuild_chrome();
    }

    fn webview_bounds(&self) -> WryRect {
        let scale = self.hidpi_scale as f64;
        let logical_w = ((self.win_size.width as f64) / scale).round().max(1.0) as u32;
        let logical_h = ((self.win_size.height as f64) / scale).round().max(1.0) as u32;
        // Leave a thin frame around the child WebView so window edges stay resizable.
        let inset = if self.maximized || !cfg!(target_os = "windows") { 0 } else { RESIZE_EDGE };
        let content_h = logical_h.saturating_sub(CHROME_HEIGHT + inset).max(1);
        WryRect {
            position: dpi::LogicalPosition::new(inset as i32, CHROME_HEIGHT as i32).into(),
            size: dpi::LogicalSize::new(logical_w.saturating_sub(inset * 2).max(1), content_h).into(),
        }
    }

    fn destroy_active_webview(&mut self) {
        if let Some(wv) = self.active_webview.take() {
            persist_session_cookies(&wv);
            drop(wv);
            trim_process_working_set();
        }
    }

    fn mount_or_navigate_webview(&mut self, tab_idx: usize) {
        let url = self.tabs[tab_idx].url.clone();
        if url.starts_with("rust://") {
            self.destroy_active_webview();
            return;
        }
        let tab_id = self.tabs[tab_idx].id;
        self.tabs[tab_idx].unload();
        self.tabs[tab_idx].js_mode = true;

        if let Some(wv) = &self.active_webview {
            let _ = wv.set_bounds(self.webview_bounds());
            let _ = wv.load_url(&url);
            return;
        }

        let Some(win) = self.window.as_ref().map(Arc::clone) else {
            return;
        };

        if self.web_context.is_none() {
            let data_dir = crate::bookmarks::app_data_dir().map(|p| p.join("webview2"));
            self.web_context = Some(WebContext::new(data_dir));
        }

        let bounds = self.webview_bounds();
        let bus_title = self.bus.clone();
        let bus_load = self.bus.clone();
        let bus_new = self.bus.clone();

        let builder = if let Some(ctx) = self.web_context.as_mut() {
            WebViewBuilder::new_with_web_context(ctx)
        } else {
            WebViewBuilder::new()
        };

        struct WinHandle<'a>(&'a dyn Window);
        impl wry::raw_window_handle::HasWindowHandle for WinHandle<'_> {
            fn window_handle(
                &self,
            ) -> Result<
                wry::raw_window_handle::WindowHandle<'_>,
                wry::raw_window_handle::HandleError,
            > {
                self.0.window_handle()
            }
        }
        let win_handle = WinHandle(&*win);

        let built = builder
            .with_url(&url)
            .with_bounds(bounds)
            .with_document_title_changed_handler(move |title| {
                bus_title.send(AppEvent::WebViewTitleChanged { tab_id, title });
            })
            .with_on_page_load_handler(move |ev, loaded_url| {
                let finished = matches!(ev, PageLoadEvent::Finished);
                bus_load.send(AppEvent::WebViewPageLoad {
                    tab_id,
                    url: loaded_url,
                    finished,
                });
            })
            .with_new_window_req_handler(move |target_url, _features| {
                bus_new.send(AppEvent::OpenInNewTab(target_url));
                NewWindowResponse::Deny
            })
            .build_as_child(&win_handle);

        match built {
            Ok(wv) => {
                #[cfg(debug_assertions)]
                eprintln!("[webview] mounted OK for {url}");
                self.active_webview = Some(wv);
            }
            Err(e) => {
                #[cfg(debug_assertions)]
                eprintln!("[webview] mount FAILED for {url}: {e}");
                self.tabs[tab_idx].js_mode = false;
                self.tabs[tab_idx].force_blitz = true;
                let err_html = render_error_page(&url, &format!("WebView2 초기화 실패: {e}"));
                let doc = self.build_content_doc(&err_html, "rust://error", None, self.tabs[tab_idx].zoom);
                self.tabs[tab_idx].doc = Some(doc);
            }
        }
    }

    fn load_active_tab(&mut self, bypass_cache: bool, restore_scroll_y: Option<f64>) {
        let idx = self.active_tab;
        let url = self.tabs[idx].url.clone();
        let tab_id = self.tabs[idx].id;
        self.tabs[idx].cancel_inflight();
        let load_seq = self.tabs[idx].load_seq;
        self.status_note = None;

        // Internal built-in pages (always Pure Rust Blitz, zero WebView2)
        if url.starts_with("rust://") {
            self.destroy_active_webview();
            self.tabs[idx].js_mode = false;
            let mem = current_process_memory();
            let unloaded = self.tabs.iter().filter(|t| !t.is_loaded()).count();
            let html = if url.starts_with("rust://bookmarks") {
                render_bookmarks_page(&self.bookmarks, &url)
            } else {
                render_home_page(
                    &self.bookmarks,
                    mem.working_set_mb,
                    self.tabs.len(),
                    unloaded,
                    self.net_provider.blocked_total(),
                )
            };
            let title = short_title_for_url(&url);
            let zoom = self.tabs[idx].zoom;
            self.tabs[idx].doc = None;
            let mut doc = self.build_content_doc(&html, &url, None, zoom);
            if let Some(y) = restore_scroll_y {
                doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y });
            } else if let Some((_, frag)) = url.split_once('#') {
                if !frag.is_empty() {
                    doc.resolve(self.anim_time());
                    doc.scroll_to_fragment(frag);
                }
            }
            let tab = &mut self.tabs[idx];
            tab.title = title;
            tab.doc = Some(doc);
            tab.raw_html = None;
            tab.is_loading = false;
            self.rebuild_chrome();
            return;
        }

        if !self.tabs[idx].force_blitz && requires_js_engine(&url) {
            self.tabs[idx].js_mode = true;
        }
        if self.tabs[idx].js_mode && !self.tabs[idx].force_blitz {
            self.tabs[idx].is_loading = true;
            self.mount_or_navigate_webview(idx);
            self.rebuild_chrome();
            return;
        }

        self.destroy_active_webview();

        let pending_request = self.tabs[idx].pending_request.take();
        let cache_key = url.split('#').next().unwrap_or(&url);
        // Fast path: compact HTML cache hit (no DOM held in RAM when sleeping!)
        if !bypass_cache && pending_request.is_none() {
            if let Some(cached) = self.html_cache.get(cache_key) {
                self.apply_html_to_tab(idx, url, cached.raw_html, restore_scroll_y);
                return;
            }
        }

        // Disk cache hit: render the last copy instantly, then revalidate in the background.
        let disk_hit = if !bypass_cache && pending_request.is_none() {
            self.page_cache.get(cache_key)
        } else {
            None
        };

        let Ok(parsed_url) = Url::parse(&url) else {
            let err_html = render_error_page(&url, "잘못된 URL 형식입니다.");
            let doc = self.build_content_doc(&err_html, "rust://error", None, self.tabs[idx].zoom);
            self.tabs[idx].doc = Some(doc);
            self.tabs[idx].title = "Invalid URL".to_string();
            self.tabs[idx].is_loading = false;
            self.rebuild_chrome();
            return;
        };

        let signal = if let Some(hit) = disk_hit {
            let shown_url = match url.split_once('#') {
                Some((_, frag)) => format!("{}#{frag}", hit.final_url.split('#').next().unwrap_or("")),
                None => hit.final_url,
            };
            self.apply_html_to_tab(idx, shown_url, hit.html, restore_scroll_y);
            self.tabs[idx].revalidating = true;
            // Same controller cancels the cached page's subresources and the revalidation.
            self.tabs[idx]
                .abort_controller
                .get_or_insert_with(AbortController::default)
                .signal
                .clone()
        } else {
            let abort_ctrl = AbortController::default();
            let signal = abort_ctrl.signal.clone();
            self.tabs[idx].abort_controller = Some(abort_ctrl);
            signal
        };
        self.tabs[idx].is_loading = true;
        self.rebuild_chrome();

        let bus_ok = self.bus.clone();
        let bus_fail = self.bus.clone();
        let requested_url = url.clone();
        let fail_url = url.clone();
        let fail_url_2 = url.clone();
        let strip_iframes = self.net_provider.is_filter_enabled();
        let cacheable = pending_request.is_none();
        let request = pending_request.unwrap_or_else(|| Request::get(parsed_url));
        self.net_provider.fetch_document(
            request,
            signal,
            move |final_url, bytes| {
                match process_fetched_document(&bytes, &final_url, strip_iframes) {
                    Ok((clean_html, is_spa_shell)) => bus_ok.send(AppEvent::PageLoaded {
                        tab_id,
                        load_seq,
                        requested_url,
                        cacheable,
                        final_url,
                        clean_html: Arc::from(clean_html),
                        is_spa_shell,
                        restore_scroll_y,
                    }),
                    Err(reason) => bus_ok.send(AppEvent::PageLoadFailed {
                        tab_id,
                        load_seq,
                        url: fail_url_2,
                        error: reason,
                    }),
                }
            },
            move |err| {
                bus_fail.send(AppEvent::PageLoadFailed {
                    tab_id,
                    load_seq,
                    url: fail_url,
                    error: err,
                });
            },
        );
    }

    fn navigate_current(&mut self, raw_input: &str) {
        self.navigate_with_request(raw_input, None);
    }

    fn navigate_with_request(&mut self, raw_input: &str, request: Option<Request>) {
        self.close_popup();
        let target = normalize_url_input(raw_input);
        let idx = self.active_tab;
        let current_url = self.tabs[idx].url.clone();

        // Same-document #fragment jump without refetching
        let cur_base = current_url.split('#').next().unwrap_or(&current_url);
        let new_base = target.split('#').next().unwrap_or(&target);
        if request.is_none() && !self.tabs[idx].js_mode && cur_base == new_base && target.contains('#') {
            if let Some((_, frag)) = target.split_once('#') {
                if let Some(doc) = &mut self.tabs[idx].doc {
                    doc.scroll_to_fragment(frag);
                    self.tabs[idx].url = target;
                    self.chrome_focused = false;
                    self.rebuild_chrome();
                    return;
                }
            }
        }

        let current_scroll = self.tabs[idx]
            .doc
            .as_ref()
            .map(|d| d.viewport_scroll().y)
            .unwrap_or(0.0);

        if current_url != target {
            self.tabs[idx].history_back.push(HistoryEntry {
                url: current_url,
                scroll_y: current_scroll,
            });
            self.tabs[idx].history_forward.clear();
        }
        self.tabs[idx].url = target;
        self.tabs[idx].scroll_y = 0.0;
        self.tabs[idx].force_blitz = false;
        self.tabs[idx].pending_request = request;
        self.chrome_focused = false;
        self.load_active_tab(false, None);
    }

    fn switch_tab(&mut self, target_idx: usize) {
        if target_idx >= self.tabs.len() || target_idx == self.active_tab {
            return;
        }
        self.close_popup();
        // Immediately drop active WebView2 and Blitz DOM of the leaving tab!
        self.destroy_active_webview();
        if self.tabs[self.active_tab].unload() {
            trim_process_working_set();
        }
        self.active_tab = target_idx;
        let restore_y = Some(self.tabs[target_idx].scroll_y);
        self.load_active_tab(false, restore_y);
    }

    fn open_new_tab(&mut self, url: &str) {
        self.close_popup();
        self.destroy_active_webview();
        if self.tabs[self.active_tab].unload() {
            trim_process_working_set();
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(id, url.to_string()));
        self.active_tab = self.tabs.len() - 1;
        self.load_active_tab(false, None);
    }

    /// Ctrl+click: add a sleeping tab right after the active one without loading it.
    fn open_background_tab(&mut self, url: &str) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, url.to_string());
        tab.title = short_title_for_url(url);
        self.tabs.insert(self.active_tab + 1, tab);
        self.status_note = Some("백그라운드 탭 열림".to_string());
        self.rebuild_chrome();
    }

    fn reopen_closed_tab(&mut self) {
        if let Some(url) = self.closed_tabs.pop() {
            self.open_new_tab(&url);
        }
    }

    fn set_zoom(&mut self, zoom: f32) {
        let zoom = (zoom * 10.0).round() / 10.0;
        let zoom = zoom.clamp(0.3, 3.0);
        let idx = self.active_tab;
        self.tabs[idx].zoom = zoom;
        if let Some(doc) = &mut self.tabs[idx].doc {
            doc.viewport_mut().set_zoom(zoom);
        }
        if let Some(wv) = &self.active_webview {
            let _ = wv.zoom(zoom as f64);
        }
        self.rebuild_chrome();
        self.request_redraw();
    }

    fn toggle_maximize(&mut self) {
        if let Some(win) = &self.window {
            win.set_maximized(!win.is_maximized());
        }
    }

    fn exit_app(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.close_popup();
        if let Some(wv) = self.active_webview.take() {
            persist_session_cookies(&wv);
        }
        self.net_provider.cookies.save();
        self.renderer.suspend();
        self.window = None;
        event_loop.exit();
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

    fn close_tab(&mut self, idx: usize, event_loop: Option<&dyn ActiveEventLoop>) {
        self.close_popup();
        let closed_url = self.tabs[idx].url.clone();
        if !closed_url.starts_with("rust://home") {
            if self.closed_tabs.len() >= 16 {
                self.closed_tabs.remove(0);
            }
            self.closed_tabs.push(closed_url);
        }
        if idx == self.active_tab {
            self.destroy_active_webview();
        }
        if self.tabs.len() <= 1 {
            if let Some(el) = event_loop {
                el.exit();
            } else {
                self.tabs[0].unload();
                trim_process_working_set();
                self.tabs[0] = Tab::new(self.next_tab_id, "rust://home".to_string());
                self.next_tab_id += 1;
                self.load_active_tab(false, None);
            }
            return;
        }

        let had_doc = self.tabs[idx].unload();
        self.tabs.remove(idx);
        if had_doc {
            trim_process_working_set();
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if idx < self.active_tab {
            self.active_tab -= 1;
        }
        if self.tabs[self.active_tab].js_mode || !self.tabs[self.active_tab].is_loaded() {
            let restore_y = Some(self.tabs[self.active_tab].scroll_y);
            self.load_active_tab(false, restore_y);
        } else {
            self.rebuild_chrome();
        }
    }

    fn go_back(&mut self) {
        let idx = self.active_tab;
        if self.tabs[idx].js_mode {
            if let Some(wv) = &self.active_webview {
                let _ = wv.evaluate_script("window.history.back();");
                return;
            }
        }
        let Some(prev) = self.tabs[idx].history_back.pop() else {
            return;
        };
        let cur_scroll = self.tabs[idx]
            .doc
            .as_ref()
            .map(|d| d.viewport_scroll().y)
            .unwrap_or(0.0);
        let cur = HistoryEntry {
            url: self.tabs[idx].url.clone(),
            scroll_y: cur_scroll,
        };
        self.tabs[idx].history_forward.push(cur);
        self.tabs[idx].url = prev.url;
        self.tabs[idx].force_blitz = false;
        self.load_active_tab(false, Some(prev.scroll_y));
    }

    fn go_forward(&mut self) {
        let idx = self.active_tab;
        if self.tabs[idx].js_mode {
            if let Some(wv) = &self.active_webview {
                let _ = wv.evaluate_script("window.history.forward();");
                return;
            }
        }
        let Some(next) = self.tabs[idx].history_forward.pop() else {
            return;
        };
        let cur_scroll = self.tabs[idx]
            .doc
            .as_ref()
            .map(|d| d.viewport_scroll().y)
            .unwrap_or(0.0);
        let cur = HistoryEntry {
            url: self.tabs[idx].url.clone(),
            scroll_y: cur_scroll,
        };
        self.tabs[idx].history_back.push(cur);
        self.tabs[idx].url = next.url;
        self.tabs[idx].force_blitz = false;
        self.load_active_tab(false, Some(next.scroll_y));
    }

    fn reload_active_tab(&mut self) {
        if self.tabs[self.active_tab].js_mode {
            if let Some(wv) = &self.active_webview {
                let _ = wv.evaluate_script("window.location.reload();");
            } else {
                self.mount_or_navigate_webview(self.active_tab);
            }
        } else if self.tabs[self.active_tab].is_loading {
            self.tabs[self.active_tab].cancel_inflight();
            self.rebuild_chrome();
        } else {
            self.load_active_tab(true, None);
        }
    }

    fn toggle_js_mode(&mut self) {
        let idx = self.active_tab;
        if self.tabs[idx].url.starts_with("rust://") {
            return;
        }
        let next = !self.tabs[idx].js_mode;
        self.tabs[idx].js_mode = next;
        self.tabs[idx].force_blitz = !next;
        if next {
            self.tabs[idx].reader_mode = false;
            self.status_note = Some("JS Mode (WebView2)".to_string());
            self.mount_or_navigate_webview(idx);
            self.rebuild_chrome();
        } else {
            self.destroy_active_webview();
            self.status_note = Some("Pure Rust Mode (Blitz)".to_string());
            self.load_active_tab(false, None);
        }
    }

    fn toggle_reader_mode(&mut self) {
        let idx = self.active_tab;
        self.tabs[idx].reader_mode = !self.tabs[idx].reader_mode;
        if let Some(raw) = self.tabs[idx].raw_html.clone() {
            let url = self.tabs[idx].url.clone();
            let scroll = self.tabs[idx]
                .doc
                .as_ref()
                .map(|d| d.viewport_scroll().y)
                .unwrap_or(0.0);
            self.apply_html_to_tab(idx, url, raw, Some(scroll));
        } else {
            self.rebuild_chrome();
        }
    }

    fn focus_url_bar(&mut self) {
        self.chrome_focused = true;
        if let Some(win) = &self.window {
            win.focus_window();
        }
        if let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") {
            self.chrome_doc.set_focus_to(url_id);
            let select_all = synthetic_key(Key::Character("a".to_string()), Code::KeyA, ACTION_MOD);
            self.dispatch_chrome_ui(UiEvent::KeyDown(select_all));
        }
        self.request_redraw();
    }

    /// Trimmed text of a focused `<input id=...>` on a built-in `rust://` page.
    fn focused_builtin_input(&self, id: &str) -> Option<String> {
        let tab = &self.tabs[self.active_tab];
        if !tab.url.starts_with("rust://") {
            return None;
        }
        let doc = tab.doc.as_ref()?;
        let sid = doc.get_element_by_id(id)?;
        if doc.get_focussed_node_id() != Some(sid) {
            return None;
        }
        let input = doc.get_node(sid)?.element_data()?.text_input_data()?;
        Some(input.editor.text().to_string().trim().to_string())
    }

    fn read_url_bar_text(&self) -> Option<String> {
        let url_id = self.chrome_doc.get_element_by_id("url-bar")?;
        let node = self.chrome_doc.get_node(url_id)?;
        let el = node.element_data()?;
        let input = el.text_input_data()?;
        Some(input.editor.text().to_string())
    }

    fn close_popup(&mut self) {
        if let Some(mut popup) = self.popup.take() {
            self.recently_closed_popup = Some((popup.key.clone(), Instant::now()));
            popup.renderer.suspend();
        }
    }

    fn build_popup_view_data(
        &self,
        root: BookmarkRootKind,
        path: &[usize],
        base_depth: usize,
        index_offset: usize,
        root_title: &str,
    ) -> Option<(String, Cow<'_, [BookmarkNode]>, bool, usize)> {
        let (folder_title, nodes) = self.bookmarks.resolve_folder(root, path)?;
        let at_base = path.len() <= base_depth;
        if at_base && index_offset > 0 {
            let sliced = match nodes {
                Cow::Borrowed(n) => Cow::Borrowed(&n[index_offset.min(n.len())..]),
                Cow::Owned(v) => Cow::Owned(v.into_iter().skip(index_offset).collect()),
            };
            Some((root_title.to_string(), sliced, false, index_offset))
        } else {
            let title = if path.is_empty() {
                root_title.to_string()
            } else {
                folder_title
            };
            Some((title, nodes, !at_base, 0))
        }
    }

    fn open_bookmark_popup(
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

        self.close_popup();

        let base_depth = path.len();
        let Some((title, nodes, can_go_back, eff_offset)) =
            self.build_popup_view_data(root, &path, base_depth, index_offset, &root_title)
        else {
            return;
        };

        let logical_h = estimate_popup_height(nodes.len(), true);
        let html = render_bookmark_popup_html(&title, &nodes, can_go_back, eff_offset);
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
            self.request_favicon(url, None);
        }
        self.pump_favicons();

        let Some(main_win) = &self.window else {
            return;
        };
        let scale = self.hidpi_scale.max(1.0) as f64;
        let surf_pos = main_win.surface_position();
        let win_w_logical = (self.win_size.width as f64 / scale).max(POPUP_WIDTH as f64);
        let anchor_x_logical = (self.last_chrome_click_x_logical as f64 - 24.0)
            .clamp(8.0, (win_w_logical - POPUP_WIDTH as f64 - 8.0).max(8.0));
        let popup_x_phys = surf_pos.x + (anchor_x_logical * scale).round() as i32;
        let popup_y_phys = surf_pos.y + ((CHROME_HEIGHT as f64 - 1.0) * scale).round() as i32;

        let attrs = WindowAttributes::default()
            .with_title("Bookmarks")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_surface_size(LogicalSize::new(POPUP_WIDTH as f64, logical_h as f64))
            .with_position(PhysicalPosition::new(popup_x_phys, popup_y_phys));
        let attrs = popup_window_style(attrs);

        let Ok(popup_box) = event_loop.create_window(attrs) else {
            return;
        };
        let popup_win: Arc<dyn Window> = Arc::from(popup_box);
        let popup_scale = popup_win.scale_factor() as f32;
        let popup_size = popup_win.surface_size();
        let w = popup_size.width.max(1);
        let h = popup_size.height.max(1);

        let mut vp = Viewport::new(w, h, popup_scale, self.color_scheme);
        vp.zoom = 1.0;
        let shell: Arc<dyn ShellProvider> = Arc::new(BrowserShellProvider {
            window: Arc::clone(&popup_win),
        });
        let doc = HtmlDocument::from_html(
            &html,
            DocumentConfig {
                viewport: Some(vp),
                font_ctx: Some(self.shared_font_ctx.clone()),
                net_provider: Some(self.net_provider.clone()),
                shell_provider: Some(shell),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );

        let mut renderer = VelloCpuWindowRenderer::new();
        renderer.resume(Arc::new(Arc::clone(&popup_win)), w, h, || {});
        renderer.complete_resume();
        renderer.set_size(w, h);

        popup_win.request_redraw();
        popup_win.focus_window();

        self.popup = Some(BookmarkPopup {
            key,
            root,
            path,
            base_depth,
            root_title,
            index_offset,
            window: popup_win,
            renderer,
            doc,
            win_size: popup_size,
            hidpi_scale: popup_scale,
            pointer_pos: PhysicalPosition::new(0.0, 0.0),
            buttons: MouseEventButtons::None,
        });
    }

    fn refresh_popup(&mut self) {
        let Some(popup) = &self.popup else {
            return;
        };
        let Some((title, nodes, can_go_back, eff_offset)) = self.build_popup_view_data(
            popup.root,
            &popup.path,
            popup.base_depth,
            popup.index_offset,
            &popup.root_title,
        ) else {
            return;
        };
        let logical_h = estimate_popup_height(nodes.len(), true);
        let html = render_bookmark_popup_html(&title, &nodes, can_go_back, eff_offset);

        let Some(popup) = &mut self.popup else {
            return;
        };
        let _ = popup
            .window
            .request_surface_size(LogicalSize::new(POPUP_WIDTH as f64, logical_h as f64).into());
        let phys_w = ((POPUP_WIDTH as f32) * popup.hidpi_scale).round().max(1.0) as u32;
        let phys_h = ((logical_h as f32) * popup.hidpi_scale).round().max(1.0) as u32;
        popup.win_size = PhysicalSize::new(phys_w, phys_h);

        let vp = Viewport::new(phys_w, phys_h, popup.hidpi_scale, self.color_scheme);
        let shell: Arc<dyn ShellProvider> = Arc::new(BrowserShellProvider {
            window: Arc::clone(&popup.window),
        });
        popup.doc = HtmlDocument::from_html(
            &html,
            DocumentConfig {
                viewport: Some(vp),
                font_ctx: Some(self.shared_font_ctx.clone()),
                net_provider: Some(self.net_provider.clone()),
                shell_provider: Some(shell),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );
        if popup.renderer.is_active() {
            popup.renderer.set_size(phys_w, phys_h);
        }
        popup.window.request_redraw();
    }

    fn handle_popup_action(&mut self, action: &str, new_tab: bool) {
        if action == "popup-back" {
            let changed = if let Some(popup) = &mut self.popup {
                if popup.path.len() > popup.base_depth {
                    popup.path.pop();
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if changed {
                self.refresh_popup();
            }
        } else if let Some(rest) = action.strip_prefix("popup-enter:") {
            if let Ok(idx) = rest.parse::<usize>() {
                if let Some(popup) = &mut self.popup {
                    popup.path.push(idx);
                }
                self.refresh_popup();
            }
        } else if let Some(url) = action.strip_prefix("popup-open:") {
            let url = url.to_string();
            self.close_popup();
            if new_tab {
                self.open_new_tab(&url);
            } else {
                self.navigate_current(&url);
            }
        } else if action == "popup-manager" {
            self.close_popup();
            if new_tab {
                self.open_new_tab("rust://bookmarks");
            } else {
                self.navigate_current("rust://bookmarks");
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
        let mut driver = EventDriver::new(&mut popup.doc, handler);
        driver.handle_ui_event(event);
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

    fn handle_chrome_action(&mut self, action: &str, event_loop: &dyn ActiveEventLoop) {
        if action == "new-tab" {
            self.open_new_tab("rust://home");
            self.focus_url_bar();
        } else if let Some(rest) = action.strip_prefix("tab:") {
            if let Ok(idx) = rest.parse::<usize>() {
                self.switch_tab(idx);
            }
        } else if let Some(rest) = action.strip_prefix("close-tab:") {
            if let Ok(idx) = rest.parse::<usize>() {
                self.close_tab(idx, Some(event_loop));
            }
        } else if action == "back" {
            self.go_back();
        } else if action == "forward" {
            self.go_forward();
        } else if action == "reload" {
            self.reload_active_tab();
        } else if action == "submit-url" {
            if let Some(text) = self.read_url_bar_text() {
                self.navigate_current(&text);
            }
        } else if action == "toggle-bookmark" {
            let tab = &self.tabs[self.active_tab];
            self.bookmarks.toggle_custom(&tab.title, &tab.url);
            self.rebuild_chrome();
        } else if action == "toggle-js" {
            self.toggle_js_mode();
        } else if action == "toggle-reader" {
            if self.tabs[self.active_tab].js_mode {
                self.tabs[self.active_tab].js_mode = false;
                self.tabs[self.active_tab].force_blitz = true;
                self.destroy_active_webview();
                self.tabs[self.active_tab].reader_mode = true;
                self.load_active_tab(false, None);
            } else {
                self.toggle_reader_mode();
            }
        } else if action == "toggle-filter" {
            self.net_provider.toggle_filter();
            self.rebuild_chrome();
        } else if action == "sync-chrome" {
            match self.bookmarks.import_from_chrome() {
                Ok(n) => self.status_note = Some(format!("Synced {n} Chrome bookmarks")),
                Err(e) => self.status_note = Some(e),
            }
            if self.tabs[self.active_tab].url.starts_with("rust://bookmarks")
                || self.tabs[self.active_tab].url == "rust://home"
            {
                self.load_active_tab(false, None);
            } else {
                self.rebuild_chrome();
            }
        } else if let Some(rest) = action.strip_prefix("bm-popup:bar:") {
            if let Ok(idx) = rest.parse::<usize>() {
                let title = self
                    .bookmarks
                    .resolve_folder(BookmarkRootKind::Bar, &[idx])
                    .map(|(t, _)| t)
                    .unwrap_or_else(|| "Folder".to_string());
                self.bus.send(AppEvent::OpenBookmarkPopup {
                    key: action.to_string(),
                    root: BookmarkRootKind::Bar,
                    path: vec![idx],
                    index_offset: 0,
                    title,
                });
            }
        } else if let Some(rest) = action.strip_prefix("bm-popup:overflow:") {
            if let Ok(offset) = rest.parse::<usize>() {
                self.bus.send(AppEvent::OpenBookmarkPopup {
                    key: action.to_string(),
                    root: BookmarkRootKind::Bar,
                    path: Vec::new(),
                    index_offset: offset,
                    title: "북마크 더보기".to_string(),
                });
            }
        } else if action == "bm-popup:other" {
            self.bus.send(AppEvent::OpenBookmarkPopup {
                key: action.to_string(),
                root: BookmarkRootKind::Other,
                path: Vec::new(),
                index_offset: 0,
                title: "기타 북마크".to_string(),
            });
        } else if let Some(url) = action.strip_prefix("nav:") {
            self.navigate_current(url);
        } else if action == "win-min" {
            if let Some(win) = &self.window {
                win.set_minimized(true);
            }
        } else if action == "win-max" {
            self.toggle_maximize();
        } else if action == "win-close" {
            self.exit_app(event_loop);
        } else if action == "zoom-reset" {
            self.set_zoom(1.0);
        }
    }

    fn process_app_events(&mut self, event_loop: &dyn ActiveEventLoop) {
        let mut favicons_changed = false;
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::FaviconDone { ok } => {
                    self.favicon_inflight = false;
                    favicons_changed |= ok;
                }
                AppEvent::RedrawDoc { doc_id } => {
                    if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                        if doc.id() == doc_id {
                            doc.poll(None);
                        }
                    }
                    self.request_redraw();
                }
                AppEvent::PageLoaded {
                    tab_id,
                    load_seq,
                    requested_url,
                    cacheable,
                    final_url,
                    clean_html,
                    is_spa_shell,
                    restore_scroll_y,
                } => {
                    if cacheable && !is_spa_shell {
                        let req_key = requested_url.split('#').next().unwrap_or(&requested_url);
                        let final_key = final_url.split('#').next().unwrap_or(&final_url);
                        self.page_cache.put(req_key, &final_url, &clean_html);
                        if final_key != req_key {
                            self.page_cache.put(final_key, &final_url, &clean_html);
                        }
                    }
                    #[cfg(debug_assertions)]
                    eprintln!("[event] PageLoaded tab={tab_id} seq={load_seq} spa={is_spa_shell} url={final_url}");
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        if idx == self.active_tab && self.tabs[idx].load_seq == load_seq {
                            if (is_spa_shell || requires_js_engine(&final_url))
                                && !self.tabs[idx].force_blitz
                                && !self.tabs[idx].reader_mode
                            {
                                self.tabs[idx].url = final_url;
                                self.tabs[idx].is_loading = false;
                                self.status_note = Some("Auto JS Mode (SPA)".to_string());
                                self.mount_or_navigate_webview(idx);
                                self.rebuild_chrome();
                            } else if self.tabs[idx].revalidating {
                                // Background revalidation of a disk-cached page: only swap in
                                // the fresh copy if it changed, keeping the reading position.
                                self.tabs[idx].revalidating = false;
                                self.tabs[idx].is_loading = false;
                                let unchanged = self.tabs[idx].raw_html.as_deref() == Some(&*clean_html);
                                if unchanged {
                                    self.rebuild_chrome();
                                } else {
                                    let scroll = self.tabs[idx].doc.as_ref().map(|d| d.viewport_scroll().y);
                                    let fragment = self.tabs[idx].url.split_once('#').map(|(_, f)| f.to_string());
                                    let shown = match fragment {
                                        Some(f) if !final_url.contains('#') => format!("{final_url}#{f}"),
                                        _ => final_url,
                                    };
                                    self.apply_html_to_tab(idx, shown, clean_html, scroll);
                                }
                            } else {
                                self.apply_html_to_tab(idx, final_url, clean_html, restore_scroll_y);
                            }
                            // Persist any login cookies set during this navigation.
                            self.net_provider.cookies.save();
                        }
                    }
                }
                AppEvent::PageLoadFailed {
                    tab_id,
                    load_seq,
                    url,
                    error,
                } => {
                    #[cfg(debug_assertions)]
                    eprintln!("[event] PageLoadFailed tab={tab_id} url={url} err={error}");
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        if idx == self.active_tab && self.tabs[idx].load_seq == load_seq && self.tabs[idx].revalidating {
                            // Offline / server error: keep showing the cached copy.
                            self.tabs[idx].revalidating = false;
                            self.tabs[idx].is_loading = false;
                            self.status_note = Some("저장된 페이지 표시 중 (오프라인)".to_string());
                            self.rebuild_chrome();
                        } else if idx == self.active_tab && self.tabs[idx].load_seq == load_seq {
                            let err_html = render_error_page(&url, &error);
                            self.tabs[idx].doc = None;
                            let doc = self.build_content_doc(&err_html, &url, None, self.tabs[idx].zoom);
                            self.tabs[idx].doc = Some(doc);
                            self.tabs[idx].title = "Load Error".to_string();
                            self.tabs[idx].is_loading = false;
                            self.rebuild_chrome();
                        }
                    }
                }
                AppEvent::Navigate {
                    source_doc_id,
                    url,
                    request,
                } => {
                    let matches_active = self.tabs[self.active_tab]
                        .doc
                        .as_ref()
                        .is_some_and(|d| d.id() == source_doc_id);
                    if matches_active {
                        // Ctrl+click opens a sleeping background tab (zero DOM memory until visited).
                        if request.is_none() && primary_modifier(self.keyboard_modifiers.state()) {
                            self.open_background_tab(url.as_str());
                        } else {
                            self.navigate_with_request(url.as_str(), request);
                        }
                    }
                }
                AppEvent::WebViewTitleChanged { tab_id, title } => {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        if !title.trim().is_empty() && self.tabs[idx].title != title {
                            self.tabs[idx].title = title;
                            if idx == self.active_tab {
                                self.rebuild_chrome();
                            }
                        }
                    }
                }
                AppEvent::WebViewPageLoad {
                    tab_id,
                    url,
                    finished,
                } => {
                    if finished {
                        self.request_favicon(&url, None);
                    }
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        self.tabs[idx].is_loading = !finished;
                        if !url.is_empty() && url != "about:blank" {
                            self.tabs[idx].url = url;
                        }
                        if idx == self.active_tab {
                            self.rebuild_chrome();
                        }
                    }
                }
                AppEvent::OpenInNewTab(url) => {
                    self.open_new_tab(&url);
                }
                AppEvent::ChromeAction(action) => {
                    self.handle_chrome_action(&action, event_loop);
                }
                AppEvent::SubmitUrlBar => {
                    if let Some(text) = self.read_url_bar_text() {
                        self.navigate_current(&text);
                    }
                }
                AppEvent::OpenBookmarkPopup {
                    key,
                    root,
                    path,
                    index_offset,
                    title,
                } => {
                    self.open_bookmark_popup(event_loop, key, root, path, index_offset, title);
                }
                AppEvent::PopupAction { action, new_tab } => {
                    self.handle_popup_action(&action, new_tab);
                }
            }
        }
        // Coalesce a burst of icon downloads into one chrome/popup refresh.
        if favicons_changed {
            self.rebuild_chrome();
            self.refresh_popup();
        }
        self.pump_favicons();
    }

    fn chrome_coords(&self, pos: PhysicalPosition<f64>) -> PointerCoords {
        let scale = self.hidpi_scale as f64;
        let logical: LogicalPosition<f32> = pos.to_logical(scale);
        PointerCoords {
            screen_x: logical.x,
            screen_y: logical.y,
            client_x: logical.x,
            client_y: logical.y,
            page_x: logical.x,
            page_y: logical.y,
        }
    }

    fn content_coords(&self, pos: PhysicalPosition<f64>) -> PointerCoords {
        let scale = self.hidpi_scale as f64;
        let logical: LogicalPosition<f32> = pos.to_logical(scale);
        let zoom = self.tabs[self.active_tab]
            .doc
            .as_ref()
            .map(|d| d.viewport().zoom)
            .unwrap_or(1.0);
        let scroll = self.tabs[self.active_tab]
            .doc
            .as_ref()
            .map(|d| d.viewport_scroll())
            .unwrap_or(blitz_dom::Point { x: 0.0, y: 0.0 });

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

    fn is_in_chrome(&self, pos: PhysicalPosition<f64>) -> bool {
        let logical_y = pos.y / (self.hidpi_scale as f64);
        logical_y < (CHROME_HEIGHT as f64)
    }

    fn dispatch_chrome_ui(&mut self, event: UiEvent) {
        let handler = ChromeEventHandler {
            bus: self.bus.clone(),
        };
        let mut driver = EventDriver::new(&mut self.chrome_doc, handler);
        driver.handle_ui_event(event);
    }

    fn handle_popup_window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Focused(false) => {
                self.close_popup();
            }
            WindowEvent::RedrawRequested => {
                self.redraw_popup();
            }
            WindowEvent::SurfaceResized(size) => {
                if let Some(popup) = &mut self.popup {
                    popup.win_size = size;
                    let w = size.width.max(1);
                    let h = size.height.max(1);
                    let vp = Viewport::new(w, h, popup.hidpi_scale, self.color_scheme);
                    popup.doc.set_viewport(vp);
                    if popup.renderer.is_active() {
                        popup.renderer.set_size(w, h);
                    }
                    popup.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() {
                    if let PhysicalKey::Code(KeyCode::Escape) = event.physical_key {
                        self.close_popup();
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
                let scale = popup.hidpi_scale as f64;
                let logical: LogicalPosition<f32> = position.to_logical(scale);
                let scroll = popup.doc.viewport_scroll();
                let coords = PointerCoords {
                    screen_x: logical.x,
                    screen_y: logical.y,
                    client_x: logical.x,
                    client_y: logical.y,
                    page_x: logical.x + scroll.x as f32,
                    page_y: logical.y + scroll.y as f32,
                };
                let ev = BlitzPointerEvent {
                    id: pointer_source_to_blitz(&source),
                    is_primary: primary,
                    coords,
                    button: Default::default(),
                    buttons: popup.buttons,
                    mods: winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state()),
                    details: pointer_source_to_blitz_details(&source),
                    element: Default::default(),
                    active_pointers: Default::default(),
                };
                // Blitz requests a redraw itself when hover state actually changes.
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
                let id = button_source_to_blitz(&button);
                let btn = match &button {
                    ButtonSource::Mouse(mb) => match mb {
                        MouseButton::Left => MouseEventButton::Main,
                        MouseButton::Right => MouseEventButton::Secondary,
                        _ => MouseEventButton::Auxiliary,
                    },
                    _ => MouseEventButton::Main,
                };
                match state {
                    ElementState::Pressed => popup.buttons |= btn.into(),
                    ElementState::Released => popup.buttons ^= btn.into(),
                }
                let scale = popup.hidpi_scale as f64;
                let logical: LogicalPosition<f32> = position.to_logical(scale);
                let scroll = popup.doc.viewport_scroll();
                let coords = PointerCoords {
                    screen_x: logical.x,
                    screen_y: logical.y,
                    client_x: logical.x,
                    client_y: logical.y,
                    page_x: logical.x + scroll.x as f32,
                    page_y: logical.y + scroll.y as f32,
                };
                let pev = BlitzPointerEvent {
                    id,
                    is_primary: primary,
                    coords,
                    button: btn,
                    buttons: popup.buttons,
                    mods: winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state()),
                    details: PointerDetails::default(),
                    element: Default::default(),
                    active_pointers: Default::default(),
                };
                let ui_ev = match state {
                    ElementState::Pressed => UiEvent::PointerDown(pev),
                    ElementState::Released => UiEvent::PointerUp(pev),
                };
                self.dispatch_popup_ui(ui_ev);
                self.process_app_events(event_loop);
                if let Some(p) = &self.popup {
                    p.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(popup) = &mut self.popup else {
                    return;
                };
                let blitz_delta = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        BlitzWheelDelta::Lines(x as f64, y as f64)
                    }
                    winit::event::MouseScrollDelta::PixelDelta(p) => {
                        BlitzWheelDelta::Pixels(p.x, p.y)
                    }
                    _ => BlitzWheelDelta::Pixels(0.0, 0.0),
                };
                let scale = popup.hidpi_scale as f64;
                let logical: LogicalPosition<f32> = popup.pointer_pos.to_logical(scale);
                let scroll = popup.doc.viewport_scroll();
                let coords = PointerCoords {
                    screen_x: logical.x,
                    screen_y: logical.y,
                    client_x: logical.x,
                    client_y: logical.y,
                    page_x: logical.x + scroll.x as f32,
                    page_y: logical.y + scroll.y as f32,
                };
                let mods = winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state());
                let buttons = popup.buttons;
                popup.doc.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
                    delta: blitz_delta,
                    coords,
                    buttons,
                    mods,
                    element: Default::default(),
                }));
            }
            _ => {}
        }
    }

    fn redraw(&mut self) {
        if !self.renderer.is_active() {
            return;
        }
        let anim_time = self.anim_time();
        let scale = self.hidpi_scale as f64;
        let win_w = self.win_size.width.max(1);
        let win_h = self.win_size.height.max(1);
        let chrome_h_phys = self.chrome_phys_height().min(win_h);
        let content_h_phys = win_h.saturating_sub(chrome_h_phys);

        self.chrome_doc.poll(None);
        self.chrome_doc.resolve(anim_time);

        let mut needs_more_frames = false;
        let has_webview = self.active_webview.is_some();
        let content_scale = if has_webview {
            scale
        } else {
            self.tabs[self.active_tab]
                .doc
                .as_mut()
                .map(|doc| {
                    if doc.poll(None) {
                        needs_more_frames = true;
                    }
                    doc.resolve(anim_time);
                    if doc.is_animating() {
                        needs_more_frames = true;
                    }
                    doc.viewport().scale_f64()
                })
                .unwrap_or(scale)
        };

        let chrome_inner = &mut *self.chrome_doc;
        let content_doc_opt = if has_webview {
            None
        } else {
            self.tabs[self.active_tab].doc.as_mut()
        };

        self.renderer.render(|scene| {
            if let Some(content_doc) = content_doc_opt {
                if content_h_phys > 0 {
                    let clip = Rect::new(
                        0.0,
                        chrome_h_phys as f64,
                        win_w as f64,
                        win_h as f64,
                    );
                    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
                    paint_scene(
                        scene,
                        content_doc,
                        content_scale,
                        win_w,
                        content_h_phys,
                        0,
                        chrome_h_phys,
                    );
                    scene.pop_layer();
                }
            }

            paint_scene(
                scene,
                chrome_inner,
                scale,
                win_w,
                chrome_h_phys,
                0,
                0,
            );
        });

        if needs_more_frames {
            self.request_redraw();
        }
    }
}

impl ApplicationHandler for BrowserApp {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        #[cfg(debug_assertions)]
        eprintln!("[winit] can_create_surfaces window_is_none={}", self.window.is_none());
        if self.window.is_none() {
            let attrs = WindowAttributes::default()
                .with_title("Rust Clean Browser")
                .with_min_surface_size(LogicalSize::new(520.0, 360.0))
                .with_surface_size(LogicalSize::new(1280.0, 820.0))
                .with_window_icon(app_icon());
            let attrs = main_window_style(attrs);
            let win: Arc<dyn Window> = Arc::from(event_loop.create_window(attrs).unwrap());
            self.maximized = win.is_maximized();
            self.hidpi_scale = win.scale_factor() as f32;
            self.win_size = win.surface_size();
            self.color_scheme = theme_to_color_scheme(win.theme().unwrap_or(Theme::Light));
            let shell: Arc<dyn ShellProvider> = Arc::new(BrowserShellProvider {
                window: Arc::clone(&win),
            });
            let chrome_vp = self.chrome_viewport();
            let content_vp = self.content_viewport(1.0);
            self.shell_provider = Some(Arc::clone(&shell));
            self.chrome_doc.set_viewport(chrome_vp);
            self.chrome_doc.set_shell_provider(Arc::clone(&shell));
            if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                doc.set_viewport(content_vp);
                doc.set_shell_provider(shell);
            }
            self.window = Some(win);
            self.update_window_title();
        }

        if self.tabs[self.active_tab].js_mode && self.active_webview.is_none() {
            self.mount_or_navigate_webview(self.active_tab);
        }

        if let Some(win) = &self.window {
            let w = self.win_size.width.max(1);
            let h = self.win_size.height.max(1);
            self.renderer.resume(Arc::new(Arc::clone(win)), w, h, || {});
            self.renderer.complete_resume();
            self.renderer.set_size(w, h);
            self.request_redraw();
        }
    }

    fn destroy_surfaces(&mut self, _event_loop: &dyn ActiveEventLoop) {
        self.close_popup();
        self.renderer.suspend();
    }

    fn resumed(&mut self, _event_loop: &dyn ActiveEventLoop) {}
    fn suspended(&mut self, _event_loop: &dyn ActiveEventLoop) {}

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.process_app_events(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.popup.as_ref().is_some_and(|p| p.window.id() == window_id) {
            self.handle_popup_window_event(event_loop, event);
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.exit_app(event_loop);
            }
            WindowEvent::ThemeChanged(theme) => {
                self.color_scheme = theme_to_color_scheme(theme);
                let chrome_vp = self.chrome_viewport();
                self.chrome_doc.set_viewport(chrome_vp);
                let content_vp = self.content_viewport(self.tabs[self.active_tab].zoom);
                if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.set_viewport(content_vp);
                }
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            WindowEvent::SurfaceResized(size) => {
                self.win_size = size;
                let w = size.width.max(1);
                let h = size.height.max(1);
                let maximized = self.window.as_ref().is_some_and(|win| win.is_maximized());
                if maximized != self.maximized {
                    self.maximized = maximized;
                    self.rebuild_chrome();
                }
                let chrome_vp = self.chrome_viewport();
                self.chrome_doc.set_viewport(chrome_vp);
                let content_vp = self.content_viewport(self.tabs[self.active_tab].zoom);
                if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.set_viewport(content_vp);
                }
                if self.renderer.is_active() {
                    self.renderer.set_size(w, h);
                }
                let bounds = self.webview_bounds();
                if let Some(wv) = &self.active_webview {
                    let _ = wv.set_bounds(bounds);
                }
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.hidpi_scale = scale_factor as f32;
                let chrome_vp = self.chrome_viewport();
                self.chrome_doc.set_viewport(chrome_vp);
                let content_vp = self.content_viewport(self.tabs[self.active_tab].zoom);
                if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.set_viewport(content_vp);
                }
                let bounds = self.webview_bounds();
                if let Some(wv) = &self.active_webview {
                    let _ = wv.set_bounds(bounds);
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.keyboard_modifiers = mods;
            }
            WindowEvent::Ime(ime_ev) => {
                let ev = UiEvent::Ime(winit_ime_to_blitz(ime_ev));
                if self.chrome_focused {
                    self.dispatch_chrome_ui(ev);
                } else if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.handle_ui_event(ev);
                }
                self.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        // Cmd on macOS (Ctrl+Tab keeps working there too), Ctrl elsewhere.
                        let ctrl = primary_modifier(self.keyboard_modifiers.state())
                            || (code == KeyCode::Tab && self.keyboard_modifiers.state().control_key());
                        let shift = self.keyboard_modifiers.state().shift_key();
                        let alt = self.keyboard_modifiers.state().alt_key();
                        if ctrl {
                            match code {
                                KeyCode::KeyT if shift => {
                                    self.reopen_closed_tab();
                                    return;
                                }
                                KeyCode::Tab if shift => {
                                    if self.tabs.len() > 1 {
                                        let prev = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
                                        self.switch_tab(prev);
                                    }
                                    return;
                                }
                                KeyCode::Digit1
                                | KeyCode::Digit2
                                | KeyCode::Digit3
                                | KeyCode::Digit4
                                | KeyCode::Digit5
                                | KeyCode::Digit6
                                | KeyCode::Digit7
                                | KeyCode::Digit8 => {
                                    let n = code as usize - KeyCode::Digit1 as usize;
                                    self.switch_tab(n.min(self.tabs.len() - 1));
                                    return;
                                }
                                KeyCode::Digit9 => {
                                    self.switch_tab(self.tabs.len() - 1);
                                    return;
                                }
                                KeyCode::KeyD => {
                                    self.handle_chrome_action("toggle-bookmark", event_loop);
                                    return;
                                }
                                KeyCode::KeyQ if cfg!(target_os = "macos") => {
                                    self.exit_app(event_loop);
                                    return;
                                }
                                KeyCode::BracketLeft => {
                                    self.go_back();
                                    return;
                                }
                                KeyCode::BracketRight => {
                                    self.go_forward();
                                    return;
                                }
                                KeyCode::KeyO if shift => {
                                    self.close_popup();
                                    self.navigate_current("rust://bookmarks");
                                    return;
                                }
                                KeyCode::KeyT => {
                                    self.open_new_tab("rust://home");
                                    self.focus_url_bar();
                                    return;
                                }
                                KeyCode::KeyW => {
                                    self.close_tab(self.active_tab, Some(event_loop));
                                    return;
                                }
                                KeyCode::KeyL => {
                                    self.focus_url_bar();
                                    return;
                                }
                                KeyCode::KeyR => {
                                    self.reload_active_tab();
                                    return;
                                }
                                KeyCode::Tab => {
                                    if self.tabs.len() > 1 {
                                        let next = (self.active_tab + 1) % self.tabs.len();
                                        self.switch_tab(next);
                                    }
                                    return;
                                }
                                KeyCode::Equal | KeyCode::NumpadAdd => {
                                    self.set_zoom(self.tabs[self.active_tab].zoom + 0.1);
                                    return;
                                }
                                KeyCode::Minus | KeyCode::NumpadSubtract => {
                                    self.set_zoom(self.tabs[self.active_tab].zoom - 0.1);
                                    return;
                                }
                                KeyCode::Digit0 | KeyCode::Numpad0 => {
                                    self.set_zoom(1.0);
                                    return;
                                }
                                _ => {}
                            }
                        }
                        if alt {
                            match code {
                                KeyCode::ArrowLeft => {
                                    self.go_back();
                                    return;
                                }
                                KeyCode::ArrowRight => {
                                    self.go_forward();
                                    return;
                                }
                                KeyCode::KeyD => {
                                    self.focus_url_bar();
                                    return;
                                }
                                KeyCode::KeyR => {
                                    self.toggle_reader_mode();
                                    return;
                                }
                                KeyCode::Home => {
                                    self.navigate_current("rust://home");
                                    return;
                                }
                                _ => {}
                            }
                        }
                        if code == KeyCode::F5 {
                            self.reload_active_tab();
                            return;
                        }
                        if code == KeyCode::Escape {
                            if self.tabs[self.active_tab].is_loading && !self.tabs[self.active_tab].js_mode {
                                self.tabs[self.active_tab].cancel_inflight();
                                self.rebuild_chrome();
                                return;
                            }
                            if self.chrome_focused {
                                self.chrome_focused = false;
                                self.chrome_doc.clear_focus();
                                self.last_chrome_html.clear();
                                self.rebuild_chrome();
                                return;
                            }
                        }
                        if !ctrl && !alt && self.keyboard_scroll(code, shift) {
                            return;
                        }
                        if matches!(code, KeyCode::Enter | KeyCode::NumpadEnter) {
                            if self.chrome_focused {
                                if let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") {
                                    if self.chrome_doc.get_focussed_node_id() == Some(url_id) {
                                        if let Some(text) = self.read_url_bar_text() {
                                            self.navigate_current(&text);
                                        }
                                        return;
                                    }
                                }
                            } else if let Some(q) = self.focused_builtin_input("home-search") {
                                if !q.is_empty() {
                                    self.navigate_current(&q);
                                }
                                return;
                            } else if self.tabs[self.active_tab].url.starts_with("rust://bookmarks")
                            {
                                if let Some(q) = self.focused_builtin_input("bm-search") {
                                    let next_url = if q.is_empty() {
                                        "rust://bookmarks".to_string()
                                    } else {
                                        let encoded: String =
                                            url::form_urlencoded::byte_serialize(q.as_bytes())
                                                .collect();
                                        format!("rust://bookmarks?q={encoded}")
                                    };
                                    self.navigate_current(&next_url);
                                    return;
                                }
                            }
                        }
                    }
                }

                let key_data = winit_key_event_to_blitz(&event, self.keyboard_modifiers.state());
                let ui_ev = if event.state.is_pressed() {
                    UiEvent::KeyDown(key_data)
                } else {
                    UiEvent::KeyUp(key_data)
                };

                if self.chrome_focused {
                    self.dispatch_chrome_ui(ui_ev);
                } else if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.handle_ui_event(ui_ev);
                }
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
                let mods = winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state());
                let leave_chrome = BlitzPointerEvent {
                    id,
                    is_primary: primary,
                    coords: self.chrome_coords(pos),
                    button: Default::default(),
                    buttons: self.buttons,
                    mods,
                    details: PointerDetails::default(),
                    element: Default::default(),
                    active_pointers: Default::default(),
                };
                self.dispatch_chrome_ui(UiEvent::PointerLeave(leave_chrome));
                let leave_content_coords = self.content_coords(pos);
                if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    let leave_content = BlitzPointerEvent {
                        id,
                        is_primary: primary,
                        coords: leave_content_coords,
                        button: Default::default(),
                        buttons: self.buttons,
                        mods,
                        details: PointerDetails::default(),
                        element: Default::default(),
                        active_pointers: Default::default(),
                    };
                    doc.handle_ui_event(UiEvent::PointerLeave(leave_content));
                }
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
                let mods = winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state());
                let details = pointer_source_to_blitz_details(&source);
                let now_in_chrome = self.is_in_chrome(position);

                if self.pointer_in_chrome == Some(true) && !now_in_chrome {
                    let leave = BlitzPointerEvent {
                        id,
                        is_primary: primary,
                        coords: self.chrome_coords(position),
                        button: Default::default(),
                        buttons: self.buttons,
                        mods,
                        details: PointerDetails::default(),
                        element: Default::default(),
                        active_pointers: Default::default(),
                    };
                    self.dispatch_chrome_ui(UiEvent::PointerLeave(leave));
                } else if self.pointer_in_chrome == Some(false) && now_in_chrome {
                    let coords = self.content_coords(position);
                    if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                        let leave = BlitzPointerEvent {
                            id,
                            is_primary: primary,
                            coords,
                            button: Default::default(),
                            buttons: self.buttons,
                            mods,
                            details: PointerDetails::default(),
                            element: Default::default(),
                            active_pointers: Default::default(),
                        };
                        doc.handle_ui_event(UiEvent::PointerLeave(leave));
                    }
                }
                self.pointer_in_chrome = Some(now_in_chrome);

                if now_in_chrome {
                    let ev = BlitzPointerEvent {
                        id,
                        is_primary: primary,
                        coords: self.chrome_coords(position),
                        button: Default::default(),
                        buttons: self.buttons,
                        mods,
                        details,
                        element: Default::default(),
                        active_pointers: Default::default(),
                    };
                    self.dispatch_chrome_ui(UiEvent::PointerMove(ev));
                } else {
                    let coords = self.content_coords(position);
                    if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                        let ev = BlitzPointerEvent {
                            id,
                            is_primary: primary,
                            coords,
                            button: Default::default(),
                            buttons: self.buttons,
                            mods,
                            details,
                            element: Default::default(),
                            active_pointers: Default::default(),
                        };
                        doc.handle_ui_event(UiEvent::PointerMove(ev));
                    }
                }
                // No unconditional redraw: Blitz requests one when hover/selection changes.
            }
            WindowEvent::PointerButton {
                button,
                state,
                primary,
                position,
                ..
            } => {
                self.pointer_pos = position;
                let id = button_source_to_blitz(&button);
                let btn = match &button {
                    ButtonSource::Mouse(mb) => match mb {
                        MouseButton::Left => MouseEventButton::Main,
                        MouseButton::Right => MouseEventButton::Secondary,
                        _ => MouseEventButton::Auxiliary,
                    },
                    _ => MouseEventButton::Main,
                };

                // Frameless window: edge resize and title-bar drag / double-click maximize.
                if state == ElementState::Pressed && btn == MouseEventButton::Main {
                    if let Some(dir) = self.resize_direction_at(position) {
                        if let Some(win) = &self.window {
                            let _ = win.drag_resize_window(dir);
                        }
                        return;
                    }
                    if self.is_titlebar_drag_area(position) {
                        self.close_popup();
                        let double = self
                            .last_titlebar_press
                            .is_some_and(|t| t.elapsed().as_millis() < 450);
                        self.last_titlebar_press = Some(Instant::now());
                        if double {
                            self.last_titlebar_press = None;
                            self.toggle_maximize();
                        } else if let Some(win) = &self.window {
                            let _ = win.drag_window();
                        }
                        self.buttons = MouseEventButtons::None;
                        return;
                    }
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
                        self.last_chrome_click_x_logical =
                            (position.x / (self.hidpi_scale.max(1.0) as f64)) as f32;
                        if let Some(win) = &self.window {
                            win.focus_window();
                        }
                    }
                }

                let mods = winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state());
                let coords = if in_chrome {
                    self.chrome_coords(position)
                } else {
                    self.content_coords(position)
                };

                let pev = BlitzPointerEvent {
                    id,
                    is_primary: primary,
                    coords,
                    button: btn,
                    buttons: self.buttons,
                    mods,
                    details: PointerDetails::default(),
                    element: Default::default(),
                    active_pointers: Default::default(),
                };

                let ui_ev = match state {
                    ElementState::Pressed => UiEvent::PointerDown(pev),
                    ElementState::Released => UiEvent::PointerUp(pev),
                };

                if in_chrome {
                    self.dispatch_chrome_ui(ui_ev);
                } else if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                    doc.handle_ui_event(ui_ev);
                }

                self.process_app_events(event_loop);
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.is_in_chrome(self.pointer_pos) {
                    let blitz_delta = match delta {
                        winit::event::MouseScrollDelta::LineDelta(x, y) => {
                            BlitzWheelDelta::Lines(x as f64, y as f64)
                        }
                        winit::event::MouseScrollDelta::PixelDelta(p) => {
                            BlitzWheelDelta::Pixels(p.x, p.y)
                        }
                        _ => BlitzWheelDelta::Pixels(0.0, 0.0),
                    };
                    let coords = self.content_coords(self.pointer_pos);
                    let mods = winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state());
                    let buttons = self.buttons;
                    if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                        doc.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
                            delta: blitz_delta,
                            coords,
                            buttons,
                            mods,
                            element: Default::default(),
                        }));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Lets cached `file://` favicons decode before a freshly built UI document is shown, so
/// icons don't flicker on every chrome rebuild. Bounded to 40 ms.
fn wait_for_images(doc: &mut HtmlDocument) {
    let deadline = Instant::now() + std::time::Duration::from_millis(40);
    loop {
        doc.handle_messages();
        let pending = doc.tree().iter().any(|(_, node)| {
            node.element_data()
                .is_some_and(|el| &*el.name.local == "img" && el.image_data().is_none())
        });
        if !pending || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The JS-mode WebView is destroyed on tab switch to free memory, which would also drop its
/// session cookies (i.e. log the user out). Promote them to 30-day persistent cookies first so
/// logins survive tab switches and restarts.
fn persist_session_cookies(wv: &WebView) {
    let Ok(cookies) = wv.cookies() else {
        return;
    };
    for cookie in cookies {
        if cookie.expires_datetime().is_none() && cookie.max_age().is_none() {
            let mut kept = cookie.clone();
            kept.set_max_age(wry::cookie::time::Duration::days(30));
            let _ = wv.set_cookie(&kept);
        }
    }
}

/// Hosts whose sign-in / app flows only work with a real JS engine and the Windows account SSO
/// (Microsoft Entra Conditional Access checks device compliance via WebView2's OS-account SSO).
fn requires_js_engine(url: &str) -> bool {
    const JS_HOSTS: &[&str] = &[
        "login.microsoftonline.com",
        "login.microsoft.com",
        "login.live.com",
        "microsoft365.com",
        "office.com",
        "cloud.microsoft",
        "copilot.microsoft.com",
        "sharepoint.com",
        "teams.microsoft.com",
        "outlook.office.com",
        "accounts.google.com",
    ];
    let Some(host) = Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)) else {
        return false;
    };
    JS_HOSTS.iter().any(|d| {
        host.strip_suffix(d)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

/// Width (logical px) of the invisible resize frame around the frameless window.
const RESIZE_EDGE: u32 = 5;

fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
        ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
    }
}

fn synthetic_key(key: Key, code: Code, modifiers: KbtModifiers) -> BlitzKeyEvent {
    BlitzKeyEvent {
        key,
        code,
        modifiers,
        location: Location::Standard,
        is_auto_repeating: false,
        is_composing: false,
        state: KeyState::Pressed,
        text: None,
    }
}

/// 64x64 RGBA app icon generated by `assets/make_icon.py`.
fn app_icon() -> Option<winit::icon::Icon> {
    const RGBA: &[u8] = include_bytes!("../assets/icon_64.rgba");
    winit::icon::RgbaIcon::new(RGBA.to_vec(), 64, 64).ok().map(Into::into)
}

/// Windows: frameless (our own title bar/buttons) with Windows 11 shadow + rounded corners.
/// macOS: native traffic lights over a transparent, full-size-content title bar.
/// Other platforms: native decorations.
fn main_window_style(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::{CornerPreference, WindowAttributesWindows};
        attrs.with_decorations(false).with_platform_attributes(Box::new(
            WindowAttributesWindows::default()
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round),
        ))
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowAttributesMacOS;
        attrs.with_platform_attributes(Box::new(
            WindowAttributesMacOS::default()
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true),
        ))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        attrs
    }
}

fn popup_window_style(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::{CornerPreference, WindowAttributesWindows};
        attrs.with_platform_attributes(Box::new(
            WindowAttributesWindows::default()
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::RoundSmall),
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        attrs
    }
}

/// Text-editing / shortcut modifier: Cmd on macOS, Ctrl elsewhere (matches Blitz).
#[cfg(target_os = "macos")]
const ACTION_MOD: KbtModifiers = KbtModifiers::META;
#[cfg(not(target_os = "macos"))]
const ACTION_MOD: KbtModifiers = KbtModifiers::CONTROL;

/// True if the platform's primary shortcut modifier is held (Cmd on macOS, Ctrl elsewhere).
fn primary_modifier(state: winit::keyboard::ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        state.meta_key()
    } else {
        state.control_key()
    }
}

fn normalize_url_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "rust://home".to_string();
    }
    if trimmed.starts_with("rust://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
    {
        return trimmed.to_string();
    }
    if !trimmed.contains(char::is_whitespace)
        && (trimmed.contains('.') || trimmed.starts_with("localhost"))
    {
        return format!("https://{trimmed}");
    }
    let encoded: String = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect();
    format!("https://html.duckduckgo.com/html/?q={encoded}")
}

#[cfg(test)]
mod tests {
    use super::normalize_url_input;

    #[test]
    fn sign_in_hosts_use_js_engine() {
        use super::requires_js_engine;
        assert!(requires_js_engine("https://m365.cloud.microsoft/chat"));
        assert!(requires_js_engine("https://login.microsoftonline.com/common/oauth2"));
        assert!(requires_js_engine("https://contoso.sharepoint.com/sites/x"));
        assert!(!requires_js_engine("https://news.ycombinator.com/"));
        assert!(!requires_js_engine("https://notoffice.com/"));
        assert!(!requires_js_engine("rust://home"));
    }

    #[test]
    fn normalizes_urls_and_search_queries() {
        assert_eq!(normalize_url_input(""), "rust://home");
        assert_eq!(normalize_url_input("rust://bookmarks"), "rust://bookmarks");
        assert_eq!(normalize_url_input("example.com"), "https://example.com");
        assert_eq!(normalize_url_input("https://rust-lang.org"), "https://rust-lang.org");
        assert_eq!(
            normalize_url_input("rust browser engine"),
            "https://html.duckduckgo.com/html/?q=rust+browser+engine"
        );
    }
}

#[cfg(test)]
mod scroll_tests {
    use blitz_dom::DocumentConfig;
    use blitz_html::HtmlDocument;
    use blitz_traits::shell::{ColorScheme, Viewport};

    #[test]
    fn negative_viewport_delta_scrolls_down() {
        let mut doc = HtmlDocument::from_html(
            r#"<html><body><div style="height:5000px">tall</div></body></html>"#,
            DocumentConfig {
                viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        doc.scroll_viewport_by(0.0, -120.0);
        assert!(doc.viewport_scroll().y > 100.0, "{:?}", doc.viewport_scroll());
    }
}
