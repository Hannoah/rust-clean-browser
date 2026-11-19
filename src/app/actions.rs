//! `data-action` clicks from the browser UI and the async `AppEvent` queue.

use super::helpers::{primary_modifier, requires_js_engine};
use super::{AppEvent, BrowserApp};
use crate::bookmarks::BookmarkRootKind;
use crate::chrome_ui::short_host;
use crate::favicon;
use blitz_dom::Document;
use winit::event_loop::ActiveEventLoop;

impl BrowserApp {
    pub(super) fn handle_chrome_action(&mut self, action: &str, event_loop: &dyn ActiveEventLoop) {
        if let Some(rest) = action.strip_prefix("tab:") {
            if let Ok(idx) = rest.parse() {
                self.switch_tab(idx);
            }
        } else if let Some(rest) = action.strip_prefix("close-tab:") {
            if let Ok(idx) = rest.parse() {
                self.close_tab(idx, Some(event_loop));
            }
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
                        if !url.is_empty() && url != "about:blank" && self.tabs[idx].url != url {
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
                AppEvent::OpenInNewTab(url) => self.open_new_tab_inheriting_js(&url),
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
