//! `data-action` clicks from the browser UI and the async `AppEvent` queue.

use super::helpers::{arg_to_url, focus_main_window, primary_modifier, requires_js_engine};
use super::{AppEvent, BrowserApp};
use crate::bookmarks::BookmarkRootKind;
use crate::chrome_ui::short_host;
use crate::favicon;
use blitz_dom::Document;
use std::sync::Arc;
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
        let Some(raw) = tab.raw_html.clone().filter(|_| !tab.js_mode && !tab.url.starts_with("rust://")) else {
            self.set_status_note("번역은 Clean 모드의 웹 페이지에서만 가능 (JS 모드 제외)");
            return self.rebuild_chrome();
        };
        let segs = crate::translate::segments(&raw);
        if segs.is_empty() {
            self.set_status_note("번역할 텍스트가 없음");
            return self.rebuild_chrome();
        }
        let tl = crate::translate::target_lang(&segs);
        self.set_status_note(if tl == "en" { "영어로 번역 중…" } else { "한국어로 번역 중…" });
        self.rebuild_chrome();
        let bus = self.bus.clone();
        let texts = segs.iter().map(|s| s.text.clone()).collect();
        tokio::spawn(async move {
            let result = crate::translate::translate(texts, tl)
                .await
                .map(|tr| Arc::<str>::from(crate::translate::apply(&raw, &segs, &tr)));
            bus.send(AppEvent::Translated { tab_id, source: raw, tl, result });
        });
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
                self.queue_popup(action, BookmarkRootKind::Bar, Vec::new(), offset, "북마크 더보기".into());
            }
        } else {
            match action {
                "new-tab" => {
                    self.open_new_tab("rust://home");
                    self.focus_url_bar();
                }
                "back" => self.history_step(false),
                "forward" => self.history_step(true),
                "reload" => self.reload_active_tab(),
                "toggle-bookmark" => {
                    self.ensure_active_valid();
                    let tab = &self.tabs[self.active_tab];
                    let added = self.bookmarks.toggle_custom(&tab.title, &tab.url);
                    self.set_status_note(if added { "북마크 추가됨" } else { "북마크 해제됨" });
                    self.rebuild_chrome();
                }
                "translate" => self.toggle_translate(),
                "pdf-editor" => self.toggle_pdf_editor(),
                "toggle-js" => self.toggle_js_mode(),
                "toggle-reader" => self.toggle_reader_mode(),
                "toggle-filter" => {
                    self.net_provider.toggle_filter();
                    let on = self.net_provider.is_filter_enabled();
                    self.set_status_note(if on { "트래커 차단 켜짐" } else { "트래커 차단 꺼짐" });
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
                "cycle-theme" => self.cycle_theme(),
                "toggle-bm-bar" => self.toggle_bookmarks_bar(),
                "trim-memory" => {
                    crate::memory::trim_process_working_set();
                    let mb = crate::memory::working_set_mb();
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
                    if let Some(doc) = &mut self.tabs[self.active_tab].doc {
                        if doc.id() == doc_id {
                            doc.poll(None);
                            self.request_redraw();
                        }
                    }
                    if self.chrome_doc.id() == doc_id {
                        self.chrome_doc.poll(None);
                        self.request_redraw();
                    }
                    if let Some(popup) = &mut self.popup {
                        if popup.doc.id() == doc_id {
                            popup.doc.poll(None);
                            popup.window.request_redraw();
                        }
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
                        self.tabs[idx].url = final_url;
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
                    if error.starts_with(crate::cleaner::NOT_HTML_ERR)
                        && !self.tabs[idx].js_mode
                        && !self.tabs[idx].force_blitz
                        && (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://"))
                    {
                        self.tabs[idx].pdf_hint = Some(url.clone());
                        self.tabs[idx].url = url;
                        self.tabs[idx].is_loading = false;
                        self.set_status_note("WebView2 뷰어로 열기");
                        self.mount_or_navigate_webview(idx);
                        self.rebuild_chrome();
                        continue;
                    }
                    if self.tabs[idx].revalidating {
                        // Offline / server error: keep showing the cached copy.
                        self.tabs[idx].revalidating = false;
                        self.tabs[idx].is_loading = false;
                        self.set_status_note("오프라인 캐시 표시 중");
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
                        // Page-initiated `file://` navigation outside our app-data dir is
                        // blocked (local-file exfiltration lure). Typed address-bar
                        // input still allows local files via `navigate_current`.
                        if url.scheme() == "file" && !crate::net::is_app_data_file_url(&url) {
                            self.set_status_note("차단됨: 페이지의 로컬 파일 접근");
                            self.rebuild_chrome();
                            continue;
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
                        if request.is_none()
                            && !is_builtin_action
                            && primary_modifier(self.keyboard_modifiers.state())
                        {
                            self.open_background_tab(url_str);
                        } else {
                            self.navigate_with_request(url_str, request);
                        }
                    }
                }
                AppEvent::Translated { tab_id, source, tl, result } => {
                    let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else { continue };
                    // Ignore if the tab navigated away (or unloaded) while translating.
                    if !self.tabs[idx].raw_html.as_ref().is_some_and(|r| Arc::ptr_eq(r, &source)) {
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
                AppEvent::WebViewTitleChanged { tab_id, title } => {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        if !title.trim().is_empty() && self.tabs[idx].title != title {
                            self.history.set_title(&self.tabs[idx].url, &title);
                            self.tabs[idx].title = title;
                            if idx == self.active_tab {
                                self.query_webview_icons();
                            }
                            self.rebuild_chrome();
                        }
                    }
                }
                AppEvent::WebViewPageLoad {
                    tab_id,
                    url,
                    finished,
                } => {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                        self.tabs[idx].is_loading = !finished;
                        // The annotator's own viewer URL must not replace the PDF's real URL.
                        if !url.is_empty() && url != "about:blank" && self.tabs[idx].url != url && !self.tabs[idx].editing_pdf() {
                            self.tabs[idx].url = url;
                            if !finished {
                                self.tabs[idx].title = crate::tabs::short_title_for_url(&self.tabs[idx].url);
                            }
                        }
                        if finished {
                            self.history.record(&self.tabs[idx].url, &self.tabs[idx].title);
                            if idx == self.active_tab {
                                self.query_webview_icons();
                            }
                        }
                        self.rebuild_chrome();
                    }
                }
                // A denied `about:blank` popup has nothing to show; never open it as a blank tab.
                AppEvent::OpenInNewTab(url) if url == "about:blank" => {}
                AppEvent::OpenInNewTab(url) => self.open_new_tab_inheriting_js(&url),
                AppEvent::DownloadDone { path, success } => {
                    let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
                    let msg = match (success, name) {
                        (true, Some(n)) => format!("다운로드 폴더에 저장됨: {n}"),
                        (true, None) => "저장 완료".to_string(),
                        (false, _) => "저장 실패".to_string(),
                    };
                    self.set_status_note(&msg);
                    self.rebuild_chrome();
                }
                AppEvent::DownloadBlocked(url) => {
                    self.set_status_note(&format!("다운로드 차단됨: {}", short_host(&url)));
                    self.rebuild_chrome();
                }
                AppEvent::ChromeAction(action) => self.handle_chrome_action(&action, event_loop),
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
