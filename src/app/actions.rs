//! `data-action` clicks from the browser UI and the async `AppEvent` queue.

use super::helpers::{arg_to_url, focus_main_window, primary_modifier, requires_js_engine};
use super::{AppEvent, BrowserApp};
use crate::bookmarks::BookmarkRootKind;
use crate::chrome_ui::{DownloadItem, DownloadStatus};
use crate::favicon;
use blitz_dom::Document;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;
use winit::event_loop::ActiveEventLoop;

impl BrowserApp {
    /// Translate the page (Korean <-> English, direction auto-detected) or restore the original.
    pub(super) fn toggle_translate(&mut self) {
        self.ensure_active_valid();
        let idx = self.active_tab;
        let tab_id = self.tabs[idx].id;
        let tab = &mut self.tabs[idx];
        if tab.translated.take().is_some() {
            if let Some(raw) = tab.raw_html.clone() {
                let url = tab.url.clone();
                let scroll = self.current_scroll_y();
                self.apply_html_to_tab(idx, url, raw, Some(scroll));
            }
            self.set_status_note("원문으로 복원됨");
            return self.rebuild_chrome();
        }
        let Some(raw) = tab
            .raw_html
            .clone()
            .filter(|_| !tab.js_mode && !tab.url.starts_with("rust://") && !tab.url.starts_with("file://"))
        else {
            self.set_status_note("번역은 Clean 모드의 http(s) 웹 페이지에서만 가능 (JS/로컬 파일 제외)");
            return self.rebuild_chrome();
        };
        let segs = crate::translate::segments(&raw);
        if segs.is_empty() {
            self.set_status_note("번역할 텍스트가 없음");
            return self.rebuild_chrome();
        }
        let tl = crate::translate::target_lang(&segs);
        self.set_status_note(if tl == "en" {
            "영어로 번역 중…"
        } else {
            "한국어로 번역 중…"
        });
        self.rebuild_chrome();
        let bus = self.bus.clone();
        let texts = segs.iter().map(|s| s.text.clone()).collect();
        tokio::spawn(async move {
            let result = crate::translate::translate(texts, tl)
                .await
                .map(|tr| Arc::<str>::from(crate::translate::apply(&raw, &segs, &tr)));
            bus.send(AppEvent::Translated {
                tab_id,
                source: raw,
                tl,
                result,
            });
        });
    }

