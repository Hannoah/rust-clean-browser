//! Page loading, tab lifecycle, history and the JS-mode WebView.

use super::helpers::{RESIZE_EDGE, normalize_url_input, persist_session_cookies, requires_js_engine};
use super::{AppEvent, BrowserApp, ClosedTab};
use crate::chrome_ui::{
    parse_query_param, render_bookmarks_page, render_error_page, render_history_page,
    render_home_page,
};
use crate::cleaner::{ReaderFont, ReaderTheme, ReaderWidth, process_fetched_document};
use crate::memory::{trim_process_working_set, working_set_mb};
use crate::tabs::{HistoryEntry, Tab, short_title_for_url};
use blitz_traits::net::{AbortController, Request};
use blitz_traits::shell::ColorScheme;
use std::sync::Arc;
use url::Url;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;
use wry::{NewWindowResponse, PageLoadEvent, Rect as WryRect, WebContext, WebView, WebViewBuilder};

/// Maximum number of background JS-mode WebViews kept alive (hidden + suspended) in LRU order.
const MAX_PARKED_WEBVIEWS: usize = 5;

impl BrowserApp {
    pub(super) fn webview_bounds(&self) -> WryRect {
        let scale = self.hidpi_scale as f64;
        let logical_w = ((self.win_size.width as f64) / scale).round().max(1.0) as u32;
        let logical_h = ((self.win_size.height as f64) / scale).round().max(1.0) as u32;
        let chrome_h = self.chrome_height();
        // Leave a thin frame around the child WebView so window edges stay resizable.
        let inset = if self.maximized || !cfg!(target_os = "windows") { 0 } else { RESIZE_EDGE };
        let content_h = logical_h.saturating_sub(chrome_h + inset).max(1);
        WryRect {
            position: dpi::LogicalPosition::new(inset as i32, chrome_h as i32).into(),
            size: dpi::LogicalSize::new(logical_w.saturating_sub(inset * 2).max(1), content_h).into(),
        }
    }

    pub(super) fn destroy_active_webview(&mut self) {
        if let Some(wv) = self.active_webview.take() {
            self.discard_webview(wv);
        }
    }

    fn discard_webview(&mut self, wv: WebView) {
        persist_session_cookies(&wv);
        drop(wv);
        // The WebView may have held keyboard focus; don't leave the window without it.
        if let Some(win) = &self.window {
            super::helpers::focus_main_window(&**win, false);
        }
        trim_process_working_set();
    }

    /// Leaving a JS-mode tab: hide and suspend its WebView instead of destroying it, so switching
    /// back is instant (no reload, session preserved). Up to `MAX_PARKED_WEBVIEWS` are kept in LRU
    /// order; when full, the least-recently-used one is evicted and freed.
    fn park_active_webview(&mut self) {
        let Some(wv) = self.active_webview.take() else {
            return;
        };
        let tab_id = self.tabs[self.active_tab].id;
        persist_session_cookies(&wv);
        let _ = wv.set_visible(false);
        super::helpers::suspend_webview(&wv);
        if let Some(win) = &self.window {
            super::helpers::focus_main_window(&**win, false);
        }
        self.parked_webviews.retain(|(id, _)| *id != tab_id);
        while self.parked_webviews.len() >= MAX_PARKED_WEBVIEWS {
            if let Some((_, old)) = self.parked_webviews.pop_front() {
                self.discard_webview(old);
            }
        }
        self.parked_webviews.push_back((tab_id, wv));
        trim_process_working_set();
    }

    /// Frees the parked WebView if it belongs to `tab_id` (e.g. that tab was closed).
    fn drop_parked_webview(&mut self, tab_id: u64) {
        if let Some(pos) = self.parked_webviews.iter().position(|(id, _)| *id == tab_id) {
            if let Some((_, wv)) = self.parked_webviews.remove(pos) {
                self.discard_webview(wv);
            }
        }
    }

