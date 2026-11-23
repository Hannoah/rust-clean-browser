//! The browser application: one winit window whose top strip is a Blitz-rendered UI document
//! ("chrome") and whose body is the active tab (a Blitz document, or a WebView in JS mode).
//!
//! - `navigation`: page loading, tabs, history, JS-mode WebView
//! - `actions`: UI `data-action` clicks and the async `AppEvent` queue
//! - `input`: keyboard shortcuts, pointer routing, frameless-window drag/resize
//! - `popup`: popup windows (bookmark folders, address-bar suggestions)
//! - `suggest`: address-bar autocomplete
//! - `bus`: Blitz/wry callback adapters that post `AppEvent`s

mod actions;
mod bus;
mod find;
mod helpers;
mod input;
mod navigation;
mod palette;
mod popup;
mod suggest;

use crate::bookmarks::{BookmarkNode, BookmarkRootKind, BookmarkStore};
use crate::chrome_ui::{ChromeRenderState, chrome_height, render_chrome_html, render_link_preview_html};
use crate::cleaner::{build_reader_html_with_options, extract_title};
use crate::events::{theme_to_color_scheme, winit_modifiers_to_kbt_modifiers};
use crate::favicon;
use crate::history::History;
use crate::memory::working_set_mb;
use crate::net::{CleanNetProvider, PersistentCookieJar};
use crate::page_cache::DiskPageCache;
use crate::prefs::{ThemePreference, UiPrefs};
use crate::tabs::{HtmlCache, Tab, short_title_for_url};
use bus::{AppBus, BrowserNavProvider, BrowserShellProvider, ChromeEventHandler};
use helpers::{ACTION_MOD, app_icon, arg_to_url, main_window_style, synthetic_key, wait_for_images};

use anyrender::{PaintScene, WindowRenderer};
use anyrender_vello_cpu::VelloCpuWindowRenderer;
use blitz_dom::{Document, DocumentConfig, EventDriver, FontContext, StyleThreading};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::events::{MouseEventButtons, UiEvent};
use blitz_traits::net::{AbortController, Request};
use blitz_traits::shell::{ColorScheme, ShellProvider, Viewport};
use keyboard_types::{Code, Key, Modifiers as KbtModifiers, NamedKey};
use kurbo::{Affine, Rect};
use peniko::Fill;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{ResizeDirection, Theme, Window, WindowAttributes, WindowId};
use wry::{WebContext, WebView};

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
    /// Page translation finished for the tab showing `source`.
    Translated {
        tab_id: u64,
        source: Arc<str>,
        tl: &'static str,
        result: Result<Arc<str>, String>,
    },
    /// Another launch (e.g. double-clicked .html file) forwarded a path/URL to this instance.
    OpenUrl(String),
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
    /// Ctrl+wheel / zoom key inside a JS-mode page (WebView2 zoom hotkeys are off so the tab zoom stays authoritative).
    ZoomBy(f32),
    /// Raw `deltaY` of a Ctrl+wheel in a page (CSS px: Chromium divides it by the current zoom, 100 per notch at 100%).
    ZoomWheelPx(f32),
    ZoomReset,
    /// A WebView download was denied for lack of a consent UI.
    DownloadBlocked(String),
    /// An allowed (PDF) download finished.
    DownloadDone {
        path: Option<std::path::PathBuf>,
        success: bool,
    },
    /// A queued favicon fetch finished.
    Favicon(favicon::Next),
    /// `<link>`s of the page shown in the JS-mode WebView.
    WebViewIcons {
        page_url: String,
        links: Vec<favicon::Link>,
    },
    ChromeAction(String),
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

pub(super) struct BookmarkDrag {
    pub src_spec: String,
    pub title: String,
    pub start_pos: PhysicalPosition<f64>,
    pub active: bool,
    pub hover_folder: Option<String>,
}

/// CLI launch options (supports normal launch, file/URL open, and tab-detach `--new-window`).
#[derive(Debug, Default, PartialEq)]
pub struct LaunchOptions {
    pub initial_url: Option<String>,
    pub new_window: bool,
    pub js_mode: bool,
    pub reader_mode: bool,
    pub zoom: Option<f32>,
    /// Initial window position in physical screen coordinates.
    pub position: Option<(i32, i32)>,
    /// Initial window surface size in logical pixels.
    pub size: Option<(u32, u32)>,
}