    /// Checks GitHub Releases for a newer build (`manual` toasts every outcome,
    /// automatic checks only announce an actual update). Throttled to once a day.
    pub(super) fn maybe_check_for_updates(&mut self, manual: bool) {
        if self.update_busy || self.update_info.is_some() {
            return;
        }
        let now = crate::update::now_secs();
        if !manual && !crate::update::should_check(self.prefs.update_last_check, now) {
            return;
        }
        self.update_busy = true;
        if manual {
            self.set_status_note("업데이트 확인 중…");
            self.rebuild_chrome();
        }
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let outcome = crate::update::fetch_latest(env!("CARGO_PKG_VERSION")).await;
            bus.send(AppEvent::UpdateCheckDone { manual, outcome });
        });
    }

    pub(super) fn on_update_check_done(
        &mut self,
        manual: bool,
        outcome: Result<Option<crate::update::UpdateInfo>, String>,
    ) {
        self.update_busy = false;
        self.prefs.update_last_check = crate::update::now_secs();
        self.prefs.save();
        match outcome {
            Ok(Some(info)) => {
                if self.prefs.update_skipped == info.version {
                    return;
                }
                self.update_info = Some(info.clone());
                self.set_status_note(&format!("새 버전 {} 나옴 · 상단 업데이트 버튼으로 설치", info.version));
                self.rebuild_chrome();
            }
            Ok(None) => {
                if manual {
                    self.set_status_note("최신 버전입니다");
                    self.rebuild_chrome();
                }
            }
            Err(e) => {
                if manual {
                    let short: String = e.chars().take(120).collect();
                    self.set_status_note(&short);
                    self.rebuild_chrome();
                }
            }
        }
    }

    /// Downloads the pending Setup exe, then hands it to the OS installer.
    pub(super) fn apply_update(&mut self) {
        let Some(info) = self.update_info.clone() else {
            self.set_status_note("설치할 업데이트가 없습니다");
            return self.rebuild_chrome();
        };
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        self.set_status_note("업데이트 다운로드 중…");
        self.rebuild_chrome();
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let result = match crate::update::download_setup(&info).await {
                Ok(path) => Ok(path.to_string_lossy().into_owned()),
                Err(e) => Err(e),
            };
            bus.send(AppEvent::UpdateDownloadDone(result));
        });
    }

    pub(super) fn on_update_download_done(&mut self, result: Result<String, String>) {
        self.update_busy = false;
        match result {
            Ok(path) => match crate::update::launch_installer(std::path::Path::new(&path)) {
                Ok(()) => self.set_status_note("설치 프로그램을 실행했습니다"),
                Err(e) => self.set_status_note(&e),
            },
            Err(e) => self.set_status_note(&e),
        }
        self.rebuild_chrome();
    }

    /// Runs a minimal 1-token request against the active API provider to prove the
    /// endpoint, key, and model work end to end. The result shows in the settings
    /// card; it never touches CLI binaries.
    pub(super) fn test_api_connection(&mut self) {
        if self.chat_config.provider.is_cli() {
            return;
        }
        if self.api_test_busy {
            return;
        }
        self.api_test_busy = true;
        self.set_status_note("연결 테스트 중…");
        self.rebuild_chrome();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar_now(false);
        let config = self.chat_config.clone();
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let flag = std::sync::atomic::AtomicBool::new(false);
            let result = crate::chat::test_api_connection(&config, &flag).await;
            let (ok, detail) = match result {
                Ok(summary) => (true, summary),
                Err(e) => (false, e),
            };
            bus.send(AppEvent::ApiTestDone {
                provider: config.provider.id().to_string(),
                ok,
                detail,
            });
        });
    }

    pub(super) fn on_api_test_done(&mut self, provider: String, ok: bool, detail: String) {
        self.api_test_busy = false;
        self.api_conn = Some((provider, ok, detail.clone()));
        let short: String = detail.chars().take(120).collect();
        self.set_status_note(&short);
        self.rebuild_chrome();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar_now(false);
    }

    /// Hides the pending update until its version is superseded by a newer one.
    pub(super) fn skip_update(&mut self) {
        if let Some(info) = self.update_info.take() {
            self.prefs.update_skipped = info.version;
            self.prefs.save();
            self.set_status_note("이 버전 건너뛰기");
        }
        self.rebuild_chrome();
    }

    /// Copy the active page's citation (`ieee`/`apa`/`slide`/`mla`) to the clipboard.
    fn copy_citation(&mut self, style: &str) {
        self.ensure_active_valid();
        let tab = &self.tabs[self.active_tab];
        let msg = match crate::cite::citation(style, &tab.title, &tab.url, tab.raw_html.as_deref()) {
            Some(text) => match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
                Ok(()) => "출처가 클립보드에 복사됨",
                Err(_) => "클립보드 접근 실패",
            },
            None => "웹 페이지에서만 출처를 만들 수 있음",
        };
        self.set_status_note(msg);
        self.rebuild_chrome();
    }

    pub(super) fn handle_chrome_action(&mut self, action: &str, event_loop: &dyn ActiveEventLoop) {
        if let Some(rest) = action.strip_prefix("tab:") {
            if let Ok(idx) = rest.parse() {
                self.switch_tab(idx);
            }
        } else if let Some(rest) = action.strip_prefix("close-tab:") {
            if let Ok(idx) = rest.parse() {
                self.close_tab(idx, Some(event_loop));
            }
        } else if let Some(style) = action.strip_prefix("cite:") {
            self.copy_citation(style);
        } else if let Some(url) = action.strip_prefix("nav-bg:") {
            self.open_background_tab(url);
        } else if let Some(url) = action.strip_prefix("nav:") {
            self.navigate_current(url);
        } else if let Some(rest) = action.strip_prefix("bm-popup:bar:") {
            if let Ok(idx) = rest.parse::<usize>() {
                let title = self
                    .bookmarks
                    .resolve_folder(BookmarkRootKind::Bar, &[idx])
                    .map_or_else(|| "Folder".to_string(), |(t, _)| t);
                self.queue_popup(action, BookmarkRootKind::Bar, vec![idx], 0, title);
            }
        } else if let Some(rest) = action.strip_prefix("bm-popup:overflow:") {
            if let Ok(offset) = rest.parse() {
                self.queue_popup(
                    action,
                    BookmarkRootKind::Bar,
                    Vec::new(),
                    offset,
                    "북마크 더보기".into(),
                );
            }
        } else {
            match action {
                "new-tab" => {
                    self.open_new_tab("rust://home");
                    self.focus_url_bar();
                }
                "new-window" => self.open_new_window(None),
                "back" => self.history_step(false),
                "forward" => self.history_step(true),
                "reload" => self.reload_active_tab(),
                "toggle-bookmark" => {
                    self.ensure_active_valid();
                    let tab = &self.tabs[self.active_tab];
                    let added = self.bookmarks.toggle_custom(&tab.title, &tab.url);
                    self.set_status_note(if added {
                        "북마크 추가됨"
                    } else {
                        "북마크 해제됨"
                    });
                    self.rebuild_chrome();
                }
                "translate" => self.toggle_translate(),
                "apply-update" => self.apply_update(),
                "check-updates" => self.maybe_check_for_updates(true),
                "skip-update" => self.skip_update(),
                "chat-test-connection" => self.test_api_connection(),
                "toggle-js" => self.toggle_js_mode(),
                "toggle-reader" => self.toggle_reader_mode(),
                "toggle-filter" => {
                    self.net_provider.toggle_filter();
                    let on = self.net_provider.is_filter_enabled();
                    self.set_status_note(if on {
                        "트래커 차단 켜짐"
                    } else {
                        "트래커 차단 꺼짐"
                    });
                    self.rebuild_chrome();
                }
                "toggle-https-only" => {
                    self.prefs.https_only = !self.prefs.https_only;
                    self.prefs.save();
                    let on = self.prefs.https_only;
                    self.set_status_note(if on { "HTTPS-Only 켜짐" } else { "HTTPS-Only 꺼짐" });
                    self.rebuild_chrome();
                }
                "open-palette" => self.open_command_palette(event_loop),
                "open-find" => self.open_find_bar(event_loop),
                "open-downloads" => self.toggle_downloads_popup(event_loop),
                "cycle-theme" => self.cycle_theme(),
                "toggle-bm-bar" => self.toggle_bookmarks_bar(),
                "toggle-sidebar" => self.toggle_sidebar(),
                "chat-summarize-page" => self.open_sidebar_and_summarize(),
                "trim-memory" => {
                    crate::memory::trim_process_working_set();
                    self.cached_ram_at = None;
                    let mb = self.cached_working_set_mb();
                    #[cfg(any(target_os = "windows", target_os = "macos"))]
                    let msg = format!("메모리 정리됨 · {mb:.0} MB");
                    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                    let msg = format!("메모리 측정됨 · {mb:.0} MB (Linux 자동 회수)");
                    self.set_status_note(&msg);
                    self.rebuild_chrome();
                }
                "dismiss-toast" => {
                    self.status_note = None;
                    self.status_note_until = None;
                    self.rebuild_chrome();
                }
                "reopen-tab" => self.reopen_closed_tab(),
                "close-current-tab" => {
                    self.ensure_active_valid();
                    self.close_tab(self.active_tab, Some(event_loop))
                }
                "sync-chrome" => {
                    let msg = match self.bookmarks.import_from_chrome() {
                        Ok(n) => format!("Chrome 북마크 {n}개 동기화"),
                        Err(e) => e,
                    };
                    self.set_status_note(&msg);
                    self.ensure_active_valid();
                    let url = &self.tabs[self.active_tab].url;
                    if url.starts_with("rust://bookmarks") || url == "rust://home" {
                        self.load_active_tab(false, None);
                    } else {
                        self.rebuild_chrome();
                    }
                }
                "bm-popup:other" => {
                    self.queue_popup(action, BookmarkRootKind::Other, Vec::new(), 0, "기타 북마크".into());
                }
                "win-min" => {
                    if let Some(win) = &self.window {
                        win.set_minimized(true);
                    }
                }
                "win-max" => self.toggle_maximize(),
                "win-close" => self.exit_app(event_loop),
                "zoom-reset" => self.set_zoom(1.0),
                _ => {}
            }
        }
    }

    /// Popups need the `ActiveEventLoop` to create their window, so they open via the queue.
    /// Records a download at the top of the Downloads list (newest first, at most 50 kept).
    fn push_download(
        &mut self,
        url: String,
        filename: String,
        path: Option<std::path::PathBuf>,
        status: DownloadStatus,
        size_bytes: Option<u64>,
    ) {
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let id = self.next_download_id;
        self.next_download_id += 1;
        let item = DownloadItem {
            id,
            url,
            filename,
            path,
            status,
            size_bytes,
            started_at,
        };
        self.downloads.insert(0, item);
        self.downloads.truncate(50);
    }

    fn queue_popup(&self, key: &str, root: BookmarkRootKind, path: Vec<usize>, index_offset: usize, title: String) {
        self.bus.send(AppEvent::OpenBookmarkPopup {
            key: key.to_string(),
            root,
            path,
            index_offset,
            title,
        });
    }

    pub(super) fn process_app_events(&mut self, event_loop: &dyn ActiveEventLoop) {
        let mut favicons_changed = false;
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Favicon(next) => {
                    self.favicon_inflight = false;
                    self.favicon_abort = None;
                    match next {
                        favicon::Next::Done { changed } => favicons_changed |= changed,
                        favicon::Next::Then(job) => self.favicon_queue.push_front(job),
                    }
                }
                AppEvent::WebViewIcons { page_url, links } => {
                    self.request_favicon_known(&page_url, &links);
                }
                AppEvent::RedrawDoc { doc_id } => {
                    self.ensure_active_valid();
                    if let Some(doc) = &mut self.tabs[self.active_tab].doc
                        && doc.id() == doc_id
                    {
                        doc.poll(None);
                        self.request_redraw();
                    }
                    if self.chrome_doc.id() == doc_id {
                        self.chrome_doc.poll(None);
                        self.request_redraw();
                    }
                    if let Some(sdoc) = &mut self.sidebar_doc
                        && sdoc.id() == doc_id
                    {
                        sdoc.poll(None);
                        self.request_redraw();
                    }
                    if let Some(popup) = &mut self.popup
                        && popup.doc.id() == doc_id
                    {
                        popup.doc.poll(None);
                        popup.window.request_redraw();
                    }
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
                    if cacheable && !is_spa_shell && !requires_js_engine(&final_url) {
                        let req_key = requested_url.split('#').next().unwrap_or(&requested_url);
                        let final_key = final_url.split('#').next().unwrap_or(&final_url);
                        self.page_cache.put(req_key, &final_url, &clean_html);
                        if final_key != req_key {
                            self.page_cache.put(final_key, &final_url, &clean_html);
                        }
                    }
                    #[cfg(debug_assertions)]
                    eprintln!("[event] PageLoaded tab={tab_id} seq={load_seq} spa={is_spa_shell} url={final_url}");
                    let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
                        continue;
                    };
                    if self.tabs[idx].load_seq != load_seq {
                        continue;
                    }
                    if (is_spa_shell || requires_js_engine(&final_url))
                        && !self.tabs[idx].force_blitz
                        && !self.tabs[idx].reader_mode
                    {
                        self.tabs[idx].url = auto_js_target_url(requested_url, final_url, cacheable);
                        self.tabs[idx].is_loading = false;
                        self.set_status_note("Auto JS Mode (SPA)");
                        self.mount_or_navigate_webview(idx);
                        self.sync_find_bar_after_doc_change();
                        self.rebuild_chrome();
                    } else if self.tabs[idx].revalidating {
                        // Background revalidation of a disk-cached page: only swap in
                        // the fresh copy if it changed, keeping the reading position.
                        self.tabs[idx].revalidating = false;
                        self.tabs[idx].is_loading = false;
                        if self.tabs[idx].raw_html.as_deref() == Some(&*clean_html) {
                            self.rebuild_chrome();
                        } else {
                            let scroll = self.tabs[idx].doc.as_ref().map(|d| d.viewport_scroll().y);
                            let shown = match self.tabs[idx].url.split_once('#') {
                                Some((_, f)) if !final_url.contains('#') => format!("{final_url}#{f}"),
                                _ => final_url,
                            };
                            self.apply_html_to_tab(idx, shown, clean_html, scroll);
                        }
                    } else {
                        self.apply_html_to_tab(idx, final_url, clean_html, restore_scroll_y);
                    }
                    if !self.tabs[idx].js_mode {
                        self.history.record(&self.tabs[idx].url, &self.tabs[idx].title);
                    }
                    // Persist any login cookies set during this navigation.
                    self.net_provider.cookies.save();
                }
                AppEvent::PageLoadFailed {
                    retry_in_js,
                    tab_id,
                    load_seq,
                    url,
                    error,
                } => {
                    #[cfg(debug_assertions)]
                    eprintln!("[event] PageLoadFailed tab={tab_id} url={url} err={error}");
                    let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
                        continue;
                    };
                    if self.tabs[idx].load_seq != load_seq {
                        continue;
                    }
                    // PDF / image / media without a telltale extension: let WebView2's viewers render it.
                    let is_pdf_resp = error == crate::cleaner::PDF_RESPONSE_ERR;
                    if (is_pdf_resp || error.starts_with(crate::cleaner::NOT_HTML_ERR))
                        && !self.tabs[idx].js_mode
                        && !self.tabs[idx].force_blitz
                        && (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://"))
                    {
                        if is_pdf_resp {
                            self.tabs[idx].pdf_hint = Some(url.clone());
                        }
                        self.tabs[idx].url = url;
                        self.tabs[idx].is_loading = false;
                        self.set_status_note("PDF 필기 모드로 열기");
                        self.mount_or_navigate_webview(idx);
                        self.rebuild_chrome();
                        continue;
                    }
                    if self.tabs[idx].revalidating {
                        // Offline / server error: keep showing the cached copy.
                        self.tabs[idx].revalidating = false;
                        self.tabs[idx].is_loading = false;
                        self.set_status_note("오프라인 캐시 표시 중");
                    } else if retry_in_js
                        && !self.tabs[idx].js_mode
                        && !self.tabs[idx].force_blitz
                        && (url.starts_with("http://") || url.starts_with("https://"))
                    {
                        self.tabs[idx].url = url;
                        self.tabs[idx].is_loading = false;
                        self.set_status_note("JS 모드로 자동 전환");
                        self.mount_or_navigate_webview(idx);
                        self.sync_find_bar_after_doc_change();
                    } else {
                        self.show_error(idx, &url, &error, Some("Load Error"));
                    }
                    self.rebuild_chrome();
                }
                AppEvent::Navigate {
                    source_doc_id,
                    url,
                    request,
                } => {
                    self.ensure_active_valid();
                    let from_active = self.tabs[self.active_tab]
                        .doc
                        .as_ref()
                        .is_some_and(|d| d.id() == source_doc_id);
                    if from_active {
                        let url_str = url.as_str();
                        // Page-initiated `file://` navigation is restricted to our app-data dir
                        // or files inside/under the current `file://` page's directory.
                        if url.scheme() == "file" && !is_page_file_nav_allowed(&self.tabs[self.active_tab].url, &url) {
                            self.set_status_note("차단됨: 페이지의 로컬 파일 접근");
                            self.rebuild_chrome();
                            continue;
                        }
                        if url.scheme() == "rust" {
                            let tab = &self.tabs[self.active_tab];
                            if !is_page_rust_nav_allowed(&tab.url, tab.is_error_page, tab.reader_mode, url_str) {
                                self.set_status_note("차단됨: 페이지의 내부 주소 접근");
                                self.rebuild_chrome();
                                continue;
                            }
                        }
                        let is_builtin_action = url_str.starts_with("rust://reader-opt")
                            || url_str.starts_with("rust://open-js")
                            || (url_str.starts_with("rust://bookmarks?")
                                && (crate::chrome_ui::parse_query_param(url_str, "del").is_some()
                                    || crate::chrome_ui::parse_query_param(url_str, "sync").as_deref() == Some("1")))
                            || (url_str.starts_with("rust://history?")
                                && (crate::chrome_ui::parse_query_param(url_str, "del").is_some()
                                    || crate::chrome_ui::parse_query_param(url_str, "clear").as_deref() == Some("1")));
                        // Ctrl+click opens a sleeping background tab (zero DOM memory until visited).
                        if request.is_none() && !is_builtin_action && primary_modifier(self.keyboard_modifiers.state())
                        {
                            self.open_background_tab(url_str);
                        } else {
                            self.navigate_with_request(url_str, request);
                        }
                    }
                }
                AppEvent::Translated {
                    tab_id,
                    source,
                    tl,
                    result,
                } => {
                    let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
                        continue;
                    };
                    // Ignore if the tab navigated away (or unloaded) while translating.
                    if !self.tabs[idx]
                        .raw_html
                        .as_ref()
                        .is_some_and(|r| Arc::ptr_eq(r, &source))
                    {
                        continue;
                    }
                    match result {
                        Ok(html) => {
                            let url = self.tabs[idx].url.clone();
                            let scroll = self.current_scroll_y();
                            self.tabs[idx].translated = Some(Arc::clone(&html));
                            self.apply_html_to_tab(idx, url, html, Some(scroll));
                            let to = if tl == "en" { "영어" } else { "한국어" };
                            self.set_status_note(&format!("{to}로 번역됨 · Alt+T로 원문 보기"));
                        }
                        Err(e) => self.set_status_note(&e),
                    }
                    self.rebuild_chrome();
                }
                AppEvent::OpenUrl(arg) => {
                    self.open_new_tab(&arg_to_url(&arg));
                    if let Some(win) = &self.window {
                        win.set_minimized(false);
                        focus_main_window(&**win, true);
                    }
                }
                AppEvent::WebViewMedia { tab_id, active } => {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id)
                        && self.tabs[idx].media_active != active
                    {
                        self.tabs[idx].media_active = active;
                        // A parked tab whose call just ended can now be frozen like any other.
                        if !active && let Some((_, wv)) = self.parked_webviews.iter().find(|(id, _)| *id == tab_id) {
                            super::helpers::suspend_webview(wv);
                        }
                        self.rebuild_chrome();
                    }
                }
                AppEvent::WebViewTitleChanged { tab_id, title } => {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id)
                        && !title.trim().is_empty()
                        && self.tabs[idx].title != title
                    {
                        self.history.set_title(&self.tabs[idx].url, &title);
                        self.tabs[idx].title = title;
                        if idx == self.active_tab {
                            self.query_webview_icons();
                            self.rebuild_sidebar(false);
                        }
                        self.rebuild_chrome();
                    }
                }
                AppEvent::WebViewPageLoad { tab_id, url, finished } => {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        self.tabs[idx].is_loading = !finished;
                        // The annotator's own viewer URL must not replace the PDF's real URL.
                        if !url.is_empty()
                            && url != "about:blank"
                            && self.tabs[idx].url != url
                            && !self.tabs[idx].editing_pdf()
                        {
                            self.tabs[idx].url = url;
                            if !finished {
                                self.tabs[idx].title = crate::tabs::short_title_for_url(&self.tabs[idx].url);
                            }
                        }
                        if finished {
                            self.history.record(&self.tabs[idx].url, &self.tabs[idx].title);
                            if idx == self.active_tab {
                                self.query_webview_icons();
                                self.rebuild_sidebar(false);
                            }
                            self.on_agent_webview_loaded(tab_id);
                        }
                        self.rebuild_chrome();
                    }
                }
                // A denied `about:blank` popup has nothing to show; untrusted JS pages must never
                // open `rust://` or `file://` tabs via `window.open` / `target="_blank"`.
                AppEvent::OpenInNewTab(url) if url == "about:blank" => {}
                AppEvent::OpenInNewTab(url) => {
                    if super::helpers::is_web_http_url(&url) {
                        self.open_new_tab_inheriting_js(&url);
                    } else {
                        self.set_status_note("차단됨: 허용되지 않은 새 탭 주소");
                        self.rebuild_chrome();
                    }
                }
                AppEvent::ZoomBy(notches) => self.zoom_by_notches(notches),
                AppEvent::ZoomWheelPx(dy) => {
                    let zoom = self.tabs[self.active_tab].zoom;
                    self.zoom_by_notches(-dy * zoom / 100.0);
                }
                AppEvent::ZoomReset => self.set_zoom(1.0),
                AppEvent::DownloadStarted { url, path } => {
                    let filename = super::helpers::download_display_name(Some(path.as_path()), &url);
                    self.push_download(url, filename.clone(), Some(path), DownloadStatus::InProgress, None);
                    self.set_status_note(&format!("다운로드 중: {filename}"));
                    self.rebuild_chrome();
                    self.open_downloads_popup(event_loop, false);
                }
                AppEvent::DownloadDone { url, path, success } => {
                    // `wry` attaches WebView2's `StateChanged` listener before invoking
                    // `download_started_handler`; cancelling a blocked download therefore fires
                    // a spurious `DownloadDone { success: false }` right after `DownloadBlocked`.
                    if !success
                        && self
                            .downloads
                            .iter()
                            .take(4)
                            .any(|d| d.status == DownloadStatus::Blocked && d.url == url)
                    {
                        continue;
                    }
                    let existing = self.downloads.iter_mut().find(|d| {
                        d.status == DownloadStatus::InProgress && ((path.is_some() && d.path == path) || d.url == url)
                    });
                    let filename = if let Some(item) = existing {
                        if let Some(p) = path {
                            item.filename = super::helpers::download_display_name(Some(p.as_path()), &item.url);
                            item.path = Some(p);
                        }
                        item.status = if success {
                            DownloadStatus::Completed
                        } else {
                            DownloadStatus::Failed
                        };
                        item.size_bytes = item
                            .path
                            .as_deref()
                            .filter(|_| success)
                            .and_then(|p| std::fs::metadata(p).ok().map(|m| m.len()));
                        item.filename.clone()
                    } else {
                        let size_bytes = path
                            .as_deref()
                            .filter(|_| success)
                            .and_then(|p| std::fs::metadata(p).ok().map(|m| m.len()));
                        let filename = super::helpers::download_display_name(path.as_deref(), &url);
                        let status = if success {
                            DownloadStatus::Completed
                        } else {
                            DownloadStatus::Failed
                        };
                        self.push_download(url, filename.clone(), path, status, size_bytes);
                        filename
                    };
                    if success && !self.has_downloads_popup() {
                        self.unseen_downloads = self.unseen_downloads.saturating_add(1);
                    }
                    let msg = if success {
                        format!("다운로드 완료: {filename}")
                    } else {
                        format!("저장 실패: {filename}")
                    };
                    self.set_status_note(&msg);
                    self.rebuild_chrome();
                    self.open_downloads_popup(event_loop, false);
                }
                AppEvent::DownloadBlocked { url, filename } => {
                    self.push_download(url, filename.clone(), None, DownloadStatus::Blocked, None);
                    self.set_status_note(&format!("실행 파일 다운로드 차단됨: {filename}"));
                    self.rebuild_chrome();
                    self.open_downloads_popup(event_loop, false);
                }
                AppEvent::ChromeAction(action) => self.handle_chrome_action(&action, event_loop),
                AppEvent::SidebarAction(action) => self.handle_sidebar_action(&action),
                AppEvent::ChatPageContext { req_id, prompt, page } => self.start_chat_task(req_id, prompt, page),
                AppEvent::ChatPdfDetected {
                    req_id,
                    prompt,
                    title,
                    url,
                } => {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab)
                        && tab.url == url
                    {
                        tab.pdf_hint = Some(url.clone());
                    }
                    self.spawn_pdf_context_extraction(req_id, prompt, title, url);
                }
                AppEvent::AgentPdfSnapshot {
                    req_id,
                    title,
                    url,
                    extracted,
                } => self.on_agent_pdf_snapshot(req_id, title, url, extracted),
                AppEvent::ChatFinished {
                    req_id,
                    provider_label,
                    result,
                } => {
                    self.on_chat_finished(req_id, provider_label, result);
                    self.finish_live(req_id);
                }
                AppEvent::ChatStreamChunk {
                    req_id,
                    provider_label,
                    delta,
                } => self.on_chat_stream_chunk(req_id, provider_label, delta),
                AppEvent::UpdateCheckDone { manual, outcome } => self.on_update_check_done(manual, outcome),
                AppEvent::UpdateDownloadDone(result) => self.on_update_download_done(result),
                AppEvent::ApiTestDone { provider, ok, detail } => self.on_api_test_done(provider, ok, detail),
                AppEvent::AgentStepReady { req_id } => self.on_agent_step_ready(req_id),
                AppEvent::AgentLoadSettled { req_id, wait_gen } => self.on_agent_load_settled(req_id, wait_gen),
                AppEvent::AgentWaitCheck {
                    req_id,
                    wait_gen,
                    found,
                } => {
                    self.on_agent_wait_check(req_id, wait_gen, found);
                }
                AppEvent::AgentActionDone {
                    req_id,
                    ok,
                    summary,
                    need_delay_ms,
                } => self.on_agent_action_done(req_id, ok, summary, need_delay_ms),
                AppEvent::AgentExtractDone {
                    req_id,
                    ok,
                    summary,
                    extracted,
                    copy,
                } => self.on_agent_extract_done(req_id, ok, summary, extracted, copy),
                AppEvent::AgyModelsLoaded(models) => self.on_agy_models_loaded(models),
                AppEvent::ChatCompactDone(result) => self.on_chat_compact_done(result),
                AppEvent::ChatProgress { req_id, progress } => self.on_chat_progress(req_id, progress),
                AppEvent::ChatLiveTick(req_id) => self.on_chat_live_tick(req_id),
                AppEvent::AgentToolPending {
                    req_id,
                    js,
                    desc,
                    deadline,
                } => self.on_agent_tool_pending(req_id, js, desc, deadline),
                AppEvent::AgentToolRerun {
                    req_id,
                    js,
                    desc,
                    deadline,
                } => {
                    if self.chat_inflight.as_ref().is_some_and(|(id, _, _)| *id == req_id) {
                        self.run_agent_data_js(req_id, js, desc, deadline);
                    }
                }
                AppEvent::ApiModelsLoaded { provider, result } => self.on_api_models_loaded(provider, result),
                AppEvent::ChatRevealTick(reveal_gen) => self.on_chat_reveal_tick(reveal_gen),
                AppEvent::ChatScriptTimeout { req_id, script_seq } => self.on_chat_script_timeout(req_id, script_seq),
                AppEvent::RefreshChrome => {
                    self.chrome_motion_timer_armed = false;
                    self.rebuild_chrome();
                }
                AppEvent::OpenBookmarkPopup {
                    key,
                    root,
                    path,
                    index_offset,
                    title,
                } => self.open_bookmark_popup(event_loop, key, root, path, index_offset, title),
                AppEvent::PopupAction { action, new_tab } => {
                    self.handle_popup_action(&action, new_tab, event_loop);
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
}