    pub(super) fn mount_or_navigate_webview(&mut self, tab_idx: usize) {
        let url = self.tabs[tab_idx].url.clone();
        if url.starts_with("rust://") {
            self.destroy_active_webview();
            return;
        }
        let tab_id = self.tabs[tab_idx].id;
        self.tabs[tab_idx].unload();
        self.tabs[tab_idx].js_mode = true;

        // Returning to a parked tab: wake and show its live WebView as-is.
        if self.active_webview.is_none() {
            if let Some(pos) = self.parked_webviews.iter().position(|(id, _)| *id == tab_id) {
                if let Some((_, wv)) = self.parked_webviews.remove(pos) {
                    super::helpers::resume_webview(&wv);
                    let _ = wv.set_bounds(self.webview_bounds());
                    let _ = wv.set_visible(true);
                    let _ = wv.zoom(self.tabs[tab_idx].zoom as f64);
                    self.active_webview = Some(wv);
                    return;
                }
            }
        }

        if let Some(wv) = &self.active_webview {
            self.tabs[tab_idx].is_loading = true;
            let _ = wv.set_bounds(self.webview_bounds());
            let _ = wv.zoom(self.tabs[tab_idx].zoom as f64);
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
        let bus_dl = self.bus.clone();

        let builder = if let Some(ctx) = self.web_context.as_mut() {
            WebViewBuilder::new_with_web_context(ctx)
        } else {
            WebViewBuilder::new()
        }
        .with_general_autofill_enabled(false);

        struct WinHandle<'a>(&'a dyn Window);
        impl wry::raw_window_handle::HasWindowHandle for WinHandle<'_> {
            fn window_handle(
                &self,
            ) -> Result<wry::raw_window_handle::WindowHandle<'_>, wry::raw_window_handle::HandleError>
            {
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
            // Drive-by downloads have no consent UI in this browser: deny and
            // surface a toast instead of dropping files into Downloads silently.
            .with_download_started_handler(move |url, _path| {
                bus_dl.send(AppEvent::DownloadBlocked(url));
                false
            })
            .build_as_child(&win_handle);

        match built {
            Ok(wv) => {
                #[cfg(debug_assertions)]
                eprintln!("[webview] mounted OK for {url}");
                let _ = wv.zoom(self.tabs[tab_idx].zoom as f64);
                self.tabs[tab_idx].is_loading = true;
                self.active_webview = Some(wv);
            }
            Err(e) => {
                #[cfg(debug_assertions)]
                eprintln!("[webview] mount FAILED for {url}: {e}");
                self.tabs[tab_idx].js_mode = false;
                self.tabs[tab_idx].force_blitz = true;
                self.show_error(tab_idx, &url, &format!("WebView2 초기화 실패: {e}"), None);
            }
        }
    }

    /// Replaces the tab's document with an error page (`title`: new tab title, if any).
    pub(super) fn show_error(&mut self, tab_idx: usize, url: &str, message: &str, title: Option<&str>) {
        let dark = self.color_scheme == ColorScheme::Dark;
        let err_html = render_error_page(url, message, dark);
        self.tabs[tab_idx].doc = None;
        self.tabs[tab_idx].raw_html = None;
        self.tabs[tab_idx].translated = None;
        let doc = self.build_content_doc(&err_html, "rust://error", None, self.tabs[tab_idx].zoom);
        let tab = &mut self.tabs[tab_idx];
        tab.doc = Some(doc);
        // Always clear the spinner; a None title keeps the previous title text.
        tab.is_loading = false;
        if let Some(t) = title {
            tab.title = t.to_string();
        }
        if tab_idx == self.active_tab {
            self.sync_find_bar_after_doc_change();
        }
    }

    pub(super) fn load_active_tab(&mut self, bypass_cache: bool, restore_scroll_y: Option<f64>) {
        self.ensure_active_valid();
        // HTTPS-Only: upgrade plain-http navigations before anything else touches them.
        if self.prefs.https_only && self.tabs[self.active_tab].url.starts_with("http://") {
            let upgraded = format!("https://{}", &self.tabs[self.active_tab].url["http://".len()..]);
            self.tabs[self.active_tab].url = upgraded;
            self.set_status_note("HTTPS로 업그레이드됨");
        }
        let idx = self.active_tab;
        let url = self.tabs[idx].url.clone();
        let tab_id = self.tabs[idx].id;
        self.tabs[idx].cancel_inflight();
        let load_seq = self.tabs[idx].load_seq;
        self.hover_link_url = None;
        self.link_preview_doc = None;
        self.clear_find_matches();

        // Internal built-in pages (always Pure Rust Blitz, zero WebView2)
        if url.starts_with("rust://") {
            self.destroy_active_webview();
            self.drop_parked_webview(tab_id);
            self.tabs[idx].js_mode = false;
            let dark = self.color_scheme == ColorScheme::Dark;
            let html = if url.starts_with("rust://bookmarks") {
                if parse_query_param(&url, "sync").as_deref() == Some("1") {
                    match self.bookmarks.import_from_chrome() {
                        Ok(n) => self.set_status_note(&format!("Chrome 북마크 {n}개 동기화")),
                        Err(e) => self.set_status_note(&e),
                    }
                    self.tabs[idx].url = "rust://bookmarks".to_string();
                } else if let Some(del_url) = parse_query_param(&url, "del") {
                    if self.bookmarks.remove_url(&del_url) {
                        self.set_status_note("북마크 삭제됨");
                    }
                    self.tabs[idx].url = "rust://bookmarks".to_string();
                }
                let drop_target = self.bm_drag.as_ref().and_then(|d| d.hover_folder.as_deref());
                render_bookmarks_page(&self.bookmarks, &self.tabs[idx].url, drop_target)
            } else if url.starts_with("rust://history") {
                if parse_query_param(&url, "clear").as_deref() == Some("1") {
                    self.history.clear();
                    self.set_status_note("방문 기록 비워짐");
                    self.tabs[idx].url = "rust://history".to_string();
                } else if let Some(del_url) = parse_query_param(&url, "del") {
                    if self.history.remove_url(&del_url) {
                        self.set_status_note("방문 기록 항목 삭제됨");
                    }
                    self.tabs[idx].url = "rust://history".to_string();
                }
                render_history_page(&self.history, &self.tabs[idx].url)
            } else {
                let unloaded = self.tabs.iter().filter(|t| !t.is_loaded()).count();
                render_home_page(
                    &self.bookmarks,
                    &self.history,
                    working_set_mb(),
                    self.tabs.len(),
                    unloaded,
                    self.net_provider.blocked_total(),
                    self.net_provider.masked_failure_total(),
                    dark,
                )
            };
            let zoom = self.tabs[idx].zoom;
            self.tabs[idx].doc = None;
            let mut doc = self.build_content_doc(&html, &url, None, zoom);
            self.restore_position(&mut doc, &url, restore_scroll_y);
            let tab = &mut self.tabs[idx];
            tab.title = short_title_for_url(&url);
            tab.doc = Some(doc);
            tab.raw_html = None;
            tab.translated = None;
            tab.is_loading = false;
            self.sync_find_bar_after_doc_change();
            self.rebuild_chrome();
            return;
        }

        if !self.tabs[idx].force_blitz && requires_js_engine(&url) {
            self.tabs[idx].js_mode = true;
        }
        if self.tabs[idx].js_mode && !self.tabs[idx].force_blitz {
            self.mount_or_navigate_webview(idx);
            self.sync_find_bar_after_doc_change();
            self.rebuild_chrome();
            return;
        }

        self.destroy_active_webview();
        self.drop_parked_webview(tab_id);

        let pending_request = self.tabs[idx].pending_request.take();
        let cache_key = url.split('#').next().unwrap_or(&url);
        let use_cache = !bypass_cache && pending_request.is_none();
        // Fast path: compact HTML cache hit (no DOM held in RAM when sleeping!)
        if use_cache {
            if let Some(cached) = self.html_cache.get(cache_key) {
                self.apply_html_to_tab(idx, url, cached.raw_html, restore_scroll_y);
                return;
            }
        }

        // Disk cache hit: render the last copy instantly, then revalidate in the background.
        let disk_hit = if use_cache { self.page_cache.get(cache_key) } else { None };

        let Ok(parsed_url) = Url::parse(&url) else {
            self.show_error(idx, &url, "잘못된 URL 형식입니다.", Some("Invalid URL"));
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
        let fail_url_2 = url;
        let strip_iframes = self.net_provider.is_filter_enabled();
        let cacheable = pending_request.is_none();
        let request = pending_request.unwrap_or_else(|| Request::get(parsed_url));
        self.net_provider.fetch_document(
            request,
            signal,
            move |final_url, bytes| {
                match process_fetched_document(&bytes, &final_url, strip_iframes) {
                    Ok((clean_html, is_spa_shell)) => {
                        bus_ok.send(AppEvent::PageLoaded {
                            tab_id,
                            load_seq,
                            requested_url,
                            cacheable,
                            final_url,
                            clean_html: Arc::from(clean_html),
                            is_spa_shell,
                            restore_scroll_y,
                        });
                    }
                    Err(reason) => {
                        bus_ok.send(AppEvent::PageLoadFailed {
                            tab_id,
                            load_seq,
                            url: fail_url_2,
                            error: reason,
                        });
                    }
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

    pub(super) fn navigate_current(&mut self, raw_input: &str) {
        self.navigate_with_request(raw_input, None);
    }

    pub(super) fn navigate_with_request(&mut self, raw_input: &str, request: Option<Request>) {
        self.close_popup();
        let target = normalize_url_input(raw_input);
        if crate::chrome_ui::is_suspicious_host(&target) {
            self.set_status_note("주의: 도메인 문자 체계가 수상합니다. 피싱 확인 후 진행하세요");
        }

        // Internal action routes from Reader Mode toolbar & Error page.
        if target.starts_with("rust://reader-opt") {
            self.handle_reader_option_url(&target);
            return;
        }
        if target.starts_with("rust://open-js") {
            if let Some(orig) = parse_query_param(&target, "url") {
                let idx = self.active_tab;
                self.tabs[idx].url = orig;
                self.tabs[idx].force_blitz = false;
                self.tabs[idx].js_mode = true;
                self.tabs[idx].reader_mode = false;
                self.set_status_note("JS 모드로 열기");
                self.mount_or_navigate_webview(idx);
                self.rebuild_chrome();
            }
            return;
        }

        let idx = self.active_tab;
        let current_url = self.tabs[idx].url.clone();

        // Same-document #fragment jump without refetching
        let cur_base = current_url.split('#').next().unwrap_or(&current_url);
        let new_base = target.split('#').next().unwrap_or(&target);
        if request.is_none() && !self.tabs[idx].js_mode && cur_base == new_base {
            if let (Some((_, frag)), Some(doc)) = (target.split_once('#'), &mut self.tabs[idx].doc) {
                doc.scroll_to_fragment(frag);
                // Record fragment navigations so Back restores the prior fragment/scroll.
                if current_url != target {
                    let scroll_y = self.current_scroll_y();
                    let prev_js = self.tabs[idx].js_mode;
                    let prev_force = self.tabs[idx].force_blitz;
                    self.tabs[idx].history_back.push(HistoryEntry {
                        url: current_url,
                        scroll_y,
                        js_mode: prev_js,
                        force_blitz: prev_force,
                    });
                    self.tabs[idx].history_forward.clear();
                }
                self.tabs[idx].url = target;
                self.chrome_focused = false;
                self.rebuild_chrome();
                return;
            }
        }

        // Navigating to a new URL in the active tab invalidates any active/parked WebView so
        // `tab.history_back` stays synchronized with WebView history and non-JS URLs return to Blitz.
        if current_url != target {
            let tab_id = self.tabs[idx].id;
            self.destroy_active_webview();
            self.drop_parked_webview(tab_id);
            let scroll_y = self.current_scroll_y();
            let prev_js = self.tabs[idx].js_mode;
            let prev_force = self.tabs[idx].force_blitz;
            self.tabs[idx].history_back.push(HistoryEntry {
                url: current_url.clone(),
                scroll_y,
                js_mode: prev_js,
                force_blitz: prev_force,
            });
            self.tabs[idx].history_forward.clear();
            self.tabs[idx].js_mode = false;
        }
        let same_url = current_url == target;
        self.tabs[idx].url = target;
        self.tabs[idx].scroll_y = 0.0;
        self.tabs[idx].force_blitz = false;
        self.tabs[idx].pending_request = request;
        self.chrome_focused = false;
        self.chrome_doc.clear_focus();
        // Same-URL address-bar navigation must revalidate, not serve memory cache instantly.
        self.load_active_tab(same_url, None);
    }

    pub(super) fn current_scroll_y(&self) -> f64 {
        self.tabs
            .get(self.active_tab)
            .and_then(|t| t.doc.as_ref())
            .map_or(0.0, |d| d.viewport_scroll().y)
    }

    /// Parks the leaving tab's WebView and drops its DOM, returning that memory to the OS.
    fn unload_active(&mut self) {
        self.close_popup();
        self.park_active_webview();
        if self.tabs[self.active_tab].unload() {
            trim_process_working_set();
        }
    }

    pub(super) fn switch_tab(&mut self, target_idx: usize) {
        if target_idx >= self.tabs.len() || target_idx == self.active_tab {
            return;
        }
        self.chrome_focused = false;
        self.chrome_doc.clear_focus();
        self.unload_active();
        self.active_tab = target_idx;
        let restore_y = Some(self.tabs[target_idx].scroll_y);
        self.load_active_tab(false, restore_y);
    }

    pub(super) fn open_new_tab(&mut self, url: &str) {
        self.open_new_tab_with_js(url, false);
    }

    /// Used for `window.open` / `target="_blank"` inside a JS-mode WebView so authenticated
    /// links keep using WebView2.
    pub(super) fn open_new_tab_inheriting_js(&mut self, url: &str) {
        let inherit_js = self.tabs.get(self.active_tab).is_some_and(|t| t.js_mode)
            && !url.starts_with("rust://");
        self.open_new_tab_with_js(url, inherit_js);
    }

    fn open_new_tab_with_js(&mut self, url: &str, js_mode: bool) {
        self.chrome_focused = false;
        self.chrome_doc.clear_focus();
        self.unload_active();
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, url.to_string());
        tab.js_mode = js_mode && !url.starts_with("rust://");
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.load_active_tab(false, None);
    }

    /// Ctrl+click: add a sleeping tab right after the active one without loading it.
    pub(super) fn open_background_tab(&mut self, url: &str) {
        if let Ok(parsed) = Url::parse(url) {
            if parsed.scheme() == "file" && !crate::net::is_app_data_file_url(&parsed) {
                self.set_status_note("차단됨: 페이지의 로컬 파일 접근");
                self.rebuild_chrome();
                return;
            }
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.insert(self.active_tab + 1, Tab::new(id, url.to_string()));
        self.set_status_note("백그라운드 탭 열림");
        self.rebuild_chrome();
    }

    pub(super) fn reopen_closed_tab(&mut self) {
        if let Some(closed) = self.closed_tabs.pop() {
            self.open_new_tab_with_js(&closed.url, closed.js_mode);
        }
    }

    pub(super) fn close_tab(&mut self, idx: usize, event_loop: Option<&dyn ActiveEventLoop>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.close_popup();
        let closed_url = self.tabs[idx].url.clone();
        if !closed_url.starts_with("rust://home") {
            if self.closed_tabs.len() >= 16 {
                self.closed_tabs.remove(0);
            }
            self.closed_tabs.push(ClosedTab {
                url: closed_url,
                js_mode: self.tabs[idx].js_mode,
            });
        }
        if idx == self.active_tab {
            self.destroy_active_webview();
            self.chrome_focused = false;
            self.chrome_doc.clear_focus();
        }
        self.drop_parked_webview(self.tabs[idx].id);
        if self.tabs.len() <= 1 {
            if let Some(el) = event_loop {
                self.exit_app(el);
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

    /// Back (`forward == false`) or forward one history entry, restoring its scroll position.
    pub(super) fn history_step(&mut self, forward: bool) {
        self.ensure_active_valid();
        let idx = self.active_tab;
        if self.tabs[idx].js_mode {
            if let Some(wv) = &self.active_webview {
                let can_step = if forward {
                    wv.can_go_forward().unwrap_or(false)
                } else {
                    wv.can_go_back().unwrap_or(false)
                };
                if can_step {
                    let res = if forward { wv.go_forward() } else { wv.go_back() };
                    if res.is_ok() {
                        // Keep tab stacks in sync optimistically; WebViewPageLoad
                        // will correct tab.url asynchronously when the page lands.
                        let scroll_y = self.current_scroll_y();
                        let cur = self.tabs[idx].url.clone();
                        // Move current URL to the opposite stack as a placeholder so
                        // rapid back/forward doesn't desync; real URL arrives via event.
                        if forward {
                            let entry = self.tabs[idx].history_forward.pop();
                            if let Some(e) = entry {
                                let cur_js = self.tabs[idx].js_mode;
                                let cur_force = self.tabs[idx].force_blitz;
                                self.tabs[idx].history_back.push(HistoryEntry {
                                    url: cur,
                                    scroll_y,
                                    js_mode: cur_js,
                                    force_blitz: cur_force,
                                });
                                self.tabs[idx].url = e.url;
                                self.tabs[idx].scroll_y = e.scroll_y;
                            }
                        } else {
                            let entry = self.tabs[idx].history_back.pop();
                            if let Some(e) = entry {
                                let cur_js = self.tabs[idx].js_mode;
                                let cur_force = self.tabs[idx].force_blitz;
                                self.tabs[idx].history_forward.push(HistoryEntry {
                                    url: cur,
                                    scroll_y,
                                    js_mode: cur_js,
                                    force_blitz: cur_force,
                                });
                                self.tabs[idx].url = e.url;
                                self.tabs[idx].scroll_y = e.scroll_y;
                            }
                        }
                        self.rebuild_chrome();
                        return;
                    }
                }
            }
        }
        let scroll_y = self.current_scroll_y();
        let entry = {
            let tab = &mut self.tabs[idx];
            let from = if forward { &mut tab.history_forward } else { &mut tab.history_back };
            from.pop()
        };
        let Some(entry) = entry else {
            return;
        };
        self.destroy_active_webview();
        let tab = &mut self.tabs[idx];
        let to = if forward { &mut tab.history_back } else { &mut tab.history_forward };
        to.push(HistoryEntry {
            url: std::mem::replace(&mut tab.url, entry.url),
            scroll_y,
            js_mode: tab.js_mode,
            force_blitz: tab.force_blitz,
        });
        tab.js_mode = entry.js_mode;
        tab.force_blitz = entry.force_blitz;
        self.load_active_tab(false, Some(entry.scroll_y));
    }

    pub(super) fn reload_active_tab(&mut self) {
        let idx = self.active_tab;
        if self.tabs[idx].js_mode {
            if let Some(wv) = &self.active_webview {
                let _ = wv.evaluate_script("window.location.reload();");
            } else {
                self.mount_or_navigate_webview(idx);
            }
        } else if self.tabs[idx].is_loading {
            self.tabs[idx].cancel_inflight();
            self.rebuild_chrome();
        } else {
            self.load_active_tab(true, None);
        }
    }

    pub(super) fn toggle_js_mode(&mut self) {
        let idx = self.active_tab;
        if self.tabs[idx].url.starts_with("rust://") {
            return;
        }
        let next = !self.tabs[idx].js_mode;
        self.tabs[idx].js_mode = next;
        self.tabs[idx].force_blitz = !next;
        if next {
            self.tabs[idx].reader_mode = false;
            self.set_status_note("JS Mode (WebView2)");
            self.mount_or_navigate_webview(idx);
            self.sync_find_bar_after_doc_change();
            self.rebuild_chrome();
        } else {
            self.destroy_active_webview();
            self.set_status_note("Pure Rust Mode (Blitz)");
            self.load_active_tab(false, None);
        }
    }

    fn handle_reader_option_url(&mut self, opt_url: &str) {
        if opt_url.contains("exit=1") {
            if self.tabs[self.active_tab].reader_mode {
                self.toggle_reader_mode();
            }
            return;
        }
        if let Some(theme) = parse_query_param(opt_url, "theme") {
            self.prefs.reader.theme = match theme.as_str() {
                "sepia" => ReaderTheme::Sepia,
                "dark" => ReaderTheme::Dark,
                _ => ReaderTheme::Light,
            };
        }
        if let Some(sz) = parse_query_param(opt_url, "size") {
            self.prefs.reader.font_size = match sz.as_str() {
                "inc" => (self.prefs.reader.font_size + 2).min(24),
                "dec" => self.prefs.reader.font_size.saturating_sub(2).max(14),
                _ => self.prefs.reader.font_size,
            };
        }
        if let Some(font) = parse_query_param(opt_url, "font") {
            self.prefs.reader.font = match font.as_str() {
                "serif" => ReaderFont::Serif,
                _ => ReaderFont::Sans,
            };
        }
        if let Some(width) = parse_query_param(opt_url, "width") {
            self.prefs.reader.width = match width.as_str() {
                "narrow" => ReaderWidth::Narrow,
                "wide" => ReaderWidth::Wide,
                _ => ReaderWidth::Medium,
            };
        }
        self.prefs.save();
        let idx = self.active_tab;
        if let Some(raw) = self.tabs[idx].shown_html() {
            let url = self.tabs[idx].url.clone();
            let scroll = self.current_scroll_y();
            self.apply_html_to_tab(idx, url, raw, Some(scroll));
        }
    }

    pub(super) fn toggle_reader_mode(&mut self) {
        let idx = self.active_tab;
        if self.tabs[idx].js_mode {
            // Reader mode needs the Blitz copy of the page: leave JS mode first.
            self.tabs[idx].js_mode = false;
            self.tabs[idx].force_blitz = true;
            self.destroy_active_webview();
            self.tabs[idx].reader_mode = true;
            self.load_active_tab(false, None);
            return;
        }
        self.tabs[idx].reader_mode = !self.tabs[idx].reader_mode;
        if let Some(raw) = self.tabs[idx].shown_html() {
            let url = self.tabs[idx].url.clone();
            let scroll = self.current_scroll_y();
            self.apply_html_to_tab(idx, url, raw, Some(scroll));
        } else {
            self.rebuild_chrome();
        }
    }

    pub(super) fn set_zoom(&mut self, zoom: f32) {
        let zoom = ((zoom * 10.0).round() / 10.0).clamp(0.3, 3.0);
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
}