impl LaunchOptions {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut opts = Self::default();
        let mut iter = args.into_iter();
        let mut positional_only = false;
        while let Some(arg) = iter.next() {
            if !positional_only {
                match arg.as_str() {
                    "--" => {
                        positional_only = true;
                        continue;
                    }
                    "--new-window" => {
                        opts.new_window = true;
                        continue;
                    }
                    "--js" => {
                        opts.js_mode = true;
                        continue;
                    }
                    "--reader" => {
                        opts.reader_mode = true;
                        continue;
                    }
                    "--zoom" => {
                        if let Some(val) = iter.next()
                            && let Ok(z) = val.parse::<f32>()
                            && z.is_finite()
                        {
                            opts.zoom = Some(((z * 10.0).round() / 10.0).clamp(0.3, 3.0));
                        }
                        continue;
                    }
                    "--pos" => {
                        if let Some(val) = iter.next()
                            && let Some((xs, ys)) = val.split_once(',')
                            && let (Ok(x), Ok(y)) = (xs.parse::<i32>(), ys.parse::<i32>())
                        {
                            opts.position = Some((x, y));
                        }
                        continue;
                    }
                    "--size" => {
                        if let Some(val) = iter.next()
                            && let Some((ws, hs)) = val.split_once('x')
                            && let (Ok(w), Ok(h)) = (ws.parse::<u32>(), hs.parse::<u32>())
                        {
                            opts.size = Some((w.clamp(520, 4096), h.clamp(360, 2160)));
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if opts.initial_url.is_none() {
                opts.initial_url = Some(arg);
            }
        }
        opts
    }

    pub(super) fn to_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.new_window {
            out.push("--new-window".to_string());
        }
        if self.js_mode {
            out.push("--js".to_string());
        }
        if self.reader_mode {
            out.push("--reader".to_string());
        }
        if let Some(z) = self.zoom {
            out.push("--zoom".to_string());
            out.push(format!("{z:.1}"));
        }
        if let Some((x, y)) = self.position {
            out.push("--pos".to_string());
            out.push(format!("{x},{y}"));
        }
        if let Some((w, h)) = self.size {
            out.push("--size".to_string());
            out.push(format!("{w}x{h}"));
        }
        if let Some(url) = &self.initial_url {
            out.push("--".to_string());
            out.push(url.clone());
        }
        out
    }
}

pub(super) struct TabDrag {
    pub tab_id: u64,
    pub start_pos: PhysicalPosition<f64>,
    pub active: bool,
    /// True while dragged outside the tab strip (not over a bookmark folder) to open in a new window.
    pub detaching: bool,
}

pub(super) struct ClosedTab {
    pub url: String,
    pub js_mode: bool,
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
    history: History,
    /// Last URL-bar text the suggestion dropdown was built for.
    suggest_query: String,
    html_cache: HtmlCache,
    page_cache: DiskPageCache,
    /// Pending favicon downloads, fetched one at a time while no page is loading.
    favicon_queue: VecDeque<favicon::Job>,
    favicon_inflight: bool,
    favicon_abort: Option<AbortController>,

    win_size: PhysicalSize<u32>,
    hidpi_scale: f32,
    os_color_scheme: ColorScheme,
    color_scheme: ColorScheme,
    prefs: UiPrefs,
    pointer_pos: PhysicalPosition<f64>,
    pointer_in_chrome: Option<bool>,
    buttons: MouseEventButtons,
    keyboard_modifiers: Modifiers,
    /// Fractional zoom from Ctrl+wheel not yet applied (set_zoom snaps to 0.1).
    zoom_wheel_acc: f32,
    chrome_focused: bool,
    start_instant: Instant,
    status_note: Option<String>,
    status_note_until: Option<Instant>,
    hover_link_url: Option<String>,
    link_preview_doc: Option<(HtmlDocument, u32)>,
    web_context: Option<WebContext>,
    active_webview: Option<WebView>,
    /// Up to 5 background JS-mode WebViews kept alive (oldest first, MRU last; hidden + suspended).
    parked_webviews: VecDeque<(u64, WebView)>,
    popup: Option<popup::Popup>,
    recently_closed_popup: Option<(String, Instant)>,
    last_chrome_click_x_logical: f32,
    /// Last chrome HTML; identical rebuilds are skipped (no re-parse / re-style).
    last_chrome_html: String,
    /// Recently closed tabs (`Ctrl+Shift+T`), newest last.
    closed_tabs: Vec<ClosedTab>,
    last_titlebar_press: Option<Instant>,
    resize_hover: Option<ResizeDirection>,
    maximized: bool,
    /// Target timestamp for the next animation frame (~60 FPS cap when chrome/content animates).
    next_anim_frame: Option<Instant>,
    /// Active mouse drag of a bookmark or tab onto a bookmark folder.
    bm_drag: Option<BookmarkDrag>,
    /// Active horizontal drag of a tab on the tab strip to reorder tabs.
    tab_drag: Option<TabDrag>,
    /// PDFs opened in the pdf.js annotator (see `pdfjs.rs`).
    pdf_sources: Arc<crate::pdfjs::PdfSources>,
    /// Set when clicking an unfocused `#url-bar`, so a click without drag selects all text.
    url_bar_select_all_on_click: Option<PhysicalPosition<f64>>,
    /// Requested initial position (physical px) when launched via tab tear-off (`--pos`).
    initial_position: Option<PhysicalPosition<i32>>,
    /// Requested initial surface size (logical px) when launched via tab tear-off (`--size`).
    initial_size: Option<LogicalSize<f64>>,
}

impl BrowserApp {
    pub fn new(
        winit_proxy: EventLoopProxy,
        launch: LaunchOptions,
        bookmarks: std::thread::JoinHandle<BookmarkStore>,
    ) -> Self {
        let (tx, rx) = channel();
        let bus = AppBus { tx, winit_proxy };
        // Secondary windows launched via `--new-window` keep the primary instance as the single-instance target.
        if !launch.new_window {
            let forwarded = bus.clone();
            crate::single::listen(move |u| forwarded.send(AppEvent::OpenUrl(u)));
        }
        let shared_font_ctx = FontContext::default();
        let cookie_jar = Arc::new(PersistentCookieJar::load(
            crate::bookmarks::app_data_dir().map(|p| p.join("cookies.json")),
        ));
        let net_provider = Arc::new(CleanNetProvider::new(Some(Arc::new(bus.clone())), cookie_jar));
        let nav_provider = Arc::new(BrowserNavProvider { bus: bus.clone() });
        let bookmarks = bookmarks.join().unwrap_or_default();

        let start_url = launch
            .initial_url
            // Local files (e.g. opened via the .html file association) become file:// URLs.
            .map(|u| arg_to_url(&u))
            .unwrap_or_else(|| "rust://home".to_string());
        let mut first_tab = Tab::new(1, start_url);
        if !first_tab.url.starts_with("rust://") {
            first_tab.js_mode = launch.js_mode;
            first_tab.reader_mode = launch.reader_mode && !launch.js_mode;
        }
        if let Some(z) = launch.zoom {
            first_tab.zoom = ((z * 10.0).round() / 10.0).clamp(0.3, 3.0);
        }

        let chrome_doc = HtmlDocument::from_html(
            "<html></html>",
            DocumentConfig {
                font_ctx: Some(shared_font_ctx.clone()),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );

        let prefs = UiPrefs::load();
        let initial_scheme = match prefs.theme {
            ThemePreference::Dark => ColorScheme::Dark,
            _ => ColorScheme::Light,
        };
        let mut app = Self {
            pdf_sources: Arc::default(),
            window: None,
            renderer: VelloCpuWindowRenderer::new(),
            bus,
            rx,
            shared_font_ctx,
            net_provider,
            nav_provider,
            shell_provider: None,
            chrome_doc,
            tabs: vec![first_tab],
            active_tab: 0,
            next_tab_id: 2,
            bookmarks,
            history: History::load(crate::bookmarks::app_data_dir().map(|p| p.join("history.json"))),
            suggest_query: String::new(),
            html_cache: HtmlCache::new(8, 2 * 1024 * 1024),
            page_cache: DiskPageCache::new(crate::bookmarks::app_data_dir().map(|p| p.join("pages"))),
            favicon_queue: VecDeque::new(),
            favicon_inflight: false,
            favicon_abort: None,
            win_size: PhysicalSize::new(1280, 820),
            hidpi_scale: 1.0,
            os_color_scheme: ColorScheme::Light,
            color_scheme: initial_scheme,
            prefs,
            pointer_pos: PhysicalPosition::new(0.0, 0.0),
            pointer_in_chrome: None,
            buttons: MouseEventButtons::None,
            keyboard_modifiers: Modifiers::default(),
            zoom_wheel_acc: 0.0,
            chrome_focused: false,
            start_instant: Instant::now(),
            status_note: None,
            status_note_until: None,
            hover_link_url: None,
            link_preview_doc: None,
            web_context: None,
            active_webview: None,
            parked_webviews: VecDeque::new(),
            popup: None,
            recently_closed_popup: None,
            last_chrome_click_x_logical: 24.0,
            last_chrome_html: String::new(),
            closed_tabs: Vec::new(),
            last_titlebar_press: None,
            resize_hover: None,
            maximized: false,
            next_anim_frame: None,
            bm_drag: None,
            tab_drag: None,
            url_bar_select_all_on_click: None,
            initial_position: launch.position.map(|(x, y)| PhysicalPosition::new(x, y)),
            initial_size: launch.size.map(|(w, h)| LogicalSize::new(w as f64, h as f64)),
        };

        app.load_active_tab(false, None);
        app
    }

    fn anim_time(&self) -> f64 {
        self.start_instant.elapsed().as_secs_f64()
    }

    fn mods(&self) -> KbtModifiers {
        winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state())
    }

    fn ensure_active_valid(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(self.next_tab_id, "rust://home".to_string()));
            self.next_tab_id += 1;
            self.active_tab = 0;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub(super) fn chrome_height(&self) -> u32 {
        chrome_height(self.prefs.show_bookmarks_bar)
    }

    fn chrome_phys_height(&self) -> u32 {
        ((self.chrome_height() as f32) * self.hidpi_scale).round() as u32
    }

    fn effective_color_scheme(&self) -> ColorScheme {
        match self.prefs.theme {
            ThemePreference::System => self.os_color_scheme,
            ThemePreference::Light => ColorScheme::Light,
            ThemePreference::Dark => ColorScheme::Dark,
        }
    }

    pub(super) fn apply_effective_color_scheme(&mut self) {
        self.color_scheme = self.effective_color_scheme();
        self.ensure_active_valid();
        let tab = &self.tabs[self.active_tab];
        if tab.url.starts_with("rust://") {
            let scroll = self.current_scroll_y();
            self.load_active_tab(false, Some(scroll));
        } else if tab.reader_mode
            && let Some(raw) = tab.shown_html()
        {
            let url = tab.url.clone();
            let scroll = self.current_scroll_y();
            self.apply_html_to_tab(self.active_tab, url, raw, Some(scroll));
        }
        self.sync_viewports();
        self.rebuild_chrome();
        self.refresh_popup();
    }

    pub(super) fn cycle_theme(&mut self) {
        self.prefs.theme = self.prefs.theme.next();
        self.prefs.save();
        self.set_status_note(&format!("테마: {}", self.prefs.theme.label()));
        self.apply_effective_color_scheme();
    }

    pub(super) fn toggle_bookmarks_bar(&mut self) {
        self.prefs.show_bookmarks_bar = !self.prefs.show_bookmarks_bar;
        self.prefs.save();
        self.set_status_note(if self.prefs.show_bookmarks_bar {
            "북마크 바 표시"
        } else {
            "북마크 바 숨김"
        });
        self.sync_viewports();
        self.rebuild_chrome();
    }

    pub(super) fn set_status_note(&mut self, msg: &str) {
        self.status_note = Some(msg.to_string());
        self.status_note_until = Some(Instant::now() + Duration::from_millis(2800));
    }

    pub(super) fn set_hover_link(&mut self, next: Option<String>) {
        if self.hover_link_url == next {
            return;
        }
        self.hover_link_url = next.clone();
        self.link_preview_doc = next.map(|url| {
            let char_w = (url.chars().count().min(72) as u32) * 7 + 138;
            let max_w = ((self.win_size.width as f32 / self.hidpi_scale.max(1.0)) * 0.65) as u32;
            let w = char_w.clamp(190, max_w.max(220));
            let h = 36u32;
            let phys_w = ((w as f32) * self.hidpi_scale).round().max(1.0) as u32;
            let phys_h = ((h as f32) * self.hidpi_scale).round().max(1.0) as u32;
            let vp = Viewport::new(phys_w, phys_h, self.hidpi_scale, self.color_scheme);
            let html = render_link_preview_html(&url);
            let doc = HtmlDocument::from_html(&html, self.doc_config(vp, None));
            (doc, w)
        });
        self.request_redraw();
    }

    fn content_phys_size(&self) -> (u32, u32) {
        let ch = self.chrome_phys_height();
        (
            self.win_size.width.max(1),
            self.win_size.height.saturating_sub(ch).max(1),
        )
    }

    fn chrome_viewport(&self) -> Viewport {
        let ch = self.chrome_phys_height();
        Viewport::new(
            self.win_size.width.max(1),
            ch.max(1),
            self.hidpi_scale,
            self.color_scheme,
        )
    }

    fn content_viewport(&self, zoom: f32) -> Viewport {
        let (cw, ch) = self.content_phys_size();
        let mut vp = Viewport::new(cw, ch, self.hidpi_scale, self.color_scheme);
        vp.zoom = zoom;
        vp
    }

    /// Re-applies size / scale / colour scheme to the chrome, the page and the WebView.
    fn sync_viewports(&mut self) {
        self.ensure_active_valid();
        let chrome_vp = self.chrome_viewport();
        self.chrome_doc.set_viewport(chrome_vp);
        let content_vp = self.content_viewport(self.tabs[self.active_tab].zoom);
        if let Some(doc) = &mut self.tabs[self.active_tab].doc {
            doc.set_viewport(content_vp);
        }
        if let Some(wv) = &self.active_webview {
            let _ = wv.set_bounds(self.webview_bounds());
        }
        self.request_redraw();
    }

    /// Shared document setup: one font context, our net provider, single-threaded styling.
    fn doc_config(&self, viewport: Viewport, shell: Option<Arc<dyn ShellProvider>>) -> DocumentConfig {
        DocumentConfig {
            viewport: Some(viewport),
            font_ctx: Some(self.shared_font_ctx.clone()),
            net_provider: Some(self.net_provider.clone()),
            shell_provider: shell,
            html_parser_provider: Some(Arc::new(blitz_html::HtmlProvider)),
            style_threading: StyleThreading::Sequential,
            ..Default::default()
        }
    }

    fn url_bar_focused(&self) -> bool {
        self.chrome_focused
            && self
                .chrome_doc
                .get_element_by_id("url-bar")
                .is_some_and(|id| self.chrome_doc.get_focussed_node_id() == Some(id))
    }

    /// Queues a one-time icon lookup for a URL we haven't rendered (tab strip, bookmarks).
    /// Downloads run one at a time so they never compete with page loads.
    fn request_favicon(&mut self, page_url: &str) {
        if let Some(job) = favicon::begin_fetch(page_url) {
            self.favicon_queue.push_back(job);
        }
    }

    /// Same, for a rendered page whose `<link>`s are known: uses the icon it declares.
    fn request_favicon_known(&mut self, page_url: &str, links: &[favicon::Link]) {
        if let Some(job) = favicon::begin_fetch_known(page_url, links) {
            self.favicon_queue.push_back(job);
        }
    }

    fn pump_favicons(&mut self) {
        self.ensure_active_valid();
        // Watchdog: if a previous fetch never called back, don't stall the queue forever.
        if self.favicon_inflight && self.favicon_queue.len() > 32 {
            self.favicon_inflight = false;
            self.favicon_abort = None;
        }
        if self.favicon_inflight || self.tabs[self.active_tab].is_loading {
            return;
        }
        let Some(job) = self.favicon_queue.pop_front() else {
            return;
        };
        self.favicon_inflight = true;
        let (bus_ok, bus_fail) = (self.bus.clone(), self.bus.clone());
        // Keep the controller alive for the fetch lifetime; dropping it
        // mid-fetch could cancel the signal on some providers.
        let abort = AbortController::default();
        let signal = abort.signal.clone();
        self.favicon_abort = Some(abort);
        match job {
            favicon::Job::Page { key, url } => {
                let (key2, url2) = (key.clone(), url.clone());
                self.net_provider.fetch_document(
                    Request::get(url.clone()),
                    signal,
                    move |final_url, bytes| {
                        let next = favicon::page_fetched(key, &url, &final_url, Some(&bytes));
                        bus_ok.send(AppEvent::Favicon(favicon::Next::Then(next)));
                    },
                    move |_| {
                        let next = favicon::page_fetched(key2, &url2, "", None);
                        bus_fail.send(AppEvent::Favicon(favicon::Next::Then(next)));
                    },
                );
            }
            favicon::Job::Icon {
                key,
                url,
                fallback,
                guess,
            } => {
                let (key2, fallback2) = (key.clone(), fallback.clone());
                self.net_provider.fetch_document(
                    Request::get(url),
                    signal,
                    move |_, bytes| {
                        bus_ok.send(AppEvent::Favicon(favicon::icon_fetched(
                            key,
                            fallback,
                            guess,
                            Some(&bytes),
                        )));
                    },
                    move |_| {
                        bus_fail.send(AppEvent::Favicon(favicon::icon_fetched(key2, fallback2, guess, None)));
                    },
                );
            }
        }
    }

    /// Asks the JS-mode WebView for the page's `<link>`s (SPAs often add their icon late,
    /// so this runs on load and on title changes).
    fn query_webview_icons(&self) {
        let Some(wv) = &self.active_webview else {
            return;
        };
        let page_url = self.tabs[self.active_tab].url.clone();
        let bus = self.bus.clone();
        let _ = wv.evaluate_script_with_callback(
            "Array.from(document.querySelectorAll('link[rel]'), l => ({rel: l.rel, href: l.href, sizes: l.getAttribute('sizes') || '', type: l.type || ''}))",
            move |json| {
                if let Ok(links) = serde_json::from_str::<Vec<favicon::Link>>(&json) {
                    bus.send(AppEvent::WebViewIcons { page_url: page_url.clone(), links });
                }
            },
        );
    }

    fn rebuild_chrome(&mut self) {
        // Icons for open tabs and the bookmark bar (deduplicated per host, fetched once).
        // Skip tabs that are currently loading so `apply_html_to_tab` / `WebViewIcons` can use
        // the page's real `<link rel=icon>` without triggering a duplicate page fetch.
        let icon_urls: Vec<String> = self
            .tabs
            .iter()
            .filter(|t| !t.is_loading)
            .map(|t| t.url.clone())
            .chain(self.bookmarks.custom.iter().map(|b| b.url.clone()))
            .chain(
                self.bookmarks
                    .bar
                    .iter()
                    .filter_map(|n| match n {
                        BookmarkNode::Url(item) => Some(item.url.clone()),
                        BookmarkNode::Folder(_) => None,
                    })
                    .take(16),
            )
            .collect();
        for url in &icon_urls {
            self.request_favicon(url);
        }
        self.pump_favicons();
        // Never rebuild `chrome_doc` while the user is mid-syllable in an IME preedit inside
        // `#url-bar`, because recreating the `PlainEditor` would drop the active compose range.
        if self.url_bar_focused() && self.url_bar_composing() {
            self.update_window_title();
            return;
        }
        // Keep whatever the user is typing (and focus / caret) when background events rebuild chrome.
        let typing = self.url_bar_focused().then(|| self.read_url_bar_text()).flatten();
        let saved_sel_range = if typing.is_some() {
            self.read_url_bar_selection()
        } else {
            None
        };
        let drop_target = self.bm_drag.as_ref().and_then(|d| d.hover_folder.as_deref());
        let detaching_tab_id = self
            .tab_drag
            .as_ref()
            .filter(|d| d.active && d.detaching)
            .map(|d| d.tab_id);
        let status_note = if detaching_tab_id.is_some() {
            Some("놓으면 새 창으로 열기")
        } else {
            self.status_note.as_deref()
        };
        let active = &self.tabs[self.active_tab];
        let (wv_back, wv_fwd) = self.active_webview.as_ref().map_or((false, false), |wv| {
            (wv.can_go_back().unwrap_or(false), wv.can_go_forward().unwrap_or(false))
        });
        let can_back = !active.history_back.is_empty() || wv_back;
        let can_forward = !active.history_forward.is_empty() || wv_fwd;
        let html = render_chrome_html(&ChromeRenderState {
            tabs: &self.tabs,
            active_idx: self.active_tab,
            bookmarks: &self.bookmarks,
            can_back,
            can_forward,
            working_set_mb: working_set_mb(),
            blocked_count: self.net_provider.blocked_total(),
            filter_enabled: self.net_provider.is_filter_enabled(),
            maximized: self.maximized,
            dark_mode: self.color_scheme == ColorScheme::Dark,
            show_bookmarks_bar: self.prefs.show_bookmarks_bar,
            theme_pref: self.prefs.theme,
            status_note,
            url_override: typing.as_deref(),
            drop_target_folder: drop_target,
            detaching_tab_id,
        });
        self.update_window_title();
        if html == self.last_chrome_html {
            return;
        }

        let mut doc = HtmlDocument::from_html(
            &html,
            self.doc_config(self.chrome_viewport(), self.shell_provider.clone()),
        );
        wait_for_images(&mut doc);
        doc.resolve(self.anim_time());
        self.chrome_doc = doc;
        self.last_chrome_html = html;
        if let Some(text) = typing {
            self.restore_url_bar_focus_and_caret(&text, saved_sel_range);
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

    fn build_content_doc(
        &self,
        html: &str,
        base_url: &str,
        abort_ctrl: Option<&AbortController>,
        zoom: f32,
    ) -> HtmlDocument {
        let mut cfg = self.doc_config(self.content_viewport(zoom), self.shell_provider.clone());
        cfg.base_url = Some(base_url.to_string());
        cfg.navigation_provider = Some(self.nav_provider.clone());
        cfg.abort_signal = abort_ctrl.map(|c| c.signal.clone());
        HtmlDocument::from_html(html, cfg)
    }

    /// Restores a saved scroll offset, or jumps to the URL's `#fragment`.
    fn restore_position(&self, doc: &mut HtmlDocument, url: &str, scroll_y: Option<f64>) {
        if let Some(y) = scroll_y {
            doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y });
        } else if let Some((_, frag)) = url.split_once('#').filter(|(_, f)| !f.is_empty()) {
            doc.resolve(self.anim_time());
            doc.scroll_to_fragment(frag);
        }
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
            let dark = self.color_scheme == ColorScheme::Dark;
            let reader = build_reader_html_with_options(&clean_html, &final_url, self.prefs.reader, dark);
            self.build_content_doc(&reader, &final_url, Some(&abort_ctrl), zoom)
        } else {
            self.build_content_doc(&clean_html, &final_url, Some(&abort_ctrl), zoom)
        };
        self.restore_position(&mut doc, &final_url, restore_scroll_y);

        self.request_favicon_known(&final_url, &favicon::links_in_html(&clean_html));
        let tab = &mut self.tabs[tab_idx];
        tab.url = final_url.clone();
        tab.title = title;
        tab.is_loading = false;
        tab.is_error_page = false;
        tab.abort_controller = Some(abort_ctrl);
        // Showing the translated copy (set by the Translated event): keep the original in `raw_html`.
        let translated = tab.translated.as_ref().is_some_and(|t| Arc::ptr_eq(t, &clean_html));
        if !translated {
            tab.raw_html = Some(Arc::clone(&clean_html));
            tab.translated = None;
        }
        tab.doc = Some(doc);

        if !translated && !final_url.starts_with("rust://") {
            let cache_url = final_url.split('#').next().unwrap_or(&final_url).to_string();
            self.html_cache.insert(cache_url, clean_html);
        }

        if tab_idx == self.active_tab {
            self.sync_find_bar_after_doc_change();
        }
        self.rebuild_chrome();
    }

    pub(super) fn exit_app(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.close_popup();
        // All WebViews share one `WebContext` / `CookieManager`, so persisting once is enough.
        if let Some(wv) = self
            .active_webview
            .as_ref()
            .or_else(|| self.parked_webviews.front().map(|(_, wv)| wv))
        {
            helpers::persist_session_cookies(wv);
        }
        self.net_provider.cookies.save();
        self.history.save();
        self.renderer.suspend();
        self.window = None;
        event_loop.exit();
    }

    fn focus_url_bar(&mut self) {
        self.chrome_focused = true;
        self.suggest_query.clear();
        if let Some(doc) = &mut self.tabs[self.active_tab].doc {
            doc.clear_focus();
        }
        if let Some(win) = &self.window {
            helpers::focus_main_window(&**win, true);
            bus::enable_window_ime(&**win);
        }
        self.select_all_url_bar();
        self.request_redraw();
    }

    pub(super) fn select_all_url_bar(&mut self) {
        if let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") {
            self.chrome_doc.set_focus_to(url_id);
            let select_all = synthetic_key(Key::Character("a".to_string()), Code::KeyA, ACTION_MOD);
            self.dispatch_chrome_ui(UiEvent::KeyDown(select_all));
        }
    }

