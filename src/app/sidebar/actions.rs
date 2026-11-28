use super::agent::{format_agent_final_message, normalize_agent_host, user_text_fallback};
use super::{AgentRun, MAX_CHAT_FONT, MIN_CHAT_FONT};
use crate::app::{AppEvent, BrowserApp};
use crate::chat::{
    AgentContext, AgentPreset, ChatHistory, ChatMessage, ChatProvider, ChatRole, PageContext,
    build_page_context_from_text, display_model_label, extract_page_context_from_html, is_loopback_endpoint,
    open_cli_login_terminal, parse_agent_response, run_chat,
};
use crate::chrome_ui::SidebarPicker;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

impl BrowserApp {
    pub(in crate::app) fn handle_sidebar_action(&mut self, action: &str) {
        if let Some(pid) = action.strip_prefix("chat-provider:") {
            if let Some(provider) = ChatProvider::from_id(pid) {
                self.sidebar_picker = SidebarPicker::None;
                self.chat_config.provider = provider;
                if !provider.is_cli() {
                    let local =
                        url::Url::parse(self.chat_config.effective_endpoint()).is_ok_and(|u| is_loopback_endpoint(&u));
                    if self.chat_config.effective_api_key().is_empty() && !local {
                        self.chat_show_settings = true;
                    }
                }
                if provider != ChatProvider::Agy {
                    crate::chat::reset_agy_worker();
                }
                self.maybe_fetch_models(false);
                if provider != ChatProvider::Claude {
                    crate::chat::reset_claude_worker();
                }
                self.warm_cli_for_current_mode();
                self.chat_config.save();
                self.sidebar_reset_drafts = true;
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            return;
        }
        if let Some(mid) = action.strip_prefix("chat-model:") {
            self.chat_config
                .set_selected_model(self.chat_config.provider, mid.to_string());
            self.sidebar_picker = SidebarPicker::None;
            self.chat_config.save();
            if !self.chat_config.agent_control {
                match self.chat_config.provider {
                    ChatProvider::Agy => {
                        crate::chat::reset_agy_worker();
                        self.warm_cli_for_current_mode();
                    }
                    ChatProvider::Claude => {
                        crate::chat::reset_claude_worker();
                        self.warm_cli_for_current_mode();
                    }
                    _ => {}
                }
            }
            self.sidebar_reset_drafts = true;
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(false);
            return;
        }
        if let Some(mid) = action.strip_prefix("chat-ctrl-model:") {
            self.chat_config
                .set_control_model(self.chat_config.provider, mid.to_string());
            self.chat_config.save();
            if self.chat_config.agent_control {
                match self.chat_config.provider {
                    ChatProvider::Agy => {
                        crate::chat::reset_agy_worker();
                        self.warm_cli_for_current_mode();
                    }
                    ChatProvider::Claude => {
                        crate::chat::reset_claude_worker();
                        self.warm_cli_for_current_mode();
                    }
                    _ => {}
                }
            }
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(false);
            return;
        }
        if let Some(level) = action.strip_prefix("chat-effort:") {
            self.chat_config
                .set_effort(self.chat_config.provider, level.to_string());
            self.chat_config.save();
            match self.chat_config.provider {
                ChatProvider::Agy => {
                    crate::chat::reset_agy_worker();
                    self.warm_cli_for_current_mode();
                }
                ChatProvider::Claude => {
                    crate::chat::reset_claude_worker();
                    self.warm_cli_for_current_mode();
                }
                _ => {}
            }
            self.sidebar_reset_drafts = true;
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(false);
            return;
        }
        if let Some(mid) = action.strip_prefix("chat-custom-model-del:") {
            self.delete_custom_model(mid);
            return;
        }
        if let Some(idx_str) = action.strip_prefix("chat-preset-del:") {
            if let Ok(idx) = idx_str.parse::<usize>()
                && idx < self.chat_config.custom_presets.len()
            {
                self.chat_config.custom_presets.remove(idx);
                self.chat_config.save();
                self.set_status_note("프리셋 삭제됨");
                self.last_sidebar_html.clear();
                self.rebuild_chrome();
                self.rebuild_sidebar_now(false);
            }
            return;
        }
        if let Some(idx_str) = action.strip_prefix("chat-preset-run:") {
            if let Ok(idx) = idx_str.parse::<usize>()
                && let Some(preset) = self.chat_config.custom_presets.get(idx).cloned()
            {
                // A control preset runs in agent mode once; the saved mode stays as the user set it.
                self.submit_chat_message(preset.prompt, true, true);
            }
            return;
        }
        if let Some(kind) = action.strip_prefix("chat-quick:") {
            let prompt = match kind {
                "summary" => "현재 웹페이지의 핵심 내용을 이해하기 쉽게 요약해 주세요.",
                "bullets" => "현재 웹페이지의 핵심 포인트를 3~5개의 글머리 기호로 정리해 주세요.",
                "explain-ko" => "현재 웹페이지의 내용을 한국어로 알기 쉽게 풀어서 설명해 주세요.",
                "translate" => "현재 웹페이지의 핵심 내용을 자연스러운 한국어로 번역해 주세요.",
                "extract-tables" => {
                    "현재 페이지의 모든 표(Table)를 extract(tables)로 추출해서 깔끔한 마크다운 표로 정리해 주세요."
                }
                "compare-tabs" => "현재 열려 있는 주요 탭들을 전환하며 확인하고 핵심 내용을 비교 표로 정리해 주세요.",
                "extract-links" => "현재 페이지의 주요 링크를 extract(links)로 수집해서 주제별로 정리해 주세요.",
                _ => return,
            };
            self.submit_chat_message(prompt.to_string(), true, self.chat_config.agent_control);
            return;
        }
        if let Some(id) = action.strip_prefix("chat-load:").and_then(|v| v.parse::<u64>().ok()) {
            self.load_chat_session(id);
            return;
        }
        if let Some(id) = action.strip_prefix("chat-del:").and_then(|v| v.parse::<u64>().ok()) {
            self.delete_chat_session(Some(id));
            return;
        }
        if let Some(step) = action.strip_prefix("chat-font:") {
            let cur = self.prefs.chat_font_size.clamp(MIN_CHAT_FONT, MAX_CHAT_FONT);
            self.prefs.chat_font_size = match step {
                "inc" => (cur + 1).min(MAX_CHAT_FONT),
                "dec" => cur.saturating_sub(1).max(MIN_CHAT_FONT),
                _ => crate::prefs::DEFAULT_CHAT_FONT,
            };
            self.prefs.save();
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(false);
            return;
        }
        if let Some(choice) = action.strip_prefix("agent-confirm:") {
            self.resolve_agent_confirm(choice == "allow");
            return;
        }
        if let Some(idx_str) = action.strip_prefix("chat-copy-msg:") {
            if let Ok(idx) = idx_str.parse::<usize>()
                && let Some(text) = self.chat_messages.get(idx).map(|m| m.text.clone())
            {
                self.copy_to_clipboard(&text);
            }
            return;
        }
        if let Some(idx) = action
            .strip_prefix("chat-activity:")
            .and_then(|v| v.parse::<usize>().ok())
        {
            if !self.chat_activity_open.remove(&idx) {
                self.chat_activity_open.insert(idx);
            }
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(false);
            return;
        }
        if let Some(idx_str) = action.strip_prefix("chat-copy-code:") {
            if let Ok(idx) = idx_str.parse::<usize>()
                && let Some(code) = self.sidebar_code_blocks.get(idx).cloned()
            {
                self.copy_to_clipboard(&code);
            }
            return;
        }

        match action {
            "toggle-sidebar" => self.toggle_sidebar(),
            "chat-picker:provider" => {
                self.sidebar_picker = if self.sidebar_picker == SidebarPicker::Provider {
                    SidebarPicker::None
                } else {
                    self.chat_show_settings = false;
                    SidebarPicker::Provider
                };
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-picker:model" => {
                self.sidebar_picker = if self.sidebar_picker == SidebarPicker::Model {
                    SidebarPicker::None
                } else {
                    self.chat_show_settings = false;
                    self.maybe_fetch_models(false);
                    SidebarPicker::Model
                };
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-refresh-models" => self.maybe_fetch_models(true),
            "chat-retry" => self.retry_last_chat(),
            "chat-open-login" => {
                let note = match open_cli_login_terminal(self.chat_config.provider) {
                    Ok(()) => format!("{} 로그인 터미널 열림", self.chat_config.provider.short_label()),
                    Err(msg) => msg,
                };
                self.set_status_note(&note);
                self.rebuild_chrome();
            }
            "chat-clear" => {
                self.cancel_chat_inflight(false);
                self.chat_messages.clear();
                self.chat_session_id = None;
                self.chat_activity_open.clear();
                self.chat_reveal = None;
                self.chat_show_history = false;
                crate::chat::reset_all_cli_workers();
                if self.sidebar_open {
                    self.warm_cli_for_current_mode();
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-toggle-history" => {
                self.chat_show_history = !self.chat_show_history;
                if self.chat_show_history {
                    self.chat_show_settings = false;
                    self.sidebar_picker = SidebarPicker::None;
                    if self.chat_history.is_none() {
                        self.chat_history = Some(ChatHistory::load());
                    }
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-del-all" => self.delete_chat_session(None),
            "chat-regen" => self.regenerate_last_reply(),
            // Header tabs: 대화 / 기록 / 설정 each take the whole sidebar body.
            "chat-tab:chat" | "chat-tab:history" | "chat-tab:settings" => {
                self.chat_show_history = action == "chat-tab:history";
                self.chat_show_settings = action == "chat-tab:settings";
                self.sidebar_picker = SidebarPicker::None;
                if self.chat_show_history {
                    // Re-read: other windows may have saved conversations since.
                    self.chat_history = Some(ChatHistory::load());
                }
                if self.chat_show_settings {
                    self.maybe_fetch_models(false);
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
                if action == "chat-tab:chat" {
                    self.focus_sidebar_input("chat-input");
                }
            }
            "chat-toggle-settings" => {
                self.chat_show_settings = !self.chat_show_settings;
                if self.chat_show_settings {
                    self.chat_show_history = false;
                    self.sidebar_picker = SidebarPicker::None;
                    self.maybe_fetch_models(false);
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-save-settings" => self.save_sidebar_settings(),
            "chat-preset-add" => self.add_sidebar_preset(),
            "chat-custom-model-add" => self.add_custom_model(),
            "chat-clear-key" => {
                self.chat_config.set_api_key(String::new());
                let note = if self.chat_config.save() {
                    "API 키 삭제됨"
                } else {
                    "API 키 삭제 저장 실패 (암호화 오류)"
                };
                self.set_status_note(note);
                self.last_sidebar_html.clear();
                self.rebuild_chrome();
                self.rebuild_sidebar_now(false);
            }
            "chat-toggle-page" => {
                self.chat_config.include_page = !self.chat_config.include_page;
                self.chat_config.save();
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-toggle-control" => {
                self.chat_config.agent_control = !self.chat_config.agent_control;
                self.chat_config.save();
                match self.chat_config.provider {
                    ChatProvider::Agy => {
                        crate::chat::reset_agy_worker();
                        self.warm_cli_for_current_mode();
                    }
                    ChatProvider::Claude => {
                        crate::chat::reset_claude_worker();
                        self.warm_cli_for_current_mode();
                    }
                    _ => {}
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-toggle-width" => {
                let target = if self.sidebar_logical_width() >= 520 { 380 } else { 600 };
                self.prefs.sidebar_width = target;
                self.prefs.save();
                self.sync_viewports();
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(false);
            }
            "chat-compact" => self.compact_chat(),
            "chat-send" => self.submit_from_sidebar_input(),
            "chat-stop" => self.cancel_chat_inflight(true),
            _ => {}
        }
    }

    pub(in crate::app) fn save_sidebar_settings(&mut self) {
        if let Some(ep) = self.read_sidebar_input("chat-api-endpoint") {
            self.chat_config.set_api_endpoint(ep);
        }
        if let Some(key) = self.read_sidebar_input("chat-api-key")
            && !key.trim().is_empty()
        {
            self.chat_config.set_api_key(key);
        }
        let saved = self.chat_config.save();
        self.last_sidebar_html.clear();
        self.set_status_note(if saved {
            "AI 설정 저장됨"
        } else {
            "AI 설정 저장 실패 (암호화 오류)"
        });
        self.rebuild_chrome();
        self.rebuild_sidebar_now(false);
        // Saving API settings immediately proves the endpoint/key/model end to end.
        if saved && !self.chat_config.provider.is_cli() {
            self.test_api_connection();
            // A new key/endpoint may unlock a different set of models.
            self.maybe_fetch_models(true);
        }
    }

    pub(in crate::app) fn add_sidebar_preset(&mut self) {
        let title = self
            .read_sidebar_input("chat-preset-title")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let prompt = self
            .read_sidebar_input("chat-preset-prompt")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if title.is_empty() || prompt.is_empty() {
            self.set_status_note("프리셋 이름과 지시 내용을 모두 입력하세요");
            self.rebuild_chrome();
            return;
        }
        if self.chat_config.custom_presets.len() >= 12 {
            self.set_status_note("프리셋은 최대 12개까지 저장할 수 있습니다");
            self.rebuild_chrome();
            return;
        }
        self.chat_config.custom_presets.push(AgentPreset { title, prompt });
        self.chat_config.save();
        self.set_status_note("제어 프리셋 추가됨");
        self.last_sidebar_html.clear();
        self.rebuild_chrome();
        self.rebuild_sidebar_now(false);
    }

    pub(in crate::app) fn add_custom_model(&mut self) {
        let id = self
            .read_sidebar_input("chat-custom-model")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let provider = self.chat_config.provider;
        // CLI agents take any model ID directly; API endpoints keep it in the list too.
        let ok = crate::chat::is_valid_model_id(&id)
            && (provider.is_cli()
                || self.current_model_options().iter().any(|o| o.id == id)
                || self.chat_config.add_custom_model(provider, id.clone()));
        if ok {
            self.chat_config.set_selected_model(provider, id);
            self.chat_config.save();
            self.sidebar_reset_drafts = true;
            self.set_status_note("모델 추가 및 선택됨");
        } else {
            self.set_status_note("추가할 수 없는 모델 ID입니다 (형식·중복·최대 20개 확인)");
        }
        self.last_sidebar_html.clear();
        self.rebuild_chrome();
        self.rebuild_sidebar_now(false);
    }

    pub(in crate::app) fn delete_custom_model(&mut self, id: &str) {
        if self.chat_config.remove_custom_model(self.chat_config.provider, id) {
            self.chat_config.save();
            self.set_status_note("모델 삭제됨");
        }
        self.last_sidebar_html.clear();
        self.rebuild_chrome();
        self.rebuild_sidebar_now(false);
    }

    pub(in crate::app) fn submit_from_sidebar_input(&mut self) {
        let Some(raw) = self.read_sidebar_input("chat-input") else {
            return;
        };
        let prompt = raw.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if let Some(doc) = &mut self.sidebar_doc
            && let Some(id) = doc.get_element_by_id("chat-input")
        {
            doc.with_text_input(id, |mut driver| {
                driver.select_all();
                driver.delete_selection();
            });
        }
        self.chat_input_lines = 1;
        // Small talk ("하이", "고마워") never needs the page: plain reply even in 제어 mode.
        let agent = self.chat_config.agent_control && !crate::chat::is_small_talk(&prompt);
        self.submit_chat_message(prompt, false, agent);
    }

    pub(in crate::app) fn sidebar_has_selection(&self) -> bool {
        self.sidebar_doc.as_ref().is_some_and(|d| d.has_text_selection())
    }

    /// Ctrl+C on text dragged inside the conversation. Handled here rather than by Blitz, whose
    /// own copy is skipped while the chat box keeps focus.
    pub(in crate::app) fn copy_sidebar_selection(&mut self) {
        let Some(doc) = self.sidebar_doc.as_ref() else {
            return;
        };
        // Like `get_selected_text`, but paragraphs / list items / table cells stay on their own
        // lines instead of being joined with spaces.
        let parts: Vec<&str> = doc
            .get_text_selection_ranges()
            .into_iter()
            .filter_map(|(id, start, end)| {
                let text = &doc.get_node(id)?.element_data()?.inline_layout_data.as_ref()?.text;
                text.get(start..end)
            })
            .filter(|t| !t.trim().is_empty())
            .collect();
        if !parts.is_empty() {
            self.copy_to_clipboard(&parts.join("\n"));
        }
    }

    pub(super) fn copy_to_clipboard(&mut self, text: &str) {
        let msg = match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
            Ok(()) => "클립보드에 복사됨",
            Err(_) => "클립보드 복사 실패",
        };
        self.set_status_note(msg);
        self.rebuild_chrome();
    }

    /// 대화 압축: asks the API model to fold the conversation into one summary message;
    /// later API requests carry only that summary plus the turns after it.
    pub(in crate::app) fn compact_chat(&mut self) {
        if self.chat_compacting.is_some() || self.chat_inflight.is_some() || self.chat_config.provider.is_cli() {
            return;
        }
        let prompt = crate::chat::compact_prompt(&self.chat_messages);
        self.chat_compacting = Some((self.chat_session_id, self.chat_messages.len()));
        let config = self.chat_config.clone();
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let cancel = Arc::new(AtomicBool::new(false));
            let result = run_chat(config, Vec::new(), prompt, None, None, cancel, None, None).await;
            bus.send(AppEvent::ChatCompactDone(result));
        });
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(false);
    }

    pub(in crate::app) fn on_chat_compact_done(&mut self, result: Result<String, String>) {
        let Some(started) = self.chat_compacting.take() else {
            return;
        };
        let same_chat = started == (self.chat_session_id, self.chat_messages.len());
        let note = match result {
            Ok(text) if same_chat && !text.trim().is_empty() => {
                self.chat_messages.push(ChatMessage {
                    role: ChatRole::Summary,
                    text: text.trim().to_string(),
                    page_title: None,
                    provider_label: None,
                    activity: Vec::new(),
                    elapsed_ms: None,
                });
                self.persist_chat();
                "대화 압축됨".to_string()
            }
            Ok(_) if !same_chat => "대화가 바뀌어 압축 결과를 버렸습니다".to_string(),
            Ok(_) => "대화 압축 실패: 빈 요약".to_string(),
            Err(e) => format!("대화 압축 실패: {}", e.chars().take(100).collect::<String>()),
        };
        self.set_status_note(&note);
        self.rebuild_chrome();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    pub(in crate::app) fn submit_chat_message(&mut self, user_text: String, force_page: bool, agent: bool) {
        self.sync_chat_to_active_tab();
        if self.chat_compacting.is_some() {
            self.set_status_note("대화 압축이 끝난 뒤 보내 주세요");
            self.rebuild_chrome();
            return;
        }
        self.cancel_chat_inflight(false);
        self.chat_show_history = false;
        self.ensure_active_valid();

        let use_page = force_page || self.chat_config.include_page;
        let active = &self.tabs[self.active_tab];
        let active_title = active.title.clone();
        let active_url = active.url.clone();
        let attach_title = if use_page && !active_url.starts_with("rust://") {
            Some(active_title)
        } else {
            None
        };

        self.chat_messages.push(ChatMessage {
            role: ChatRole::User,
            text: user_text.clone(),
            page_title: attach_title,
            provider_label: None,
            activity: Vec::new(),
            elapsed_ms: None,
        });
        self.persist_chat();

        self.dispatch_chat_for_last_user(user_text, use_page, agent);
    }

    pub(super) fn retry_last_chat(&mut self) {
        if self.chat_inflight.is_some() {
            return;
        }
        while self.chat_messages.last().is_some_and(|m| m.role == ChatRole::Error) {
            self.chat_messages.pop();
        }
        let Some(last_user) = self.chat_messages.iter().rev().find(|m| m.role == ChatRole::User) else {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
            return;
        };
        let prompt = last_user.text.clone();
        let use_page = last_user.page_title.is_some() || self.chat_config.include_page;
        self.dispatch_chat_for_last_user(prompt, use_page, self.chat_config.agent_control);
    }

    pub(super) fn dispatch_chat_for_last_user(&mut self, user_text: String, use_page: bool, agent: bool) {
        self.ensure_active_valid();
        let idx = self.active_tab;
        let active = &self.tabs[idx];
        let active_title = active.title.clone();
        let active_url = active.url.clone();

        self.chat_req_seq += 1;
        let req_id = self.chat_req_seq;
        let cancel = Arc::new(AtomicBool::new(false));

        if agent {
            let target_tab_id = self.tabs[idx].id;
            let mut trusted_hosts = Vec::new();
            if let Some(start_host) = url::Url::parse(&active_url)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .and_then(|u| u.host_str().map(normalize_agent_host))
            {
                trusted_hosts.push(start_host);
            }
            self.agent_run = Some(AgentRun {
                req_id,
                target_tab_id,
                goal: user_text,
                step: 0,
                max_steps: 10,
                action_log: Vec::new(),
                notes: Vec::new(),
                status: "생각 중…".to_string(),
                pending_actions: VecDeque::new(),
                finish_after_queue: false,
                finish_cleaned: String::new(),
                finish_summary: String::new(),
                provider_label: String::new(),
                last_action: None,
                consecutive_errors: 0,
                awaiting_load: None,
                awaiting_text: None,
                load_wait_seq: 0,
                extracted_data: None,
                screenshot: None,
                trusted_hosts,
                awaiting_confirm: None,
            });
            let dummy = tokio::spawn(async {});
            self.chat_inflight = Some((req_id, cancel, dummy.abort_handle()));
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(true);

            // Controlling the live DOM requires WebView2 (JS mode). If an external URL is
            // currently in Clean Blitz mode, transparently promote it first.
            if !active_url.starts_with("rust://") && self.active_webview.is_none() {
                let tab_id = self.tabs[idx].id;
                self.tabs[idx].force_blitz = false;
                self.mount_or_navigate_webview(idx);
                self.rebuild_chrome();
                self.arm_agent_load_wait(req_id, tab_id, None, 3200);
                return;
            }
            self.capture_agent_snapshot_and_run(req_id, true);
            return;
        }

        if use_page && !active_url.starts_with("rust://") {
            if active.is_pdf() {
                self.spawn_pdf_context_extraction(req_id, user_text, active_title, active_url);
                return;
            }
            if self.active_webview.is_some() {
                let dummy = tokio::spawn(async {});
                let abort = dummy.abort_handle();
                self.chat_inflight = Some((req_id, cancel, abort));
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(true);

                let bus = self.bus.clone();
                let prompt = user_text;
                let script_seq = self.arm_chat_script_timeout(req_id);
                let res = self.active_webview.as_ref().map(|wv| {
                    wv.evaluate_script_with_callback(
                        "(function(){try{var t=(document.body?document.body.innerText:'').slice(0,14000);if(!t.trim()&&(document.contentType==='application/pdf'||document.querySelector('embed[type=\"application/pdf\"]')))return '__RCB_PDF__';return t;}catch(e){return '';}})()",
                        move |json| {
                            let raw_text = serde_json::from_str::<String>(&json).unwrap_or_default();
                            if raw_text == "__RCB_PDF__" {
                                bus.send(AppEvent::ChatPdfDetected {
                                    req_id,
                                    prompt: prompt.clone(),
                                    title: active_title.clone(),
                                    url: active_url.clone(),
                                });
                                return;
                            }
                            let ctx = build_page_context_from_text(&active_title, &active_url, &raw_text);
                            bus.send(AppEvent::ChatPageContext {
                                req_id,
                                prompt: prompt.clone(),
                                page: Some(ctx),
                            });
                        },
                    )
                });
                let _ = script_seq;
                if !matches!(res, Some(Ok(()))) {
                    self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
                    self.start_chat_task(req_id, user_text_fallback(&self.chat_messages), None);
                }
                return;
            }
            let page = active
                .shown_html()
                .map(|raw| extract_page_context_from_html(&raw, &active_title, &active_url));
            self.start_chat_task_with_cancel(req_id, user_text, page, cancel);
        } else {
            self.start_chat_task_with_cancel(req_id, user_text, None, cancel);
        }
    }

    pub(in crate::app) fn spawn_pdf_context_extraction(
        &mut self,
        req_id: u64,
        prompt: String,
        title: String,
        url: String,
    ) {
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
        let cancel = self
            .chat_inflight
            .as_ref()
            .filter(|(id, _, _)| *id == req_id)
            .map(|(_, c, _)| Arc::clone(c))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let bus = self.bus.clone();
        let cookies = Arc::clone(&self.net_provider.cookies);
        let handle = tokio::spawn(async move {
            let text = match crate::pdfjs::load_pdf_bytes(&url, cookies).await {
                Ok(bytes) => match tokio::task::spawn_blocking(move || crate::pdfjs::extract_pdf_text(&bytes, 14_000)).await {
                    Ok(Ok(s)) if !s.trim().is_empty() => s,
                    Ok(Ok(_)) => {
                        "[이 PDF에는 추출 가능한 텍스트 레이어가 없습니다. 스캔된 이미지 PDF이거나 텍스트 복사가 제한된 문서일 수 있습니다.]".to_string()
                    }
                    Ok(Err(e)) => format!("[PDF 텍스트를 읽지 못했습니다: {e}]"),
                    Err(e) => format!("[PDF 텍스트 추출 작업 실패: {e}]"),
                },
                Err(e) => format!("[PDF 파일을 불러오지 못했습니다: {e}]"),
            };
            let ctx = build_page_context_from_text(&title, &url, &text);
            bus.send(AppEvent::ChatPageContext {
                req_id,
                prompt,
                page: Some(ctx),
            });
        });
        self.chat_inflight = Some((req_id, cancel, handle.abort_handle()));
        self.last_sidebar_html.clear();
        self.rebuild_sidebar_now(true);
    }

    pub(in crate::app) fn start_chat_task(&mut self, req_id: u64, prompt: String, page: Option<PageContext>) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
        let cancel = self
            .chat_inflight
            .as_ref()
            .map(|(_, c, _)| Arc::clone(c))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        self.start_chat_task_with_cancel(req_id, prompt, page, cancel);
    }

    pub(super) fn start_chat_task_with_cancel(
        &mut self,
        req_id: u64,
        prompt: String,
        page: Option<PageContext>,
        cancel: Arc<AtomicBool>,
    ) {
        let config = self.chat_config.clone();
        let history_len = self.chat_messages.len().saturating_sub(1);
        let history = self.chat_messages[..history_len].to_vec();
        let agent_ctx = self
            .agent_run
            .as_mut()
            .filter(|r| r.req_id == req_id)
            .map(|r| AgentContext {
                step: r.step,
                max_steps: r.max_steps,
                action_log: r.action_log.clone(),
                notes: r.notes.clone(),
                // A screenshot is shown to the model once, on the round right after it was taken.
                image_png_b64: r.screenshot.take(),
            });
        let model_id = if agent_ctx.is_some() {
            config.effective_control_model(config.provider)
        } else {
            config.selected_model(config.provider)
        };
        let model_lbl = display_model_label(config.provider, &model_id, &self.agy_models);
        let provider_label = if model_lbl == "기본 모델" {
            config.provider.short_label().to_string()
        } else {
            format!("{} · {model_lbl}", config.provider.short_label())
        };
        let bus = self.bus.clone();
        let cancel_task = Arc::clone(&cancel);

        // Plain (non-agent) API chats stream SSE deltas straight to the UI as they
        // arrive, so the first words paint long before the full reply is done.
        // CLI agents keep their request/response workers; agent-control steps keep
        // buffered per-step JSON (partial control blocks must never hit the UI).
        // The API path never invokes a CLI binary: pure HTTPS + JSON throughout.
        let stream_cb: Option<Arc<dyn Fn(String) + Send + Sync>> = if agent_ctx.is_none() && !config.provider.is_cli() {
            let bus_cb = self.bus.clone();
            let label_cb = provider_label.clone();
            Some(Arc::new(move |delta: String| {
                bus_cb.send(AppEvent::ChatStreamChunk {
                    req_id,
                    provider_label: label_cb.clone(),
                    delta,
                });
            }))
        } else {
            None
        };

        self.begin_live(req_id);
        let bus_p = self.bus.clone();
        let progress: crate::chat::ProgressCb =
            Arc::new(move |progress| bus_p.send(AppEvent::ChatProgress { req_id, progress }));
        let handle = tokio::spawn(async move {
            let result = run_chat(
                config,
                history,
                prompt,
                page,
                agent_ctx,
                cancel_task,
                stream_cb,
                Some(progress),
            )
            .await;
            bus.send(AppEvent::ChatFinished {
                req_id,
                provider_label,
                result,
            });
        });

        self.chat_inflight = Some((req_id, cancel, handle.abort_handle()));
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    pub(in crate::app) fn on_chat_stream_chunk(&mut self, req_id: u64, provider_label: String, delta: String) {
        if delta.is_empty() {
            return;
        }
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        if self.agent_run.as_ref().is_some_and(|r| r.req_id == req_id) {
            return;
        }
        let continues = self.chat_stream_req == Some(req_id)
            && self
                .chat_messages
                .last()
                .is_some_and(|m| m.role == ChatRole::Assistant && m.provider_label.as_deref() == Some(&provider_label));
        if continues {
            if let Some(last) = self.chat_messages.last_mut() {
                last.text.push_str(&delta);
            }
        } else {
            self.chat_messages.push(ChatMessage {
                role: ChatRole::Assistant,
                text: delta,
                page_title: None,
                provider_label: Some(provider_label),
                activity: Vec::new(),
                elapsed_ms: None,
            });
            self.chat_stream_req = Some(req_id);
        }
        if let Some(live) = self.chat_live.as_mut().filter(|l| l.req_id == req_id) {
            live.writing = true;
        }
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    pub(in crate::app) fn on_chat_finished(
        &mut self,
        req_id: u64,
        provider_label: String,
        result: Result<String, String>,
    ) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        if let Ok(raw_reply) = &result
            && self.agent_run.as_ref().is_some_and(|r| r.req_id == req_id)
        {
            let (cleaned, maybe_plan) = parse_agent_response(raw_reply);
            let Some(plan) = maybe_plan else {
                let log = self.agent_run.take().map(|r| r.action_log).unwrap_or_default();
                self.chat_inflight = None;
                let _ = log; // shown as activity cards instead (see `live`)
                let text = format_agent_final_message(&cleaned, "", &[]);
                self.chat_messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text,
                    page_title: None,
                    provider_label: Some(provider_label),
                    activity: Vec::new(),
                    elapsed_ms: None,
                });
                self.persist_chat();
                self.start_reveal(self.chat_messages.len() - 1);
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(true);
                return;
            };

            if !plan.note.is_empty()
                && let Some(run) = self.agent_run.as_mut()
            {
                if run.notes.len() >= 10 {
                    run.notes.remove(0);
                }
                run.notes.push(plan.note);
            }

            if plan.actions.is_empty() {
                if let Some(run) = self.agent_run.as_mut() {
                    run.finish_cleaned = cleaned;
                    run.finish_summary = plan.summary;
                    run.provider_label = provider_label;
                }
                self.finish_agent_run(None);
                return;
            }

            // Duplicate-action loop guard: stop instead of repeating the same step forever.
            let is_duplicate = self.agent_run.as_ref().is_some_and(|run| {
                plan.actions.len() == 1 && !plan.finish_after && run.last_action.as_ref() == Some(&plan.actions[0])
            });
            if is_duplicate {
                if let Some(run) = self.agent_run.as_mut() {
                    run.finish_cleaned = cleaned;
                    run.provider_label = provider_label;
                }
                self.finish_agent_run(Some("동일한 동작이 반복되어 중단했습니다.".to_string()));
                return;
            }

            let reached_cap = self.agent_run.as_ref().is_some_and(|r| r.step >= r.max_steps);
            if reached_cap {
                if let Some(run) = self.agent_run.as_mut() {
                    run.finish_cleaned = cleaned;
                    run.provider_label = provider_label;
                }
                self.finish_agent_run(Some("최대 실행 라운드에 도달했습니다.".to_string()));
                return;
            }

            if let Some(run) = self.agent_run.as_mut() {
                run.step += 1;
                run.pending_actions = plan.actions.into();
                run.finish_after_queue = plan.finish_after || run.step >= run.max_steps;
                run.finish_cleaned = cleaned;
                run.finish_summary = plan.summary;
                run.provider_label = provider_label;
            }
            self.run_next_queued_agent_action(req_id);
            return;
        }

        self.agent_run = None;
        self.chat_inflight = None;
        match result {
            Ok(text) => {
                if self.chat_stream_req == Some(req_id) {
                    // The streamed draft becomes the authoritative final text; it is
                    // already on screen, so no typing effect replays it.
                    if let Some(last) = self.chat_messages.last_mut()
                        && last.role == ChatRole::Assistant
                    {
                        last.text = text;
                    } else {
                        self.chat_messages.push(ChatMessage {
                            role: ChatRole::Assistant,
                            text,
                            page_title: None,
                            provider_label: Some(provider_label),
                            activity: Vec::new(),
                            elapsed_ms: None,
                        });
                    }
                    self.chat_stream_req = None;
                } else {
                    self.chat_messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        text,
                        page_title: None,
                        provider_label: Some(provider_label),
                        activity: Vec::new(),
                        elapsed_ms: None,
                    });
                    self.start_reveal(self.chat_messages.len() - 1);
                }
            }
            Err(err) => {
                // Any streamed partial text stays visible above the error note.
                if self.chat_stream_req == Some(req_id) {
                    self.chat_stream_req = None;
                }
                self.chat_messages.push(ChatMessage {
                    role: ChatRole::Error,
                    text: err,
                    page_title: None,
                    provider_label: Some(provider_label),
                    activity: Vec::new(),
                    elapsed_ms: None,
                });
            }
        }
        self.persist_chat();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }
}
