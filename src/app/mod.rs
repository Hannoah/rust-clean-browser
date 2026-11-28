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
mod sidebar;
mod suggest;

use crate::bookmarks::{BookmarkNode, BookmarkRootKind, BookmarkStore};
use crate::chrome_ui::{
    ChromeRenderState, DownloadItem, DownloadStatus, chrome_height, render_chrome_html, render_link_preview_html,
};
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
use keyboard_types::{Code, Key, Modifiers as KbtModifiers};
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
        /// Plain GET (not a form POST): safe to retry the same URL in the JS engine.
        retry_in_js: bool,
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
    /// A download started in WebView2 (recorded immediately in the Downloads UI).
    DownloadStarted {
        url: String,
        path: std::path::PathBuf,
    },
    /// A dangerous executable/script download was blocked by policy.
    DownloadBlocked {
        url: String,
        filename: String,
    },
    /// A download finished (succeeded or failed).
    DownloadDone {
        url: String,
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
    /// A JS-mode page started/stopped using camera, mic or audible media (`MEDIA_SCRIPT`).
    WebViewMedia {
        tab_id: u64,
        active: bool,
    },
    ChromeAction(String),
    SidebarAction(String),
    ChatPageContext {
        req_id: u64,
        prompt: String,
        page: Option<crate::chat::PageContext>,
    },
    /// Active JS tab had empty `innerText` and reported `application/pdf` (`contentType`/`<embed>`).
    ChatPdfDetected {
        req_id: u64,
        prompt: String,
        title: String,
        url: String,
    },
    AgentPdfSnapshot {
        req_id: u64,
        title: String,
        url: String,
        extracted: String,
    },
    ChatFinished {
        req_id: u64,
        provider_label: String,
        result: Result<String, String>,
    },
    /// Automatic (daily) or manual update check finished: `Ok(Some(_))` carries a
    /// newer release, `Ok(None)` means already current, `Err` is the failure.
    UpdateCheckDone {
        manual: bool,
        outcome: Result<Option<crate::update::UpdateInfo>, String>,
    },
    /// Setup-exe download finished: `Ok(path)` is ready to launch, `Err` failed.
    UpdateDownloadDone(Result<String, String>),
    /// API connection test finished for `provider`: live end-to-end result.
    ApiTestDone {
        provider: String,
        ok: bool,
        detail: String,
    },
    /// One streamed text delta from an in-flight API reply. The matching in-flight
    /// request appends it to a pending assistant message; the final `ChatFinished`
    /// replaces that draft with the authoritative full text.
    ChatStreamChunk {
        req_id: u64,
        provider_label: String,
        delta: String,
    },
    AgentStepReady {
        req_id: u64,
    },
    AgentLoadSettled {
        req_id: u64,
        wait_gen: u64,
    },
    AgentWaitCheck {
        req_id: u64,
        wait_gen: u64,
        found: bool,
    },
    AgentActionDone {
        req_id: u64,
        ok: bool,
        summary: String,
        need_delay_ms: u64,
    },
    AgentExtractDone {
        req_id: u64,
        ok: bool,
        summary: String,
        extracted: String,
        copy: bool,
    },
    AgyModelsLoaded(Vec<crate::chat::ModelOption>),
    /// Conversation summary for 대화 압축 (`Ok` = summary text).
    ChatCompactDone(Result<String, String>),
    /// What the agent is doing for request `req_id` (tool started/finished, reply text started).
    ChatProgress {
        req_id: u64,
        progress: crate::chat::Progress,
    },
    /// Once a second while `req_id` runs: refresh the "생각 중 (Ns)…" line.
    ChatLiveTick(u64),
    /// Model listing from the active API endpoint (`Err` = could not list; presets are shown).
    ApiModelsLoaded {
        provider: crate::chat::ChatProvider,
        result: Result<Vec<crate::chat::ModelOption>, String>,
    },
    /// Typing-effect frame for the newest assistant reply (`reveal_gen`).
    ChatRevealTick(u64),
    /// Guards `evaluate_script_with_callback` against silent WebView detachment/tab switches.
    ChatScriptTimeout {
        req_id: u64,
        script_seq: u64,
    },
    /// Re-render the chrome (end of a timed animation such as the progress bar's finish sweep).
    RefreshChrome,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiRegion {
    Chrome,
    Content,
    Sidebar,
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
    pointer_region: Option<UiRegion>,
    buttons: MouseEventButtons,
    keyboard_modifiers: Modifiers,
    /// Fractional zoom from Ctrl+wheel not yet applied (set_zoom snaps to 0.1).
    zoom_wheel_acc: f32,
    focus_region: UiRegion,
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
    /// Set when clicking an unfocused `#url-bar`, so a click without drag selects all text on release.
    url_bar_select_all_on_click: Option<PhysicalPosition<f64>>,
    /// Timestamp of the last click-to-select-all on `#url-bar` so an immediate 2nd click places
    /// the caret cleanly instead of triggering blitz-dom's 500ms double-click word selection.
    url_bar_just_selected_all: Option<Instant>,
    /// Recent downloads (newest first), shown in the toolbar Downloads popup.
    downloads: Vec<DownloadItem>,
    next_download_id: u64,
    /// Completed downloads since the user last opened the Downloads popup.
    unseen_downloads: usize,
    /// Requested initial position (physical px) when launched via tab tear-off (`--pos`).
    initial_position: Option<PhysicalPosition<i32>>,
    /// Requested initial surface size (logical px) when launched via tab tear-off (`--size`).
    initial_size: Option<LogicalSize<f64>>,

    sidebar_open: bool,
    sidebar_doc: Option<HtmlDocument>,
    last_sidebar_html: String,
    sidebar_drag: bool,
    /// Tracks a button `PointerDown` inside the sidebar so minor jitter / `:active` layout shifts
    /// cannot silently drop the click on `PointerUp`.
    sidebar_pressed_action: Option<(String, PhysicalPosition<f64>)>,
    sidebar_code_blocks: Vec<String>,
    sidebar_picker: crate::chrome_ui::SidebarPicker,
    agy_models: Vec<crate::chat::ModelOption>,
    agy_models_loading: bool,
    agy_models_fetched: bool,
    /// Last model listing per API provider + whether a fetch is running.
    api_models: std::collections::HashMap<crate::chat::ChatProvider, Result<Vec<crate::chat::ModelOption>, String>>,
    api_models_loading: bool,
    chat_config: crate::chat::ChatConfig,
    chat_cli_avail: crate::chat::CliAvailability,
    chat_messages: Vec<crate::chat::ChatMessage>,
    chat_show_settings: bool,
    chat_req_seq: u64,
    chat_inflight: Option<(u64, Arc<std::sync::atomic::AtomicBool>, tokio::task::AbortHandle)>,
    agent_run: Option<sidebar::AgentRun>,
    /// Saved conversations, loaded the first time the sidebar opens.
    chat_history: Option<crate::chat::ChatHistory>,
    /// History id of the conversation on screen (`None` until its first message is saved).
    chat_session_id: Option<u64>,
    /// 대화 압축 in flight: `(session id, message count)` when it started, so a summary
    /// never lands in a conversation that was cleared or switched meanwhile.
    chat_compacting: Option<(Option<u64>, usize)>,
    /// Status of the request in flight (live line + steps), see `sidebar::live`.
    chat_live: Option<sidebar::LiveActivity>,
    /// Replies whose "도구 N개 사용" card list is expanded (message indices).
    chat_activity_open: std::collections::HashSet<usize>,
    chat_show_history: bool,
    /// `(message index, chars shown)` while a reply is typing out.
    chat_reveal: Option<(usize, usize)>,
    chat_reveal_gen: u64,
    /// In-flight request whose API reply is streaming chunk by chunk into the last
    /// assistant message (`None` once `ChatFinished` replaces it with final text).
    chat_stream_req: Option<u64>,
    chat_script_seq: u64,
    sidebar_dirty: bool,
    sidebar_dirty_scroll: bool,
    /// Next sidebar rebuild ignores the settings inputs' drafts (a chip changed the value).
    sidebar_reset_drafts: bool,
    chat_input_lines: usize,
    /// Newer release found by the update check (toolbar pill offers one-click install).
    update_info: Option<crate::update::UpdateInfo>,
    /// An update check or installer download is running.
    update_busy: bool,
    /// Last API connection test result: `(provider id, ok, summary)`.
    api_conn: Option<(String, bool, String)>,
    /// An API connection test is running.
    api_test_busy: bool,

    // ── Motion bookkeeping (entrance animations survive HTML rebuilds via negative delays) ──
    /// Highest tab id already shown; ids above it are new tabs that get an entrance animation.
    seen_tab_id: Option<u64>,
    fresh_tabs: Vec<(u64, Instant)>,
    /// Tab id that was loading at the last chrome render, to catch the moment it finishes.
    loading_tab_seen: Option<u64>,
    progress_done_at: Option<Instant>,
    /// Placeholder painted while a Clean-mode page loads into an empty content area.
    skeleton_doc: Option<HtmlDocument>,
    /// Static skeleton painted under a WebView about to appear (see `paint_under_webview`).
    skeleton_static: Option<HtmlDocument>,
    /// Set only while `paint_under_webview` renders its one frame.
    prepaint_skeleton: bool,
    sidebar_opened_at: Option<Instant>,
    chat_seen_len: usize,
    chat_msg_added: Option<(usize, Instant)>,
    sidebar_panel_key: u8,
    sidebar_panel_at: Option<Instant>,
    /// Wheel-notch / keyboard scroll currently gliding (Clean-mode pages and the sidebar).
    smooth_scroll: Option<input::SmoothScroll>,
    /// Shared `(offset_x, offset_y, scale)` transform applied to IME candidate cursor updates.
    ime_transform: Arc<std::sync::Mutex<(f32, f32, f32)>>,
    cached_ram_mb: f64,
    cached_ram_at: Option<Instant>,
    chrome_motion_timer_armed: bool,
}

const FRESH_TAB_MS: u64 = 130;
const PROGRESS_DONE_MS: u64 = 220;

impl BrowserApp {
    pub fn new(
        winit_proxy: EventLoopProxy,
        launch: LaunchOptions,
        bookmarks: std::thread::JoinHandle<BookmarkStore>,
        ask_first_run: bool,
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

        let prefs = UiPrefs::load();
        let start_url = launch
            .initial_url
            // Local files (e.g. opened via the .html file association) become file:// URLs.
            .map(|u| arg_to_url(&u))
            .unwrap_or_else(|| {
                // First run with importable browsers around: ask on a welcome page
                // instead of silently copying anything.
                if ask_first_run && !prefs.onboarded && !crate::bookmarks::available_browser_sources().is_empty() {
                    "rust://welcome".to_string()
                } else {
                    "rust://home".to_string()
                }
            });
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

        let chat_cli_avail = crate::chat::CliAvailability::detect();
        let chat_config = crate::chat::ChatConfig::load_with_avail(chat_cli_avail);
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
            pointer_region: None,
            buttons: MouseEventButtons::None,
            keyboard_modifiers: Modifiers::default(),
            zoom_wheel_acc: 0.0,
            focus_region: UiRegion::Content,
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
            url_bar_just_selected_all: None,
            downloads: Vec::new(),
            next_download_id: 1,
            unseen_downloads: 0,
            initial_position: launch.position.map(|(x, y)| PhysicalPosition::new(x, y)),
            initial_size: launch.size.map(|(w, h)| LogicalSize::new(w as f64, h as f64)),
            sidebar_open: false,
            sidebar_doc: None,
            last_sidebar_html: String::new(),
            sidebar_drag: false,
            sidebar_pressed_action: None,
            sidebar_code_blocks: Vec::new(),
            sidebar_picker: crate::chrome_ui::SidebarPicker::None,
            agy_models: Vec::new(),
            agy_models_loading: false,
            agy_models_fetched: false,
            api_models: std::collections::HashMap::new(),
            api_models_loading: false,
            chat_config,
            chat_cli_avail,
            chat_messages: Vec::new(),
            chat_show_settings: false,
            chat_req_seq: 0,
            chat_inflight: None,
            agent_run: None,
            chat_history: None,
            chat_session_id: None,
            chat_compacting: None,
            chat_live: None,
            chat_activity_open: std::collections::HashSet::new(),
            chat_show_history: false,
            chat_reveal: None,
            chat_reveal_gen: 0,
            chat_stream_req: None,
            chat_script_seq: 0,
            sidebar_dirty: false,
            sidebar_dirty_scroll: false,
            sidebar_reset_drafts: false,
            chat_input_lines: 1,
            update_info: None,
            update_busy: false,
            api_conn: None,
            api_test_busy: false,
            seen_tab_id: None,
            fresh_tabs: Vec::new(),
            loading_tab_seen: None,
            progress_done_at: None,
            skeleton_doc: None,
            skeleton_static: None,
            prepaint_skeleton: false,
            sidebar_opened_at: None,
            chat_seen_len: 0,
            chat_msg_added: None,
            sidebar_panel_key: 0,
            sidebar_panel_at: None,
            smooth_scroll: None,
            ime_transform: Arc::new(std::sync::Mutex::new((0.0, 0.0, 1.0))),
            cached_ram_mb: 0.0,
            cached_ram_at: None,
            chrome_motion_timer_armed: false,
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
        self.skeleton_doc = None;
        self.skeleton_static = None;
        self.sync_viewports();
        self.rebuild_chrome();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(false);
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

    /// New tabs and the progress bar's finish sweep, as ages for `ChromeMotion`.
    fn chrome_motion(&mut self) -> crate::chrome_ui::ChromeMotion {
        let now = Instant::now();
        let age = |t: Instant| now.saturating_duration_since(t).as_millis() as u64;
        let max_id = self.tabs.iter().map(|t| t.id).max();
        if let (Some(seen), Some(max_id)) = (self.seen_tab_id, max_id) {
            for t in self.tabs.iter().filter(|t| t.id > seen) {
                self.fresh_tabs.push((t.id, now));
            }
            self.seen_tab_id = Some(seen.max(max_id));
        } else {
            // First render: every tab is new (launch / torn-off window).
            self.fresh_tabs = self.tabs.iter().map(|t| (t.id, now)).collect();
            self.seen_tab_id = max_id;
        }
        self.fresh_tabs.retain(|(_, t)| age(*t) < FRESH_TAB_MS);

        let active = &self.tabs[self.active_tab];
        if active.is_loading {
            self.loading_tab_seen = Some(active.id);
            self.progress_done_at = None;
        } else if self.loading_tab_seen.take() == Some(active.id) {
            self.progress_done_at = Some(now);
        }
        let progress_done_ms = self.progress_done_at.map(age).filter(|ms| *ms < PROGRESS_DONE_MS);
        if progress_done_ms.is_none() {
            self.progress_done_at = None;
        }
        let mut next_refresh_ms: Option<u64> = progress_done_ms.map(|ms| (PROGRESS_DONE_MS + 20).saturating_sub(ms));
        for &(_, t) in &self.fresh_tabs {
            let wait = (FRESH_TAB_MS + 20).saturating_sub(age(t));
            next_refresh_ms = Some(next_refresh_ms.map_or(wait, |cur| cur.min(wait)));
        }
        if let Some(wait_ms) = next_refresh_ms
            && !self.chrome_motion_timer_armed
        {
            self.chrome_motion_timer_armed = true;
            let bus = self.bus.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(wait_ms.max(16))).await;
                bus.send(AppEvent::RefreshChrome);
            });
        }
        crate::chrome_ui::ChromeMotion {
            fresh_tabs: self.fresh_tabs.iter().map(|(id, t)| (*id, age(*t))).collect(),
            progress_done_ms,
        }
    }

    fn content_phys_size(&self) -> (u32, u32) {
        let ch = self.chrome_phys_height();
        let sw = self.sidebar_phys_width();
        (
            self.win_size.width.saturating_sub(sw).max(1),
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
        let anim_time = self.anim_time();
        let chrome_vp = self.chrome_viewport();
        self.chrome_doc.set_viewport(chrome_vp);
        let content_vp = self.content_viewport(self.tabs[self.active_tab].zoom);
        if let Some(doc) = &mut self.tabs[self.active_tab].doc {
            doc.set_viewport(content_vp);
        }
        let sb_vp = self.sidebar_viewport();
        if let Some(sdoc) = &mut self.sidebar_doc {
            sdoc.set_viewport(sb_vp);
            sdoc.resolve(anim_time);
        }
        let sk_vp = self.content_viewport(1.0);
        for sk in [&mut self.skeleton_doc, &mut self.skeleton_static]
            .into_iter()
            .flatten()
        {
            sk.set_viewport(sk_vp.clone());
        }
        if let Some(wv) = &self.active_webview {
            let _ = wv.set_bounds(self.webview_bounds());
        }
        self.sync_ime_transform();
        self.request_redraw();
    }

    /// Physical top-left origin `(x, y)` where `sidebar_doc` is painted and hit-tested.
    pub(super) fn sidebar_origin_phys(&self) -> (u32, u32) {
        let win_w = self.win_size.width.max(1);
        let win_h = self.win_size.height.max(1);
        let chrome_h_phys = self.chrome_phys_height().min(win_h);
        let sidebar_w_phys = self.sidebar_phys_width().min(win_w.saturating_sub(1));
        let content_w_phys = win_w.saturating_sub(sidebar_w_phys).max(1);
        (content_w_phys, chrome_h_phys)
    }

    pub(super) fn cached_working_set_mb(&mut self) -> f64 {
        if let Some(at) = self.cached_ram_at
            && at.elapsed() < Duration::from_secs(2)
        {
            return self.cached_ram_mb;
        }
        self.cached_ram_mb = working_set_mb().round();
        self.cached_ram_at = Some(Instant::now());
        self.cached_ram_mb
    }

    pub(super) fn sync_ime_transform(&self) {
        let next = match self.focus_region {
            UiRegion::Chrome => (0.0, 0.0, 1.0),
            UiRegion::Content => (
                0.0,
                self.chrome_height() as f32,
                self.tabs.get(self.active_tab).map_or(1.0, |t| t.zoom.max(0.1)),
            ),
            UiRegion::Sidebar => (self.sidebar_left_logical(), self.chrome_height() as f32, 1.0),
        };
        *self.ime_transform.lock().unwrap_or_else(|e| e.into_inner()) = next;
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
        self.focus_region == UiRegion::Chrome
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
        // Keep whatever the user is typing (and focus / selection / scroll) when background events rebuild chrome.
        let typing = self.url_bar_focused().then(|| self.read_url_bar_text()).flatten();
        let saved_url_state = if typing.is_some() {
            self.read_url_bar_state()
        } else {
            None
        };
        let motion = self.chrome_motion();
        let working_set_mb = self.cached_working_set_mb();
        let js_webview_count = self.tabs.iter().filter(|t| t.js_mode).count();
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
        let downloads_in_progress = self
            .downloads
            .iter()
            .filter(|d| d.status == DownloadStatus::InProgress)
            .count();
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
            working_set_mb,
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
            downloads_in_progress,
            downloads_unseen: self.unseen_downloads,
            sidebar_open: self.sidebar_open,
            js_webview_count,
            update_version: self.update_info.as_ref().map(|i| i.version.as_str()),
            motion,
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
            self.restore_url_bar_focus_and_caret(&text, saved_url_state);
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
        cfg.base_url = Some(safe_blitz_base_url(base_url));
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
        self.rebuild_sidebar(false);
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
        // Stop any in-flight turn first so the worker lock frees, then kill the CLI processes.
        if let Some((_, cancel, abort)) = self.chat_inflight.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
            abort.abort();
        }
        crate::chat::shutdown_cli_workers();
        crate::single::cleanup();
        self.renderer.suspend();
        self.window = None;
        event_loop.exit();
    }

    fn focus_url_bar(&mut self) {
        self.focus_region = UiRegion::Chrome;
        self.sync_ime_transform();
        self.sync_suggest_query_to_url_bar();
        if let Some(doc) = &mut self.tabs[self.active_tab].doc {
            doc.clear_focus();
        }
        if let Some(sdoc) = &mut self.sidebar_doc {
            sdoc.clear_focus();
        }
        if let Some(win) = &self.window {
            helpers::focus_main_window(&**win, true);
            bus::enable_window_ime(&**win);
        }
        self.select_all_url_bar();
        self.request_redraw();
    }

    /// Snapshots the current trimmed `#url-bar` text into `suggest_query` so non-editing keys
    /// (ArrowRight, End, Ctrl, Shift) right after focusing don't pop open the suggestion list.
    pub(super) fn sync_suggest_query_to_url_bar(&mut self) {
        self.suggest_query = self
            .read_url_bar_text()
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
    }

    pub(super) fn select_all_url_bar(&mut self) {
        if let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") {
            self.chrome_doc.set_focus_to(url_id);
            let select_all = synthetic_key(Key::Character("a".to_string()), Code::KeyA, ACTION_MOD);
            self.dispatch_chrome_ui(UiEvent::KeyDown(select_all));
        }
    }

    /// Places `#url-bar`'s caret directly at physical pointer position `pos`, clamping left/right
    /// clicks on `.omni` padding/icons cleanly to the start/end of the URL.
    pub(super) fn place_url_bar_caret_at(&mut self, pos: PhysicalPosition<f64>) {
        let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") else {
            return;
        };
        let scale = self.chrome_doc.viewport().scale_f64();
        let logical_x = (pos.x / (self.hidpi_scale.max(1.0) as f64)) as f32;
        let logical_y = (pos.y / (self.hidpi_scale.max(1.0) as f64)) as f32;
        let Some((tx, ty, content_w, content_h)) = self.chrome_doc.get_node(url_id).and_then(|node| {
            let ti = node.element_data()?.text_input_data()?;
            let lay = ti.editor.try_layout()?;
            let abs = node.absolute_position(0.0, 0.0);
            let fl = node.final_layout();
            let content_x = abs.x + fl.padding.left + fl.border.left;
            let content_h = fl.content_box_height();
            let content_w = fl.content_box_width();
            let input_h = lay.height() / lay.scale();
            let y_offset = ((content_h - input_h) / 2.0).max(0.0);
            let content_y = abs.y + fl.padding.top + fl.border.top + y_offset;
            let tx = (logical_x - content_x) as f64 * scale + (ti.scroll_offset as f64) * scale;
            let ty = (logical_y - content_y) as f64 * scale;
            Some((tx as f32, ty as f32, content_w, content_h))
        }) else {
            return;
        };
        self.chrome_doc.set_focus_to(url_id);
        self.chrome_doc.with_text_input(url_id, |mut driver| {
            driver.move_to_point(tx, ty);
        });
        if let Some(input) = self
            .chrome_doc
            .get_node_mut(url_id)
            .and_then(|n| n.element_data_mut())
            .and_then(|e| e.text_input_data_mut())
        {
            input.clamp_scroll_offset(content_w, content_h);
        }
    }

    /// Trimmed text of a focused `<input id=...>` on a built-in `rust://` page, returning `None`
    /// while an IME preedit syllable is active so Enter confirms IME instead of submitting.
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
        if input.editor.is_composing() {
            return None;
        }
        Some(input.editor.raw_text().trim().to_string())
    }

    pub(super) fn toggle_maximize(&mut self) {
        if let Some(win) = &self.window {
            win.set_maximized(!win.is_maximized());
        }
    }

    /// Captures `(anchor_byte, focus_byte, scroll_offset)` of `#url-bar` before a chrome rebuild.
    fn read_url_bar_state(&self) -> Option<(usize, usize, f32)> {
        let url_id = self.chrome_doc.get_element_by_id("url-bar")?;
        let input = self.chrome_doc.get_node(url_id)?.element_data()?.text_input_data()?;
        let sel = input.editor.raw_selection();
        Some((sel.anchor().index(), sel.focus().index(), input.scroll_offset))
    }

    fn restore_url_bar_focus_and_caret(&mut self, text: &str, saved: Option<(usize, usize, f32)>) {
        let Some(url_id) = self.chrome_doc.get_element_by_id("url-bar") else {
            return;
        };
        self.chrome_doc.set_focus_to(url_id);
        let Some((anchor, focus, saved_scroll)) = saved else {
            self.chrome_doc.with_text_input(url_id, |mut driver| {
                driver.move_to_text_end();
            });
            return;
        };
        let len = text.len();
        let a = anchor.min(len);
        let f = focus.min(len);
        self.chrome_doc.with_text_input(url_id, |mut driver| {
            if a == 0 && f == len && !text.is_empty() {
                driver.select_all();
            } else if a == f && text.is_char_boundary(f) {
                driver.move_to_byte(f);
            } else if text.is_char_boundary(a) && text.is_char_boundary(f) {
                driver.select_byte_range(a, f);
            } else {
                driver.move_to_text_end();
            }
        });
        if let Some(node) = self.chrome_doc.get_node_mut(url_id) {
            let content_w = node.final_layout().content_box_width();
            let content_h = node.final_layout().content_box_height();
            if let Some(input) = node.element_data_mut().and_then(|e| e.text_input_data_mut()) {
                input.scroll_offset = saved_scroll;
                input.clamp_scroll_offset(content_w, content_h);
            }
        }
    }

    fn read_url_bar_text(&self) -> Option<String> {
        let url_id = self.chrome_doc.get_element_by_id("url-bar")?;
        let input = self.chrome_doc.get_node(url_id)?.element_data()?.text_input_data()?;
        Some(input.editor.raw_text().to_string())
    }

    /// True when `<input id=...>` inside `doc` has an active IME preedit composition.
    pub(super) fn is_input_composing(doc: &HtmlDocument, id: &str) -> bool {
        doc.get_element_by_id(id)
            .and_then(|nid| doc.get_node(nid))
            .and_then(|n| n.element_data())
            .and_then(|e| e.text_input_data())
            .is_some_and(|input| input.editor.is_composing())
    }

    /// True when `#url-bar` has an in-progress IME preedit syllable.
    pub(super) fn url_bar_composing(&self) -> bool {
        Self::is_input_composing(&self.chrome_doc, "url-bar")
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
        let gliding = self.step_smooth_scroll();
        let scale = self.hidpi_scale as f64;
        let win_w = self.win_size.width.max(1);
        let win_h = self.win_size.height.max(1);
        let (content_w_phys, chrome_h_phys) = self.sidebar_origin_phys();
        let content_h_phys = win_h.saturating_sub(chrome_h_phys);
        let sidebar_w_phys = win_w.saturating_sub(content_w_phys);

        self.chrome_doc.poll(None);
        self.chrome_doc.resolve(anim_time);

        let mut needs_more_frames = self.chrome_doc.is_animating() || gliding;
        // Empty content area while a Clean-mode page loads: show the page skeleton. Under a WebView
        // about to appear, paint the static one (`paint_under_webview`).
        let show_skeleton = self.prepaint_skeleton
            || (self.active_webview.is_none()
                && self.tabs[self.active_tab].doc.is_none()
                && self.tabs[self.active_tab].is_loading);
        let animated = !self.prepaint_skeleton;
        let slot_empty = if animated {
            self.skeleton_doc.is_none()
        } else {
            self.skeleton_static.is_none()
        };
        if show_skeleton && slot_empty {
            let mut sk = HtmlDocument::from_html(
                &crate::chrome_ui::render_page_skeleton_html(animated),
                self.doc_config(self.content_viewport(1.0), self.shell_provider.clone()),
            );
            sk.resolve(anim_time);
            if animated {
                self.skeleton_doc = Some(sk);
            } else {
                self.skeleton_static = Some(sk);
            }
        }
        let skeleton_slot = if animated {
            &mut self.skeleton_doc
        } else {
            &mut self.skeleton_static
        };
        let skeleton = if show_skeleton {
            skeleton_slot.as_mut().map(|sk| {
                sk.resolve(anim_time);
                needs_more_frames |= animated && sk.is_animating();
                &mut **sk
            })
        } else {
            None
        };
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
        let sidebar = if self.sidebar_open {
            self.sidebar_doc.as_mut().map(|sdoc| {
                needs_more_frames |= sdoc.poll(None);
                sdoc.resolve(anim_time);
                needs_more_frames |= sdoc.is_animating();
                &mut **sdoc
            })
        } else {
            None
        };
        let chrome_inner = &mut *self.chrome_doc;
        let preview = if self.active_webview.is_none() {
            self.link_preview_doc.as_mut().map(|(doc, w_logical)| {
                doc.resolve(anim_time);
                let pw = (((*w_logical as f32) * self.hidpi_scale).round().max(1.0) as u32).min(content_w_phys);
                let ph = ((36.0 * self.hidpi_scale).round().max(1.0) as u32).min(win_h);
                let px = ((6.0 * self.hidpi_scale).round() as u32).min(content_w_phys.saturating_sub(pw));
                let py = win_h.saturating_sub(ph + (6.0 * self.hidpi_scale).round() as u32);
                (&mut **doc, pw, ph, px, py)
            })
        } else {
            None
        };

        self.renderer.render(|scene| {
            if let Some(sk) = skeleton
                && content_h_phys > 0
            {
                let clip = Rect::new(0.0, chrome_h_phys as f64, content_w_phys as f64, win_h as f64);
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
                paint_scene(scene, sk, scale, content_w_phys, content_h_phys, 0, chrome_h_phys);
                scene.pop_layer();
            }
            if let Some((content_doc, content_scale)) = content
                && content_h_phys > 0
            {
                let clip = Rect::new(0.0, chrome_h_phys as f64, content_w_phys as f64, win_h as f64);
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
                paint_scene(
                    scene,
                    content_doc,
                    content_scale,
                    content_w_phys,
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
            if let Some(sdoc) = sidebar
                && sidebar_w_phys > 0
                && content_h_phys > 0
            {
                let sclip = Rect::new(content_w_phys as f64, chrome_h_phys as f64, win_w as f64, win_h as f64);
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &sclip);
                paint_scene(
                    scene,
                    sdoc,
                    scale,
                    sidebar_w_phys,
                    content_h_phys,
                    content_w_phys,
                    chrome_h_phys,
                );
                scene.pop_layer();
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
                ime_transform: Arc::clone(&self.ime_transform),
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

        // Automatic update check (throttled to once a day) so a new release
        // surfaces itself instead of users hunting for it.
        self.maybe_check_for_updates(false);
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
        if self.next_anim_frame.is_some_and(|d| now >= d) {
            self.next_anim_frame = None;
            self.request_redraw();
        }
        if let Some(popup) = &mut self.popup
            && popup.next_anim_frame.is_some_and(|d| now >= d)
        {
            popup.next_anim_frame = None;
            popup.window.request_redraw();
        }
        let next_wakeup = [
            self.next_anim_frame,
            self.status_note_until,
            self.popup.as_ref().and_then(|p| p.next_anim_frame),
        ]
        .into_iter()
        .flatten()
        .min();
        if let Some(deadline) = next_wakeup {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

/// Ensures `blitz-dom`'s `DocumentConfig::base_url` parses cleanly as a hierarchical URL;
/// a `cannot_be_a_base` URL (like `data:...`) or unparseable string makes `DocumentUrl::from_str`
/// fall back to `data:text/css;charset=utf-8;base64,` and panic on relative `<link>`/`<img>` URLs.
fn safe_blitz_base_url(base_url: &str) -> String {
    if Url::parse(base_url).is_ok_and(|u| !u.cannot_be_a_base()) {
        base_url.to_string()
    } else {
        "https://invalid.local/".to_string()
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

    #[test]
    fn url_bar_direct_selection_and_ime_composing_work() {
        let html = r#"<!DOCTYPE html><html><body><input id="url-bar" type="text" value="https://sas-demos.engage.sas.com/SASLanding/" style="width:400px;height:30px;border:none;padding:0;" /></body></html>"#;
        let mut doc = HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(Viewport::new(800, 80, 1.0, ColorScheme::Light)),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        let id = doc.get_element_by_id("url-bar").expect("url-bar");
        doc.set_focus_to(id);
        assert!(!BrowserApp::is_input_composing(&doc, "url-bar"));

        // Direct byte-range restoration preserves partial selection accurately (no 256-char cap).
        doc.with_text_input(id, |mut driver| {
            driver.select_byte_range(8, 17);
        });
        let range = doc
            .get_node(id)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap()
            .editor
            .raw_selection()
            .text_range();
        assert_eq!(range, 8..17);

        // Moving past text end lands cleanly at text length.
        doc.with_text_input(id, |mut driver| {
            driver.move_to_point(9999.0, 10.0);
        });
        let end_range = doc
            .get_node(id)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap()
            .editor
            .raw_selection()
            .text_range();
        assert_eq!(end_range, 44..44);
    }

    #[test]
    fn safe_base_url_prevents_blitz_relative_stylesheet_panic() {
        assert_eq!(safe_blitz_base_url("https://example.com/a"), "https://example.com/a");
        assert_eq!(safe_blitz_base_url("rust://home"), "rust://home");
        assert_eq!(safe_blitz_base_url("<!DOCTYPE html>"), "https://invalid.local/");
        assert_eq!(safe_blitz_base_url("data:text/html,hi"), "https://invalid.local/");

        let html = r#"<!DOCTYPE html><html><head><link rel="stylesheet" href="/a/b.css"></head><body>hi</body></html>"#;
        let doc = HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
                base_url: Some(safe_blitz_base_url("<!DOCTYPE html>")),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );
        assert!(doc.base_url().as_str().starts_with("https://invalid.local/"));
    }
}