    /// Trimmed text of a focused `<input id=...>` on a built-in `rust://` page (including IME preedit).
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
        Some(input.editor.raw_text().trim().to_string())
    }

    pub(super) fn toggle_maximize(&mut self) {
        if let Some(win) = &self.window {
            win.set_maximized(!win.is_maximized());
        }
    }

    fn read_url_bar_selection(&self) -> Option<std::ops::Range<usize>> {
        let url_id = self.chrome_doc.get_element_by_id("url-bar")?;
        let input = self.chrome_doc.get_node(url_id)?.element_data()?.text_input_data()?;
        Some(input.editor.raw_selection().text_range())
    }

    fn restore_url_bar_focus_and_caret(&mut self, text: &str, saved: Option<std::ops::Range<usize>>) {
        let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") else {
            return;
        };
        self.chrome_doc.set_focus_to(url_id);
        let Some(range) = saved else {
            self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
                Key::Named(NamedKey::End),
                Code::End,
                KbtModifiers::empty(),
            )));
            return;
        };
        if range.start == 0 && range.end == text.len() && !text.is_empty() {
            self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
                Key::Character("a".to_string()),
                Code::KeyA,
                ACTION_MOD,
            )));
            return;
        }
        let pos = range.end.min(text.len());
        if range.start == range.end && pos < text.len() && text.is_char_boundary(pos) {
            let from_start = text[..pos].chars().count();
            let from_end = text[pos..].chars().count();
            if from_start <= from_end && from_start <= 256 {
                self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
                    Key::Named(NamedKey::Home),
                    Code::Home,
                    KbtModifiers::empty(),
                )));
                for _ in 0..from_start {
                    self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
                        Key::Named(NamedKey::ArrowRight),
                        Code::ArrowRight,
                        KbtModifiers::empty(),
                    )));
                }
                return;
            }
            if from_end <= 256 {
                self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
                    Key::Named(NamedKey::End),
                    Code::End,
                    KbtModifiers::empty(),
                )));
                for _ in 0..from_end {
                    self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
                        Key::Named(NamedKey::ArrowLeft),
                        Code::ArrowLeft,
                        KbtModifiers::empty(),
                    )));
                }
                return;
            }
        }
        self.dispatch_chrome_ui(UiEvent::KeyDown(synthetic_key(
            Key::Named(NamedKey::End),
            Code::End,
            KbtModifiers::empty(),
        )));
    }

    fn read_url_bar_text(&self) -> Option<String> {
        let url_id = self.chrome_doc.get_element_by_id("url-bar")?;
        let input = self.chrome_doc.get_node(url_id)?.element_data()?.text_input_data()?;
        Some(input.editor.raw_text().to_string())
    }

    /// True when `#url-bar` has an in-progress IME preedit syllable (`raw_text` != committed `text`).
    pub(super) fn url_bar_composing(&self) -> bool {
        let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") else {
            return false;
        };
        let Some(input) = self
            .chrome_doc
            .get_node(url_id)
            .and_then(|n| n.element_data())
            .and_then(|e| e.text_input_data())
        else {
            return false;
        };
        input.editor.raw_text().len() != input.editor.text().to_string().len()
    }

    fn dispatch_chrome_ui(&mut self, event: UiEvent) {
        let handler = ChromeEventHandler { bus: self.bus.clone() };
        EventDriver::new(&mut self.chrome_doc, handler).handle_ui_event(event);
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

        let mut needs_more_frames = self.chrome_doc.is_animating();
        // With a WebView mounted, it draws the content area itself.
        let content_doc = if self.active_webview.is_some() {
            None
        } else {
            self.tabs[self.active_tab].doc.as_mut()
        };
        let content = content_doc.map(|doc| {
            needs_more_frames |= doc.poll(None);
            doc.resolve(anim_time);
            needs_more_frames |= doc.is_animating();
            let doc_scale = doc.viewport().scale_f64();
            (doc, doc_scale)
        });
        let chrome_inner = &mut *self.chrome_doc;
        let preview = if self.active_webview.is_none() {
            self.link_preview_doc.as_mut().map(|(doc, w_logical)| {
                doc.resolve(anim_time);
                let pw = (((*w_logical as f32) * self.hidpi_scale).round().max(1.0) as u32).min(win_w);
                let ph = ((36.0 * self.hidpi_scale).round().max(1.0) as u32).min(win_h);
                let px = ((6.0 * self.hidpi_scale).round() as u32).min(win_w.saturating_sub(pw));
                let py = win_h.saturating_sub(ph + (6.0 * self.hidpi_scale).round() as u32);
                (&mut **doc, pw, ph, px, py)
            })
        } else {
            None
        };

        self.renderer.render(|scene| {
            if let Some((content_doc, content_scale)) = content
                && content_h_phys > 0
            {
                let clip = Rect::new(0.0, chrome_h_phys as f64, win_w as f64, win_h as f64);
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
            if let Some((pdoc, pw, ph, px, py)) = preview
                && content_h_phys > ph
            {
                paint_scene(scene, pdoc, scale, pw, ph, px, py);
            }
            paint_scene(scene, chrome_inner, scale, win_w, chrome_h_phys, 0, 0);
        });

        self.next_anim_frame = if needs_more_frames {
            Some(Instant::now() + Duration::from_millis(16))
        } else {
            None
        };
    }
}