/// URL to open in WebView2 when a Clean-mode load auto-switches to JS mode.
///
/// For GET navigations (`cacheable == true`), always hand WebView2 the originally requested URL
/// rather than a mid-redirect `final_url`: `blitz-net` and WebView2 have separate cookie jars,
/// so sending WebView2 directly to an IdP/OAuth redirect URL leaves behind the SP's session and
/// OAuth `state` cookies (causing 500 errors on the OAuth callback) and bypasses existing logins.
fn auto_js_target_url(requested_url: String, final_url: String, cacheable: bool) -> String {
    if cacheable { requested_url } else { final_url }
}

/// Only built-in `rust://` documents (or the built-in error page / reader toolbar) may navigate
/// to internal `rust://` action URLs. External web pages must never clear history, delete
/// bookmarks, or bounce `file://` URLs through `rust://open-js`.
fn is_page_rust_nav_allowed(page_url: &str, is_error_page: bool, reader_mode: bool, target: &str) -> bool {
    if page_url.starts_with("rust://") {
        return true;
    }
    if is_error_page && (target.starts_with("rust://open-js") || target == "rust://home") {
        return true;
    }
    reader_mode && target.starts_with("rust://reader-opt")
}

pub(super) fn is_page_file_nav_allowed(current_page_url: &str, target: &Url) -> bool {
    if crate::net::is_app_data_file_url(target) {
        return true;
    }
    let Ok(cur) = Url::parse(current_page_url) else {
        return false;
    };
    if cur.scheme() != "file" || target.scheme() != "file" {
        return false;
    }
    if target.path().split('/').any(|s| s == "..") {
        return false;
    }
    let (Ok(cur_path), Ok(target_path)) = (cur.to_file_path(), target.to_file_path()) else {
        return false;
    };
    if target_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    let Some(base_dir) = cur_path.parent() else {
        return false;
    };
    let (Ok(canon_base), Ok(canon_target)) = (std::fs::canonicalize(base_dir), std::fs::canonicalize(&target_path))
    else {
        return false;
    };
    canon_target.starts_with(&canon_base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_file_navigation_allows_only_same_or_subdir_files() {
        let root = std::env::temp_dir().join(format!("rcb_file_nav_{}", std::process::id()));
        let sub = root.join("sub");
        let deep = sub.join("nested");
        let _ = std::fs::create_dir_all(&deep);
        let cur_sub = sub.join("index.html");
        let sibling = sub.join("next.html");
        let child = deep.join("detail.html");
        let outside = root.join("secret.txt");
        for p in [&cur_sub, &sibling, &child, &outside] {
            let _ = std::fs::write(p, b"ok");
        }
        let cur_url = Url::from_file_path(&cur_sub).unwrap().to_string();
        assert!(is_page_file_nav_allowed(
            &cur_url,
            &Url::from_file_path(&sibling).unwrap()
        ));
        assert!(is_page_file_nav_allowed(
            &cur_url,
            &Url::from_file_path(&child).unwrap()
        ));
        assert!(!is_page_file_nav_allowed(
            &cur_url,
            &Url::from_file_path(&outside).unwrap()
        ));
        assert!(!is_page_file_nav_allowed(
            "https://example.com/",
            &Url::from_file_path(&sibling).unwrap()
        ));
        let traversal = Url::parse(&format!("{}../secret.txt", Url::from_directory_path(&sub).unwrap())).unwrap();
        assert!(!is_page_file_nav_allowed(&cur_url, &traversal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn auto_js_prefers_requested_url_for_get_redirects() {
        let req = "https://sas-demos.engage.sas.com/SASLanding/".to_string();
        let idp = "https://auth.sas.com/login/login.htm?fromURI=%2Foauth2%2Fv1%2Fauthorize%3Fokta_key%3Dx".to_string();
        assert_eq!(auto_js_target_url(req.clone(), idp.clone(), true), req);
        assert_eq!(auto_js_target_url(req, idp.clone(), false), idp);
    }

    #[test]
    fn blocks_external_pages_from_invoking_rust_internal_routes() {
        assert!(!is_page_rust_nav_allowed(
            "https://evil.test/",
            false,
            false,
            "rust://history?clear=1"
        ));
        assert!(!is_page_rust_nav_allowed(
            "https://evil.test/",
            false,
            false,
            "rust://open-js?url=file:///C:/Windows/win.ini"
        ));
        assert!(!is_page_rust_nav_allowed(
            "https://article.test/",
            false,
            true,
            "rust://history?clear=1"
        ));
        assert!(is_page_rust_nav_allowed(
            "https://article.test/",
            false,
            true,
            "rust://reader-opt?theme=dark"
        ));
        assert!(is_page_rust_nav_allowed(
            "https://down.test/",
            true,
            false,
            "rust://open-js?url=https%3A%2F%2Fdown.test%2F"
        ));
        assert!(is_page_rust_nav_allowed(
            "rust://history",
            false,
            false,
            "rust://history?clear=1"
        ));
    }
}