impl ApplicationHandler for BrowserApp {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        #[cfg(debug_assertions)]
        eprintln!("[winit] can_create_surfaces window_is_none={}", self.window.is_none());
        if self.window.is_none() {
            let init_size = self.initial_size.take().unwrap_or(LogicalSize::new(1280.0, 820.0));
            let mut attrs = WindowAttributes::default()
                .with_title("Rust Clean Browser")
                .with_min_surface_size(LogicalSize::new(520.0, 360.0))
                .with_surface_size(init_size)
                .with_window_icon(app_icon());
            if let Some(pos) = self.initial_position.take() {
                attrs = attrs.with_position(pos);
            }
            let win: Arc<dyn Window> = match event_loop.create_window(main_window_style(attrs)) {
                Ok(w) => Arc::from(w),
                Err(e) => {
                    eprintln!("[winit] create_window failed: {e}");
                    return;
                }
            };
            bus::enable_window_ime(&*win);
            self.maximized = win.is_maximized();
            self.hidpi_scale = win.scale_factor() as f32;
            self.win_size = win.surface_size();
            let prev_scheme = self.color_scheme;
            self.os_color_scheme = theme_to_color_scheme(win.theme().unwrap_or(Theme::Light));
            self.color_scheme = self.effective_color_scheme();
            let shell: Arc<dyn ShellProvider> = Arc::new(BrowserShellProvider {
                window: Arc::clone(&win),
            });
            let chrome_vp = self.chrome_viewport();
            let content_vp = self.content_viewport(self.tabs[self.active_tab].zoom);
            self.shell_provider = Some(Arc::clone(&shell));
            self.chrome_doc.set_viewport(chrome_vp);
            self.chrome_doc.set_shell_provider(Arc::clone(&shell));
            if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                doc.set_viewport(content_vp);
                doc.set_shell_provider(shell);
            }
            self.window = Some(win);
            if self.color_scheme != prev_scheme && self.tabs[self.active_tab].url.starts_with("rust://") {
                self.load_active_tab(false, None);
            } else {
                self.rebuild_chrome();
            }
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

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.process_app_events(event_loop);
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        if self.popup_owns(window_id) {
            self.handle_popup_window_event(event_loop, event);
        } else if self.window.as_ref().is_some_and(|w| w.id() == window_id) {
            self.handle_main_window_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        let now = Instant::now();
        if let Some(until) = self.status_note_until
            && now >= until
        {
            self.status_note = None;
            self.status_note_until = None;
            self.rebuild_chrome();
        }
        let next_wakeup = match (self.next_anim_frame, self.status_note_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(deadline) = next_wakeup {
            if now >= deadline {
                if self.next_anim_frame.is_some_and(|d| now >= d) {
                    self.next_anim_frame = None;
                    self.request_redraw();
                }
                if let Some(until) = self.status_note_until {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(until));
                } else {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options_round_trip_cli_args() {
        let opts = LaunchOptions {
            initial_url: Some("https://example.com/a?x=1".to_string()),
            new_window: true,
            js_mode: true,
            reader_mode: false,
            zoom: Some(1.3),
            position: Some((-200, 140)),
            size: Some((1100, 760)),
        };
        assert_eq!(LaunchOptions::from_args(opts.to_args()), opts);

        // Plain URL / local path stays compatible with single-instance forwarding.
        let plain = LaunchOptions::from_args(["https://rust-lang.org".to_string()]);
        assert!(!plain.new_window);
        assert_eq!(plain.initial_url.as_deref(), Some("https://rust-lang.org"));

        // `--` protects URLs/paths that begin with `-`.
        let dashed = LaunchOptions::from_args(["--new-window".into(), "--".into(), "-weird.html".into()]);
        assert!(dashed.new_window);
        assert_eq!(dashed.initial_url.as_deref(), Some("-weird.html"));
    }
}
