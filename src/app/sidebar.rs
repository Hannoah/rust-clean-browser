//! Docked right-hand AI Assistant sidebar: lifecycle, resizing, prompt submission,
//! active webpage context extraction (Clean & JS modes), and clipboard copy.

use super::bus::SidebarEventHandler;
use super::{AppEvent, BrowserApp, UiRegion};
use crate::chat::{
    AGENT_SNAPSHOT_JS, AgentAction, AgentContext, AgentPreset, ChatHistory, ChatMessage, ChatProvider, ChatRole,
    ModelOption, PageContext, build_agent_action_js, build_agent_extract_js, build_agent_text_check_js,
    build_page_context_from_text, display_model_label, extract_page_context_from_html, fetch_agy_models,
    format_agent_snapshot, is_loopback_endpoint, open_cli_login_terminal, parse_agent_response, preset_models,
    run_chat,
};
use crate::chrome_ui::{SidebarPicker, SidebarRenderState, render_sidebar_html};
use crate::memory::trim_process_working_set;
use blitz_dom::{EventDriver, ScrollBehavior};
use blitz_html::HtmlDocument;
use blitz_traits::events::UiEvent;
use blitz_traits::shell::Viewport;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;

pub(super) const MIN_SIDEBAR_WIDTH: u32 = 280;
pub(super) const MAX_SIDEBAR_WIDTH: u32 = 960;
pub(super) const MIN_CHAT_FONT: u32 = 11;
pub(super) const MAX_CHAT_FONT: u32 = 20;
/// Typing-effect frame interval and frame budget (a reply finishes revealing in ~0.7 s).
const REVEAL_TICK_MS: u64 = 30;
const REVEAL_FRAMES: usize = 22;
const MAX_INPUT_LINES: usize = 6;

/// An agent navigation to a site the user hasn't opened or named, waiting for 허용 / 거부.
#[derive(Debug, Clone)]
pub(in crate::app) struct PendingConfirm {
    pub action: AgentAction,
    pub desc: String,
    pub host: String,
    pub is_final: bool,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct AgentLoadWait {
    pub tab_id: u64,
    pub wait_gen: u64,
    pub action_summary: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct AgentTextWait {
    pub text: String,
    pub wait_gen: u64,
    pub deadline: Instant,
    pub action_summary: String,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct AgentRun {
    pub req_id: u64,
    pub goal: String,
    pub step: usize,
    pub max_steps: usize,
    pub action_log: Vec<String>,
    pub notes: Vec<String>,
    pub status: String,
    pub pending_actions: VecDeque<AgentAction>,
    pub finish_after_queue: bool,
    pub finish_cleaned: String,
    pub finish_summary: String,
    pub provider_label: String,
    pub last_action: Option<AgentAction>,
    pub consecutive_errors: usize,
    pub awaiting_load: Option<AgentLoadWait>,
    pub awaiting_text: Option<AgentTextWait>,
    pub load_wait_seq: u64,
    pub extracted_data: Option<String>,
    /// Hosts the user approved during this run.
    pub trusted_hosts: Vec<String>,
    pub awaiting_confirm: Option<PendingConfirm>,
}

impl BrowserApp {
    pub(super) fn sidebar_logical_width(&self) -> u32 {
        if !self.sidebar_open {
            return 0;
        }
        let scale = self.hidpi_scale.max(1.0);
        let win_logical_w = ((self.win_size.width as f32) / scale).round() as u32;
        let max_allowed = win_logical_w.saturating_sub(240).max(MIN_SIDEBAR_WIDTH);
        self.prefs
            .sidebar_width
            .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH.min(max_allowed))
    }

    pub(super) fn sidebar_phys_width(&self) -> u32 {
        if !self.sidebar_open {
            return 0;
        }
        ((self.sidebar_logical_width() as f32) * self.hidpi_scale)
            .round()
            .max(1.0) as u32
    }

    pub(super) fn sidebar_left_logical(&self) -> f32 {
        let scale = self.hidpi_scale.max(1.0);
        let win_w = (self.win_size.width as f32) / scale;
        (win_w - self.sidebar_logical_width() as f32).max(0.0)
    }

    pub(super) fn sidebar_viewport(&self) -> Viewport {
        let (_, ch) = self.content_phys_size();
        Viewport::new(
            self.sidebar_phys_width().max(1),
            ch.max(1),
            self.hidpi_scale,
            self.color_scheme,
        )
    }

    pub(super) fn sidebar_input_composing(&self) -> bool {
        let Some(doc) = &self.sidebar_doc else {
            return false;
        };
        Self::is_input_composing(doc, "chat-input")
            || Self::is_input_composing(doc, "chat-api-endpoint")
            || Self::is_input_composing(doc, "chat-api-model")
            || Self::is_input_composing(doc, "chat-api-key")
    }

    pub(super) fn dispatch_sidebar_ui(&mut self, event: UiEvent) {
        let Some(doc) = &mut self.sidebar_doc else {
            return;
        };
        let handler = SidebarEventHandler { bus: self.bus.clone() };
        EventDriver::new(doc, handler).handle_ui_event(event);
    }

    pub(super) fn read_sidebar_input(&self, id: &str) -> Option<String> {
        let doc = self.sidebar_doc.as_ref()?;
        let nid = doc.get_element_by_id(id)?;
        let ti = doc.get_node(nid)?.element_data()?.text_input_data()?;
        Some(ti.editor.raw_text().to_string())
    }

    pub(super) fn is_sidebar_input_focused(&self, id: &str) -> bool {
        if self.focus_region != UiRegion::Sidebar {
            return false;
        }
        let Some(doc) = &self.sidebar_doc else {
            return false;
        };
        doc.get_element_by_id(id)
            .is_some_and(|nid| doc.get_focussed_node_id() == Some(nid))
    }

    /// Focuses a sidebar input on user request (also warms the CLI agent for the chat box).
    pub(super) fn focus_sidebar_input(&mut self, id: &str) {
        self.focus_sidebar_input_inner(id, true);
    }

    /// `warm == false` when merely restoring focus after a rebuild: warming there re-spawned the
    /// CLI process (busy cursor) every time the page title changed after the idle reaper ran.
    fn focus_sidebar_input_inner(&mut self, id: &str, warm: bool) {
        if let Some(doc) = &mut self.sidebar_doc
            && let Some(nid) = doc.get_element_by_id(id)
        {
            doc.set_focus_to(nid);
            self.focus_region = UiRegion::Sidebar;
            self.chrome_doc.clear_focus();
            if let Some(cdoc) = &mut self.tabs[self.active_tab].doc {
                cdoc.clear_focus();
            }
            if let Some(win) = &self.window {
                super::helpers::focus_main_window(&**win, true);
                super::bus::enable_window_ime(&**win);
            }
            if warm && id == "chat-input" && self.sidebar_open {
                self.warm_cli_for_current_mode();
            }
            self.request_redraw();
        }
    }

    fn maybe_fetch_agy_models(&mut self, force: bool) {
        if self.agy_models_loading || (!force && self.agy_models_fetched) || !self.chat_cli_avail.agy {
            return;
        }
        self.agy_models_loading = true;
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let models = fetch_agy_models().await;
            bus.send(AppEvent::AgyModelsLoaded(models));
        });
        if self.sidebar_open {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }

    pub(super) fn on_agy_models_loaded(&mut self, models: Vec<ModelOption>) {
        self.agy_models_loading = false;
        self.agy_models_fetched = true;
        if !models.is_empty() {
            self.agy_models = models;
        }
        if self.sidebar_open {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }

    fn current_model_options(&self) -> Vec<ModelOption> {
        preset_models(self.chat_config.provider, &self.agy_models)
    }

    pub(super) fn warm_cli_for_current_mode(&self) {
        let fast = self.chat_config.agent_control;
        match self.chat_config.provider {
            ChatProvider::Agy if self.chat_cli_avail.agy => {
                let model = if fast {
                    self.chat_config.effective_control_model(ChatProvider::Agy)
                } else {
                    self.chat_config.selected_model(ChatProvider::Agy)
                };
                crate::chat::warm_agy_worker(model, fast);
            }
            ChatProvider::Claude if self.chat_cli_avail.claude => {
                let model = if fast {
                    self.chat_config.effective_control_model(ChatProvider::Claude)
                } else {
                    self.chat_config.selected_model(ChatProvider::Claude)
                };
                crate::chat::warm_claude_worker(model, fast);
            }
            _ => {}
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        if self.sidebar_open {
            self.sidebar_opened_at = Some(Instant::now());
            self.chat_cli_avail = crate::chat::CliAvailability::detect();
            if self.chat_history.is_none() {
                self.chat_history = Some(ChatHistory::load());
            }
            if self.chat_config.provider == ChatProvider::Agy {
                self.maybe_fetch_agy_models(false);
            }
            self.warm_cli_for_current_mode();
            self.sync_viewports();
            self.rebuild_sidebar(true);
            self.focus_sidebar_input("chat-input");
        } else {
            self.sidebar_drag = false;
            self.sidebar_picker = SidebarPicker::None;
            self.sidebar_doc = None;
            self.last_sidebar_html.clear();
            self.sidebar_code_blocks.clear();
            if self.focus_region == UiRegion::Sidebar {
                self.focus_region = UiRegion::Content;
            }
            if self.pointer_region == Some(UiRegion::Sidebar) {
                self.pointer_region = None;
            }
            self.sync_viewports();
            if self.chat_inflight.is_none() {
                crate::chat::reset_all_cli_workers();
            }
            trim_process_working_set();
        }
        self.rebuild_chrome();
    }

    pub(super) fn open_sidebar_and_summarize(&mut self) {
        if !self.sidebar_open {
            self.toggle_sidebar();
        }
        self.handle_sidebar_action("chat-quick:summary");
    }

    pub(super) fn rebuild_sidebar(&mut self, scroll_to_bottom: bool) {
        if !self.sidebar_open {
            return;
        }
        if self.sidebar_input_composing() {
            return;
        }
        self.ensure_active_valid();
        let motion = self.sidebar_motion();
        let was_chat_focused = self.is_sidebar_input_focused("chat-input");
        let draft = self.read_sidebar_input("chat-input").unwrap_or_default();
        // After a provider/model chip click the inputs must show the new choice, not the old draft
        // (which 저장/적용 would otherwise write back over the selection).
        let reset_drafts = std::mem::take(&mut self.sidebar_reset_drafts);
        let endpoint_draft = self.read_sidebar_input("chat-api-endpoint").filter(|_| !reset_drafts);
        let model_draft = self.read_sidebar_input("chat-api-model").filter(|_| !reset_drafts);
        let active = &self.tabs[self.active_tab];
        let model_options = self.current_model_options();
        let active_model_id = self.chat_config.selected_model(self.chat_config.provider);
        let active_model_label = display_model_label(self.chat_config.provider, &active_model_id, &self.agy_models);
        let api_endpoint = endpoint_draft
            .as_deref()
            .unwrap_or_else(|| self.chat_config.effective_endpoint());
        let api_model = model_draft.as_deref().unwrap_or(&active_model_id);
        let can_retry = self.chat_inflight.is_none()
            && self.chat_messages.last().is_some_and(|m| m.role == ChatRole::Error)
            && self.chat_messages.iter().any(|m| m.role == ChatRole::User);

        let (html, code_blocks) = render_sidebar_html(&SidebarRenderState {
            provider: self.chat_config.provider,
            avail: self.chat_cli_avail,
            picker_open: self.sidebar_picker,
            active_model_id: &active_model_id,
            active_model_label: &active_model_label,
            control_model_id: &self.chat_config.selected_control_model(self.chat_config.provider),
            custom_presets: &self.chat_config.custom_presets,
            model_options: &model_options,
            agy_models_loading: self.agy_models_loading,
            messages: &self.chat_messages,
            is_loading: self.chat_inflight.is_some(),
            can_retry,
            include_page: self.chat_config.include_page,
            agent_control: self.chat_config.agent_control,
            agent_status: self.agent_run.as_ref().map(|r| r.status.as_str()),
            agent_log: self.agent_run.as_ref().map_or(&[], |r| r.action_log.as_slice()),
            sidebar_wide: self.sidebar_logical_width() >= 520,
            show_settings: self.chat_show_settings,
            active_title: &active.title,
            active_url: &active.url,
            api_endpoint,
            api_model,
            has_api_key: !self.chat_config.api_key.trim().is_empty(),
            draft: &draft,
            font_size: self.prefs.chat_font_size.clamp(MIN_CHAT_FONT, MAX_CHAT_FONT),
            reveal: self.chat_reveal,
            show_history: self.chat_show_history,
            history: self.chat_history.as_ref().map_or(&[], |h| h.sessions.as_slice()),
            session_id: self.chat_session_id,
            agent_confirm: self
                .agent_run
                .as_ref()
                .and_then(|r| r.awaiting_confirm.as_ref())
                .map(|c| (c.desc.as_str(), c.host.as_str())),
            motion,
        });

        self.sidebar_code_blocks = code_blocks;
        if html == self.last_sidebar_html && self.sidebar_doc.is_some() {
            if scroll_to_bottom {
                self.scroll_sidebar_to_bottom();
            }
            return;
        }

        let scroll_of = |id: &str| {
            self.sidebar_doc
                .as_ref()
                .and_then(|d| d.get_element_by_id(id).and_then(|nid| d.get_node(nid)))
                .map_or(0.0, |n| n.scroll_offset().y)
        };
        let prev_scroll = scroll_of("chat-messages");
        let prev_settings_scroll = scroll_of("chat-settings");

        let mut doc = HtmlDocument::from_html(
            &html,
            self.doc_config(self.sidebar_viewport(), self.shell_provider.clone()),
        );
        doc.resolve(self.anim_time());
        if let Some(msg_id) = doc.get_element_by_id("chat-messages") {
            if scroll_to_bottom {
                doc.scroll_to(msg_id, 0.0, 1.0e9, ScrollBehavior::Instant);
            } else if prev_scroll > 0.0 {
                doc.scroll_to(msg_id, 0.0, prev_scroll, ScrollBehavior::Instant);
            }
        }
        if prev_settings_scroll > 0.0
            && let Some(set_id) = doc.get_element_by_id("chat-settings")
        {
            doc.scroll_to(set_id, 0.0, prev_settings_scroll, ScrollBehavior::Instant);
        }
        self.sidebar_doc = Some(doc);
        self.last_sidebar_html = html;
        if was_chat_focused {
            self.focus_sidebar_input_inner("chat-input", false);
            if !draft.is_empty()
                && let Some(doc) = &mut self.sidebar_doc
                && let Some(id) = doc.get_element_by_id("chat-input")
            {
                doc.with_text_input(id, |mut driver| driver.move_to_text_end());
            }
        }
        self.request_redraw();
    }

    /// Notices newly added messages / newly opened panels and returns their animation ages.
    fn sidebar_motion(&mut self) -> crate::chrome_ui::SidebarMotion {
        let now = Instant::now();
        let len = self.chat_messages.len();
        if len > self.chat_seen_len {
            self.chat_msg_added = Some((len - 1, now));
        } else if len < self.chat_seen_len {
            self.chat_msg_added = None;
        }
        self.chat_seen_len = len;
        let picker = match self.sidebar_picker {
            SidebarPicker::None => 0,
            SidebarPicker::Provider => 4,
            SidebarPicker::Model => 8,
        };
        let panel_key = u8::from(self.chat_show_settings) | (u8::from(self.chat_show_history) << 1) | picker;
        if panel_key != self.sidebar_panel_key {
            self.sidebar_panel_key = panel_key;
            self.sidebar_panel_at = (panel_key != 0).then_some(now);
        }
        let age = |t: Instant| now.saturating_duration_since(t).as_millis() as u64;
        crate::chrome_ui::SidebarMotion {
            msg: self.chat_msg_added.map(|(i, t)| (i, age(t))),
            panel_ms: self.sidebar_panel_at.map(age),
            shell_ms: self.sidebar_opened_at.map(age),
        }
    }

    fn scroll_sidebar_to_bottom(&mut self) {
        let anim_time = self.anim_time();
        if let Some(doc) = &mut self.sidebar_doc
            && let Some(msg_id) = doc.get_element_by_id("chat-messages")
        {
            doc.resolve(anim_time);
            doc.scroll_to(msg_id, 0.0, 1.0e9, ScrollBehavior::Instant);
            self.request_redraw();
        }
    }

    pub(super) fn is_sidebar_resize_edge(&self, pos: PhysicalPosition<f64>) -> bool {
        if !self.sidebar_open {
            return false;
        }
        let scale = self.hidpi_scale.max(1.0) as f64;
        let lx = pos.x / scale;
        let ly = pos.y / scale;
        if ly < self.chrome_height() as f64 {
            return false;
        }
        let left = self.sidebar_left_logical() as f64;
        lx >= left - 5.0 && lx <= left + 8.0
    }

    pub(super) fn update_sidebar_drag(&mut self, pos: PhysicalPosition<f64>) -> bool {
        if !self.sidebar_drag {
            return false;
        }
        let scale = self.hidpi_scale.max(1.0) as f64;
        let win_w = self.win_size.width as f64 / scale;
        let lx = pos.x / scale;
        let max_allowed = (win_w as u32).saturating_sub(240).max(MIN_SIDEBAR_WIDTH);
        let next_w = ((win_w - lx).round() as u32).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH.min(max_allowed));
        if next_w != self.prefs.sidebar_width {
            self.prefs.sidebar_width = next_w;
            self.sync_viewports();
        }
        true
    }

    pub(super) fn handle_sidebar_action(&mut self, action: &str) {
        if let Some(pid) = action.strip_prefix("chat-provider:") {
            if let Some(provider) = ChatProvider::from_id(pid) {
                let prev_provider = self.chat_config.provider;
                self.sidebar_picker = SidebarPicker::None;
                self.chat_config.provider = provider;
                if !provider.is_cli() {
                    let cur_ep = self.chat_config.api_endpoint.trim();
                    let matches_other_default = ChatProvider::ALL
                        .iter()
                        .filter(|p| **p != provider && !p.is_cli())
                        .any(|p| p.default_endpoint() == cur_ep);
                    if prev_provider != provider && (cur_ep.is_empty() || matches_other_default) {
                        self.chat_config.api_endpoint = provider.default_endpoint().to_string();
                    }
                    let local =
                        url::Url::parse(self.chat_config.effective_endpoint()).is_ok_and(|u| is_loopback_endpoint(&u));
                    if self.chat_config.api_key.trim().is_empty() && !local {
                        self.chat_show_settings = true;
                    }
                }
                if provider == ChatProvider::Agy {
                    self.maybe_fetch_agy_models(false);
                } else {
                    crate::chat::reset_agy_worker();
                }
                if provider != ChatProvider::Claude {
                    crate::chat::reset_claude_worker();
                }
                self.warm_cli_for_current_mode();
                self.chat_config.save();
                self.sidebar_reset_drafts = true;
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(false);
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
            self.rebuild_sidebar(false);
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
            self.rebuild_sidebar(false);
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
                self.rebuild_sidebar(false);
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
            self.rebuild_sidebar(false);
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
                self.rebuild_sidebar(false);
            }
            "chat-picker:model" => {
                self.sidebar_picker = if self.sidebar_picker == SidebarPicker::Model {
                    SidebarPicker::None
                } else {
                    self.chat_show_settings = false;
                    if self.chat_config.provider == ChatProvider::Agy {
                        self.maybe_fetch_agy_models(false);
                    }
                    SidebarPicker::Model
                };
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(false);
            }
            "chat-refresh-models" => self.maybe_fetch_agy_models(true),
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
                self.chat_reveal = None;
                self.chat_show_history = false;
                crate::chat::reset_all_cli_workers();
                if self.sidebar_open {
                    self.warm_cli_for_current_mode();
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(false);
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
                self.rebuild_sidebar(false);
            }
            "chat-del-all" => self.delete_chat_session(None),
            "chat-regen" => self.regenerate_last_reply(),
            "chat-toggle-settings" => {
                self.chat_show_settings = !self.chat_show_settings;
                if self.chat_show_settings {
                    self.chat_show_history = false;
                    self.sidebar_picker = SidebarPicker::None;
                    if self.chat_config.provider == ChatProvider::Agy {
                        self.maybe_fetch_agy_models(false);
                    }
                }
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(false);
            }
            "chat-save-settings" => self.save_sidebar_settings(),
            "chat-preset-add" => self.add_sidebar_preset(),
            "chat-clear-key" => {
                self.chat_config.api_key.clear();
                self.chat_config.save();
                self.set_status_note("API 키 삭제됨");
                self.last_sidebar_html.clear();
                self.rebuild_chrome();
                self.rebuild_sidebar(false);
            }
            "chat-toggle-page" => {
                self.chat_config.include_page = !self.chat_config.include_page;
                self.chat_config.save();
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(false);
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
                self.rebuild_sidebar(false);
            }
            "chat-toggle-width" => {
                let target = if self.sidebar_logical_width() >= 520 { 380 } else { 600 };
                self.prefs.sidebar_width = target;
                self.prefs.save();
                self.sync_viewports();
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(false);
            }
            "chat-send" => self.submit_from_sidebar_input(),
            "chat-stop" => self.cancel_chat_inflight(true),
            _ => {}
        }
    }

    pub(super) fn save_sidebar_settings(&mut self) {
        if let Some(ep) = self.read_sidebar_input("chat-api-endpoint") {
            self.chat_config.api_endpoint = ep.trim().to_string();
        }
        if let Some(model) = self.read_sidebar_input("chat-api-model") {
            self.chat_config
                .set_selected_model(self.chat_config.provider, model.trim().to_string());
        }
        if let Some(key) = self.read_sidebar_input("chat-api-key")
            && !key.trim().is_empty()
        {
            self.chat_config.api_key = key.trim().to_string();
        }
        self.chat_config.save();
        self.chat_show_settings = false;
        self.last_sidebar_html.clear();
        self.set_status_note("AI 설정 저장됨");
        self.rebuild_chrome();
        self.rebuild_sidebar(false);
    }

    pub(super) fn add_sidebar_preset(&mut self) {
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
        self.rebuild_sidebar(false);
    }

    pub(super) fn submit_from_sidebar_input(&mut self) {
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
        self.submit_chat_message(prompt, false, self.chat_config.agent_control);
    }

    pub(super) fn sidebar_has_selection(&self) -> bool {
        self.sidebar_doc.as_ref().is_some_and(|d| d.has_text_selection())
    }

    /// Ctrl+C on text dragged inside the conversation. Handled here rather than by Blitz, whose
    /// own copy is skipped while the chat box keeps focus.
    pub(super) fn copy_sidebar_selection(&mut self) {
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

    fn copy_to_clipboard(&mut self, text: &str) {
        let msg = match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
            Ok(()) => "클립보드에 복사됨",
            Err(_) => "클립보드 복사 실패",
        };
        self.set_status_note(msg);
        self.rebuild_chrome();
    }

    pub(super) fn cancel_chat_inflight(&mut self, record_note: bool) {
        self.agent_run = None;
        self.finish_reveal();
        if let Some((_, cancel, abort)) = self.chat_inflight.take() {
            cancel.store(true, Ordering::Release);
            abort.abort();
            if record_note {
                self.chat_messages.push(ChatMessage {
                    role: ChatRole::Error,
                    text: "생성이 중지되었습니다.".to_string(),
                    page_title: None,
                    provider_label: None,
                });
                self.persist_chat();
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(true);
            }
        }
    }

    // ── Local conversation history ──

    /// Saves the current conversation into `chat_history.json` (new session id on first save).
    fn persist_chat(&mut self) {
        if self.chat_messages.is_empty() {
            return;
        }
        let id = *self.chat_session_id.get_or_insert_with(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_millis() as u64)
        });
        let history = self.chat_history.get_or_insert_with(ChatHistory::load);
        history.upsert(id, &self.chat_messages);
        history.save();
    }

    fn load_chat_session(&mut self, id: u64) {
        let Some(session) = self.chat_history.get_or_insert_with(ChatHistory::load).get(id).cloned() else {
            return;
        };
        self.cancel_chat_inflight(false);
        self.chat_messages = session.messages;
        self.chat_session_id = Some(id);
        self.chat_show_history = false;
        // The persistent Claude worker holds the previous conversation: start fresh so the
        // loaded history is sent with the next turn.
        crate::chat::reset_all_cli_workers();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    /// Deletes one saved conversation, or all of them with `None`.
    fn delete_chat_session(&mut self, id: Option<u64>) {
        let history = self.chat_history.get_or_insert_with(ChatHistory::load);
        match id {
            Some(id) => history.remove(id),
            None => history.sessions.clear(),
        }
        history.save();
        if id.is_none() || id == self.chat_session_id {
            self.cancel_chat_inflight(false);
            self.chat_messages.clear();
            self.chat_session_id = None;
            crate::chat::reset_all_cli_workers();
        }
        self.set_status_note(if id.is_some() {
            "대화 삭제됨"
        } else {
            "대화 기록 전체 삭제됨"
        });
        self.rebuild_chrome();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(false);
    }

    /// Drops the last assistant reply and asks again.
    fn regenerate_last_reply(&mut self) {
        if self.chat_inflight.is_some() {
            return;
        }
        self.finish_reveal();
        if self.chat_messages.last().is_some_and(|m| m.role == ChatRole::Assistant) {
            self.chat_messages.pop();
        }
        self.retry_last_chat();
    }

    // ── Typing effect ──

    fn start_reveal(&mut self, msg_idx: usize) {
        self.chat_reveal = Some((msg_idx, 0));
        self.schedule_reveal_tick();
    }

    fn schedule_reveal_tick(&self) {
        let bus = self.bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(REVEAL_TICK_MS)).await;
            bus.send(AppEvent::ChatRevealTick);
        });
    }

    pub(super) fn on_chat_reveal_tick(&mut self) {
        let Some((idx, shown)) = self.chat_reveal else {
            return;
        };
        let total = self.chat_messages.get(idx).map_or(0, |m| m.text.chars().count());
        let next = shown + (total / REVEAL_FRAMES).max(4);
        if next >= total {
            self.chat_reveal = None;
        } else {
            self.chat_reveal = Some((idx, next));
            self.schedule_reveal_tick();
        }
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    /// Shows the rest of a reply that is still typing out (new request, stop, history switch).
    fn finish_reveal(&mut self) {
        if self.chat_reveal.take().is_some() {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }

    /// Grows the chat textarea with its content (Shift+Enter lines), up to `MAX_INPUT_LINES`.
    pub(super) fn sync_chat_input_height(&mut self) {
        let Some(text) = self.read_sidebar_input("chat-input") else {
            return;
        };
        let lines = text.split('\n').count().clamp(1, MAX_INPUT_LINES);
        if lines == self.chat_input_lines {
            return;
        }
        self.chat_input_lines = lines;
        if let Some(doc) = &mut self.sidebar_doc
            && let Some(id) = doc.get_element_by_id("chat-input")
        {
            doc.set_style_property(
                id,
                "height",
                &format!("{}px", crate::chrome_ui::chat_input_height(lines)),
            );
            self.request_redraw();
        }
    }

    pub(super) fn submit_chat_message(&mut self, user_text: String, force_page: bool, agent: bool) {
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
        });
        self.persist_chat();

        self.dispatch_chat_for_last_user(user_text, use_page, agent);
    }

    fn retry_last_chat(&mut self) {
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

    fn dispatch_chat_for_last_user(&mut self, user_text: String, use_page: bool, agent: bool) {
        self.ensure_active_valid();
        let idx = self.active_tab;
        let active = &self.tabs[idx];
        let active_title = active.title.clone();
        let active_url = active.url.clone();

        self.chat_req_seq += 1;
        let req_id = self.chat_req_seq;
        let cancel = Arc::new(AtomicBool::new(false));

        if agent {
            self.agent_run = Some(AgentRun {
                req_id,
                goal: user_text,
                step: 0,
                max_steps: 10,
                action_log: Vec::new(),
                notes: Vec::new(),
                status: "화면 분석 중…".to_string(),
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
                trusted_hosts: Vec::new(),
                awaiting_confirm: None,
            });
            let dummy = tokio::spawn(async {});
            self.chat_inflight = Some((req_id, cancel, dummy.abort_handle()));
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(true);

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
            if self.active_webview.is_some() {
                let dummy = tokio::spawn(async {});
                let abort = dummy.abort_handle();
                self.chat_inflight = Some((req_id, cancel, abort));
                self.last_sidebar_html.clear();
                self.rebuild_sidebar(true);

                let bus = self.bus.clone();
                let prompt = user_text;
                let res = self.active_webview.as_ref().map(|wv| {
                    wv.evaluate_script_with_callback(
                        "(function(){try{return (document.body?document.body.innerText:'').slice(0,14000);}catch(e){return '';}})()",
                        move |json| {
                            let raw_text = serde_json::from_str::<String>(&json).unwrap_or_default();
                            let ctx = build_page_context_from_text(&active_title, &active_url, &raw_text);
                            bus.send(AppEvent::ChatPageContext {
                                req_id,
                                prompt: prompt.clone(),
                                page: Some(ctx),
                            });
                        },
                    )
                });
                if !matches!(res, Some(Ok(()))) {
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

    fn format_open_tabs_summary(&self) -> String {
        let mut s = String::from("[OPEN TABS]\n");
        for (i, t) in self.tabs.iter().enumerate() {
            let mark = if i == self.active_tab { "*" } else { " " };
            let title: String = t.title.chars().take(48).collect();
            let url: String = t.url.chars().take(90).collect();
            s.push_str(&format!("{mark}[{i}] \"{title}\" ({url})\n"));
        }
        s
    }

    fn arm_agent_load_wait(&mut self, req_id: u64, tab_id: u64, action_summary: Option<String>, timeout_ms: u64) {
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        run.load_wait_seq += 1;
        let wait_gen = run.load_wait_seq;
        run.awaiting_load = Some(AgentLoadWait {
            tab_id,
            wait_gen,
            action_summary,
        });
        let bus = self.bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            bus.send(AppEvent::AgentLoadSettled { req_id, wait_gen });
        });
    }

    pub(super) fn on_agent_webview_loaded(&mut self, tab_id: u64) {
        let Some(run) = self.agent_run.as_mut() else {
            return;
        };
        let Some(wait) = run.awaiting_load.as_mut() else {
            return;
        };
        if wait.tab_id != tab_id {
            return;
        }
        let req_id = run.req_id;
        run.load_wait_seq += 1;
        let settle_gen = run.load_wait_seq;
        wait.wait_gen = settle_gen;
        let bus = self.bus.clone();
        tokio::spawn(async move {
            // Brief settle after PageLoadEvent::Finished for hydration / paint.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            bus.send(AppEvent::AgentLoadSettled {
                req_id,
                wait_gen: settle_gen,
            });
        });
    }

    pub(super) fn on_agent_load_settled(&mut self, req_id: u64, wait_gen: u64) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        let matches_wait = self
            .agent_run
            .as_ref()
            .filter(|r| r.req_id == req_id)
            .and_then(|r| r.awaiting_load.as_ref())
            .is_some_and(|w| w.wait_gen == wait_gen);
        if !matches_wait {
            return;
        }
        let wait = self.agent_run.as_mut().and_then(|r| r.awaiting_load.take());
        match wait.and_then(|w| w.action_summary) {
            Some(summary) => self.on_agent_action_done(req_id, true, summary, 0),
            None => self.capture_agent_snapshot_and_run(req_id, false),
        }
    }

    pub(super) fn on_agent_extract_done(
        &mut self,
        req_id: u64,
        ok: bool,
        summary: String,
        extracted: String,
        copy: bool,
    ) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        if copy && ok && !extracted.is_empty() {
            self.copy_to_clipboard(&extracted);
        }
        if ok
            && !extracted.is_empty()
            && let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id)
        {
            run.extracted_data = Some(extracted);
        }
        self.on_agent_action_done(req_id, ok, summary, 0);
    }

    fn capture_agent_snapshot_and_run(&mut self, req_id: u64, allow_load_wait: bool) {
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        let goal = run.goal.clone();
        let extracted_opt = run.extracted_data.take();
        self.ensure_active_valid();
        let active_tab_id = self.tabs[self.active_tab].id;
        let active_loading = self.tabs[self.active_tab].is_loading;
        if allow_load_wait && self.active_webview.is_some() && active_loading {
            if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                run.extracted_data = extracted_opt;
                run.status = "페이지 로딩 대기 중…".to_string();
            }
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(true);
            self.arm_agent_load_wait(req_id, active_tab_id, None, 2800);
            return;
        }
        let tabs_summary = self.format_open_tabs_summary();
        let active = &self.tabs[self.active_tab];
        let active_title = active.title.clone();
        let active_url = active.url.clone();

        if let Some(wv) = &self.active_webview {
            let bus = self.bus.clone();
            let goal_c = goal.clone();
            let title_c = active_title.clone();
            let url_c = active_url.clone();
            let tabs_c = tabs_summary.clone();
            let ext_c = extracted_opt.clone();
            let res = wv.evaluate_script_with_callback(AGENT_SNAPSHOT_JS, move |json| {
                let mut ctx = format_agent_snapshot(&json, &title_c, &url_c);
                let mut prefix = tabs_c.clone();
                if let Some(ext) = &ext_c {
                    prefix.push_str("\n[EXTRACTED DATA]\n");
                    prefix.push_str(ext);
                    prefix.push('\n');
                }
                prefix.push('\n');
                prefix.push_str(&ctx.text);
                ctx.text = prefix;
                bus.send(AppEvent::ChatPageContext {
                    req_id,
                    prompt: goal_c.clone(),
                    page: Some(ctx),
                });
            });
            if res.is_err() {
                let mut text = format!(
                    "{tabs_summary}\n[INTERACTIVE ELEMENTS]\n(스냅샷 읽기 실패 — navigate/switch_tab/scroll 가능)"
                );
                if let Some(ext) = extracted_opt {
                    text.push_str("\n\n[EXTRACTED DATA]\n");
                    text.push_str(&ext);
                }
                let fallback = PageContext {
                    title: active_title,
                    url: active_url,
                    text,
                };
                self.start_chat_task(req_id, goal, Some(fallback));
            }
        } else {
            let mut text = format!(
                "{tabs_summary}\n[INTERACTIVE ELEMENTS]\n(내부 페이지 — navigate/open_tab/switch_tab 액션으로 웹페이지를 열거나 전환할 수 있습니다)"
            );
            if let Some(ext) = extracted_opt {
                text.push_str("\n\n[EXTRACTED DATA]\n");
                text.push_str(&ext);
            }
            let ctx = PageContext {
                title: active_title,
                url: active_url,
                text,
            };
            self.start_chat_task(req_id, goal, Some(ctx));
        }
    }

    pub(super) fn on_agent_step_ready(&mut self, req_id: u64) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        let Some((wait_gen, has_more)) = self.agent_run.as_ref().filter(|r| r.req_id == req_id).map(|r| {
            (
                r.awaiting_text.as_ref().map(|w| w.wait_gen),
                !r.pending_actions.is_empty(),
            )
        }) else {
            return;
        };
        if let Some(wgen) = wait_gen {
            self.poll_agent_text_wait(req_id, wgen);
        } else if has_more {
            self.run_next_queued_agent_action(req_id);
        } else {
            self.capture_agent_snapshot_and_run(req_id, true);
        }
    }

    fn poll_agent_text_wait(&mut self, req_id: u64, wait_gen: u64) {
        let Some(wait) = self
            .agent_run
            .as_ref()
            .filter(|r| r.req_id == req_id)
            .and_then(|r| r.awaiting_text.as_ref())
            .filter(|w| w.wait_gen == wait_gen)
            .cloned()
        else {
            return;
        };
        let Some(wv) = &self.active_webview else {
            if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                run.awaiting_text = None;
            }
            self.on_agent_action_done(
                req_id,
                false,
                format!("{} (WebView2가 필요합니다)", wait.action_summary),
                0,
            );
            return;
        };
        let bus = self.bus.clone();
        let js = build_agent_text_check_js(&wait.text);
        let res = wv.evaluate_script_with_callback(&js, move |raw| {
            let found = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| match v {
                    serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(&s).ok(),
                    other => Some(other),
                })
                .and_then(|v| v.get("found").and_then(|f| f.as_bool()))
                .unwrap_or(false);
            bus.send(AppEvent::AgentWaitCheck {
                req_id,
                wait_gen,
                found,
            });
        });
        if res.is_err() {
            if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                run.awaiting_text = None;
            }
            self.on_agent_action_done(req_id, false, format!("{} (스크립트 실패)", wait.action_summary), 0);
        }
    }

    pub(super) fn on_agent_wait_check(&mut self, req_id: u64, wait_gen: u64, found: bool) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        let matches = self
            .agent_run
            .as_ref()
            .filter(|r| r.req_id == req_id)
            .and_then(|r| r.awaiting_text.as_ref())
            .is_some_and(|w| w.wait_gen == wait_gen);
        if !matches {
            return;
        }
        if found {
            let summary = self
                .agent_run
                .as_mut()
                .and_then(|r| r.awaiting_text.take())
                .map(|w| format!("{} (확인됨)", w.action_summary))
                .unwrap_or_default();
            self.on_agent_action_done(req_id, true, summary, 0);
            return;
        }
        let expired = self
            .agent_run
            .as_ref()
            .and_then(|r| r.awaiting_text.as_ref())
            .is_some_and(|w| Instant::now() >= w.deadline);
        if expired {
            let summary = self
                .agent_run
                .as_mut()
                .and_then(|r| r.awaiting_text.take())
                .map(|w| format!("{} (시간 초과)", w.action_summary))
                .unwrap_or_default();
            self.on_agent_action_done(req_id, false, summary, 0);
            return;
        }
        let bus = self.bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            bus.send(AppEvent::AgentStepReady { req_id });
        });
    }

    pub(super) fn on_agent_action_done(&mut self, req_id: u64, ok: bool, summary: String, need_delay_ms: u64) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        let status_prefix = if ok { "OK" } else { "ERR" };
        run.action_log.push(format!("{status_prefix}: {summary}"));
        if ok {
            run.consecutive_errors = 0;
        } else {
            run.consecutive_errors += 1;
            // The rest of the batch was planned before this failure: re-plan from a fresh snapshot.
            run.pending_actions.clear();
            run.finish_after_queue = false;
        }

        // Stop early on repeated DOM action errors instead of looping.
        if !ok && run.consecutive_errors >= 2 {
            let err_note = format!("연속 동작 실패로 중단했습니다: {summary}");
            self.finish_agent_run(Some(err_note));
            return;
        }

        if ok && !run.pending_actions.is_empty() {
            let queued_next = run
                .pending_actions
                .front()
                .map(AgentAction::describe)
                .unwrap_or_default();
            run.status = format!("{}단계 실행 · {queued_next}", run.step);
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(true);
            if need_delay_ms == 0 {
                self.run_next_queued_agent_action(req_id);
            } else {
                let bus = self.bus.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(need_delay_ms)).await;
                    bus.send(AppEvent::AgentStepReady { req_id });
                });
            }
            return;
        }

        if run.pending_actions.is_empty() && (run.finish_after_queue || run.step >= run.max_steps) {
            let fallback_note = if !run.finish_after_queue && run.step >= run.max_steps {
                Some(format!("최대 실행 라운드({}단계)에 도달했습니다.", run.max_steps))
            } else {
                None
            };
            self.finish_agent_run(fallback_note);
            return;
        }

        run.status = format!("{}/{}단계 · 다음 동작 판단 중…", run.step, run.max_steps);
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);

        if need_delay_ms == 0 {
            self.capture_agent_snapshot_and_run(req_id, true);
        } else {
            let bus = self.bus.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(need_delay_ms)).await;
                bus.send(AppEvent::AgentStepReady { req_id });
            });
        }
    }

    fn finish_agent_run(&mut self, override_summary: Option<String>) {
        let Some(run) = self.agent_run.take() else {
            self.chat_inflight = None;
            return;
        };
        self.chat_inflight = None;
        let summary = override_summary.unwrap_or(run.finish_summary);
        let text = format_agent_final_message(&run.finish_cleaned, &summary, &run.action_log);
        let provider_label = (!run.provider_label.is_empty()).then_some(run.provider_label);
        self.chat_messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text,
            page_title: None,
            provider_label,
        });
        self.persist_chat();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    pub(super) fn start_chat_task(&mut self, req_id: u64, prompt: String, page: Option<PageContext>) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        let cancel = self
            .chat_inflight
            .as_ref()
            .map(|(_, c, _)| Arc::clone(c))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        self.start_chat_task_with_cancel(req_id, prompt, page, cancel);
    }

    fn start_chat_task_with_cancel(
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
            .as_ref()
            .filter(|r| r.req_id == req_id)
            .map(|r| AgentContext {
                step: r.step,
                max_steps: r.max_steps,
                action_log: r.action_log.clone(),
                notes: r.notes.clone(),
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

        let handle = tokio::spawn(async move {
            let result = run_chat(config, history, prompt, page, agent_ctx, cancel_task).await;
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

    pub(super) fn on_chat_finished(&mut self, req_id: u64, provider_label: String, result: Result<String, String>) {
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
                let text = format_agent_final_message(&cleaned, "", &log);
                self.chat_messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text,
                    page_title: None,
                    provider_label: Some(provider_label),
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
                self.chat_messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text,
                    page_title: None,
                    provider_label: Some(provider_label),
                });
                self.start_reveal(self.chat_messages.len() - 1);
            }
            Err(err) => self.chat_messages.push(ChatMessage {
                role: ChatRole::Error,
                text: err,
                page_title: None,
                provider_label: Some(provider_label),
            }),
        }
        self.persist_chat();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    fn run_next_queued_agent_action(&mut self, req_id: u64) {
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        let Some(action) = run.pending_actions.pop_front() else {
            return;
        };
        run.last_action = Some(action.clone());
        let desc = action.describe();
        let is_final = run.pending_actions.is_empty() && run.finish_after_queue;
        let has_more_queued = !run.pending_actions.is_empty();
        run.status = format!("{}단계 실행 · {desc}", run.step);
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
        self.execute_agent_action(req_id, action, desc, is_final, has_more_queued);
    }

    /// Hosts the agent may open without asking: any open tab's site, a site the user named in the
    /// request, or one already approved in this run. Anything else could be page-injected
    /// exfiltration (`https://attacker/?d=<other tab's text>`), so the user decides.
    fn agent_host_trusted(&self, req_id: u64, host: &str) -> bool {
        let Some(run) = self.agent_run.as_ref().filter(|r| r.req_id == req_id) else {
            return false;
        };
        let site = |h: &str| h.trim_start_matches("www.").to_string();
        let want = site(host);
        run.trusted_hosts.iter().any(|h| site(h) == want)
            || run.goal.to_ascii_lowercase().contains(&want)
            || self.tabs.iter().any(|t| {
                url::Url::parse(&t.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| site(&h.to_ascii_lowercase())))
                    .is_some_and(|h| h == want)
            })
    }

    fn ask_agent_confirm(
        &mut self,
        req_id: u64,
        action: AgentAction,
        desc: String,
        host: String,
        is_final: bool,
        has_more: bool,
    ) {
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        run.status = format!("확인 필요 · {host} 이동");
        run.awaiting_confirm = Some(PendingConfirm {
            action,
            desc,
            host,
            is_final,
            has_more,
        });
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    fn resolve_agent_confirm(&mut self, allow: bool) {
        let Some((req_id, pending)) = self
            .agent_run
            .as_mut()
            .and_then(|r| r.awaiting_confirm.take().map(|p| (r.req_id, p)))
        else {
            return;
        };
        if !allow {
            self.finish_agent_run(Some(format!(
                "{} 사이트로 이동하려 해서 사용자가 중단했습니다.",
                pending.host
            )));
            return;
        }
        if let Some(run) = self.agent_run.as_mut() {
            run.trusted_hosts.push(pending.host);
        }
        self.execute_agent_action(req_id, pending.action, pending.desc, pending.is_final, pending.has_more);
    }

    fn execute_agent_action(
        &mut self,
        req_id: u64,
        action: AgentAction,
        desc: String,
        is_final: bool,
        has_more_queued: bool,
    ) {
        let bus = self.bus.clone();
        if let AgentAction::Navigate { url } | AgentAction::OpenTab { url } = &action {
            let target = url.trim();
            let Some(host) = url::Url::parse(target)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            else {
                self.on_agent_action_done(req_id, false, format!("차단된 URL ({target})"), 0);
                return;
            };
            if !self.agent_host_trusted(req_id, &host) {
                self.ask_agent_confirm(req_id, action.clone(), desc, host, is_final, has_more_queued);
                return;
            }
        }
        match &action {
            AgentAction::Navigate { url } => {
                let mut target = url.trim().to_string();
                if self.prefs.https_only && target.starts_with("http://") {
                    target = format!("https://{}", &target["http://".len()..]);
                }
                self.ensure_active_valid();
                let idx = self.active_tab;
                let tab_id = self.tabs[idx].id;
                // Keep Back working after agent navigation, like an address-bar navigation.
                if self.tabs[idx].url != target && !self.tabs[idx].url.starts_with("rust://") {
                    let entry = crate::tabs::HistoryEntry {
                        url: self.tabs[idx].url.clone(),
                        scroll_y: self.current_scroll_y(),
                        js_mode: self.tabs[idx].js_mode,
                        force_blitz: self.tabs[idx].force_blitz,
                    };
                    self.tabs[idx].history_back.push(entry);
                    self.tabs[idx].history_forward.clear();
                }
                self.tabs[idx].url = target;
                self.tabs[idx].force_blitz = false;
                self.mount_or_navigate_webview(idx);
                self.rebuild_chrome();
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    self.arm_agent_load_wait(req_id, tab_id, Some(desc), 3200);
                }
            }
            AgentAction::OpenTab { url } => {
                self.open_new_tab_with_js(url.trim(), true);
                let tab_id = self.tabs[self.active_tab].id;
                self.rebuild_chrome();
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    self.arm_agent_load_wait(req_id, tab_id, Some(desc), 3200);
                }
            }
            AgentAction::SwitchTab { tab } => {
                if *tab >= self.tabs.len() {
                    self.on_agent_action_done(req_id, false, format!("존재하지 않는 탭 인덱스 [{tab}]"), 0);
                    return;
                }
                self.switch_tab(*tab);
                let idx = self.active_tab;
                let tab_id = self.tabs[idx].id;
                if !self.tabs[idx].url.starts_with("rust://") && self.active_webview.is_none() {
                    self.tabs[idx].force_blitz = false;
                    self.mount_or_navigate_webview(idx);
                    self.rebuild_chrome();
                    if !is_final {
                        self.arm_agent_load_wait(req_id, tab_id, Some(desc), 2500);
                        return;
                    }
                }
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        bus.send(AppEvent::AgentActionDone {
                            req_id,
                            ok: true,
                            summary: desc,
                            need_delay_ms: 0,
                        });
                    });
                }
            }
            AgentAction::CloseTab { tab } => {
                if *tab >= self.tabs.len() || self.tabs.len() <= 1 {
                    self.on_agent_action_done(
                        req_id,
                        false,
                        "닫을 수 없는 탭입니다 (마지막 탭이거나 잘못된 인덱스)".to_string(),
                        0,
                    );
                    return;
                }
                self.close_tab(*tab, None);
                // Later tab indices in this batch were numbered before the close: re-plan instead
                // of switching to / closing the wrong tab.
                if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id)
                    && let Some(cut) = run
                        .pending_actions
                        .iter()
                        .position(|a| matches!(a, AgentAction::SwitchTab { .. } | AgentAction::CloseTab { .. }))
                {
                    run.pending_actions.truncate(cut);
                    run.finish_after_queue = false;
                }
                let is_final = is_final
                    && self
                        .agent_run
                        .as_ref()
                        .is_some_and(|r| r.finish_after_queue && r.pending_actions.is_empty());
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        bus.send(AppEvent::AgentActionDone {
                            req_id,
                            ok: true,
                            summary: desc,
                            need_delay_ms: 0,
                        });
                    });
                }
            }
            AgentAction::Extract { kind, copy } => {
                let Some(wv) = &self.active_webview else {
                    self.on_agent_action_done(
                        req_id,
                        false,
                        format!("{desc} (WebView2가 활성화된 웹페이지가 필요합니다)"),
                        0,
                    );
                    return;
                };
                let js = build_agent_extract_js(kind);
                let desc_c = desc.clone();
                let copy_flag = *copy;
                let res = wv.evaluate_script_with_callback(&js, move |raw| {
                    let parsed = serde_json::from_str::<serde_json::Value>(&raw)
                        .ok()
                        .and_then(|v| match v {
                            serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(&s).ok(),
                            other => Some(other),
                        });
                    let ok = parsed
                        .as_ref()
                        .and_then(|v| v.get("ok"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let data = parsed
                        .as_ref()
                        .and_then(|v| v.get("data"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let summary = if ok {
                        desc_c.clone()
                    } else {
                        let msg = parsed
                            .as_ref()
                            .and_then(|v| v.get("msg"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("추출 실패");
                        format!("{desc_c} ({msg})")
                    };
                    bus.send(AppEvent::AgentExtractDone {
                        req_id,
                        ok,
                        summary,
                        extracted: data,
                        copy: copy_flag,
                    });
                });
                if res.is_err() {
                    self.on_agent_action_done(req_id, false, format!("{desc} (스크립트 실행 실패)"), 0);
                }
            }
            AgentAction::Back => {
                self.history_step(false);
                self.ensure_active_valid();
                let tab_id = self.tabs[self.active_tab].id;
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    self.arm_agent_load_wait(req_id, tab_id, Some(desc), 2200);
                }
            }
            AgentAction::Forward => {
                self.history_step(true);
                self.ensure_active_valid();
                let tab_id = self.tabs[self.active_tab].id;
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    self.arm_agent_load_wait(req_id, tab_id, Some(desc), 2200);
                }
            }
            AgentAction::Reload => {
                self.reload_active_tab();
                self.ensure_active_valid();
                let tab_id = self.tabs[self.active_tab].id;
                if is_final {
                    self.on_agent_action_done(req_id, true, desc, 0);
                } else {
                    self.arm_agent_load_wait(req_id, tab_id, Some(desc), 2200);
                }
            }
            AgentAction::Wait { ms } => {
                let wait_ms = (*ms).clamp(100, 5000);
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
                    bus.send(AppEvent::AgentActionDone {
                        req_id,
                        ok: true,
                        summary: desc,
                        need_delay_ms: 0,
                    });
                });
            }
            AgentAction::WaitFor { text, timeout_ms } => {
                let wait_gen = if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                    run.load_wait_seq += 1;
                    let wg = run.load_wait_seq;
                    run.awaiting_text = Some(AgentTextWait {
                        text: text.clone(),
                        wait_gen: wg,
                        deadline: Instant::now() + Duration::from_millis((*timeout_ms).clamp(300, 8000)),
                        action_summary: desc,
                    });
                    wg
                } else {
                    return;
                };
                self.poll_agent_text_wait(req_id, wait_gen);
            }
            _ => {
                let Some(js) = build_agent_action_js(&action) else {
                    self.on_agent_action_done(req_id, false, "지원되지 않는 액션".to_string(), 0);
                    return;
                };
                let is_submit_or_click = matches!(
                    action,
                    AgentAction::Click { .. }
                        | AgentAction::ClickText { .. }
                        | AgentAction::Type { submit: true, .. }
                        | AgentAction::Press { .. }
                );
                let settle_ms = if is_final {
                    0
                } else if has_more_queued {
                    if is_submit_or_click { 120 } else { 60 }
                } else if is_submit_or_click {
                    180
                } else {
                    80
                };
                if let Some(wv) = &self.active_webview {
                    let desc_c = desc.clone();
                    let res = wv.evaluate_script_with_callback(&js, move |raw| {
                        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
                            .ok()
                            .and_then(|v| match v {
                                serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(&s).ok(),
                                other => Some(other),
                            });
                        let ok = parsed
                            .as_ref()
                            .and_then(|v| v.get("ok"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        let msg = parsed
                            .as_ref()
                            .and_then(|v| v.get("msg"))
                            .and_then(|v| v.as_str())
                            .map(|m| if ok { desc_c.clone() } else { format!("{desc_c} ({m})") })
                            .unwrap_or_else(|| desc_c.clone());
                        bus.send(AppEvent::AgentActionDone {
                            req_id,
                            ok,
                            summary: msg,
                            need_delay_ms: settle_ms,
                        });
                    });
                    if res.is_err() {
                        self.on_agent_action_done(req_id, false, format!("{desc} (스크립트 실행 실패)"), 0);
                    }
                } else {
                    self.on_agent_action_done(
                        req_id,
                        false,
                        format!("{desc} (WebView2가 활성화된 웹페이지가 필요합니다)"),
                        0,
                    );
                }
            }
        }
    }
}

fn format_agent_final_message(cleaned: &str, summary: &str, log: &[String]) -> String {
    let body = if !cleaned.is_empty() {
        cleaned.to_string()
    } else if !summary.is_empty() {
        summary.to_string()
    } else {
        "작업을 완료했습니다.".to_string()
    };
    if log.is_empty() {
        return body;
    }
    let mut out = String::from("**실행한 브라우저 제어**\n");
    for (i, item) in log.iter().enumerate() {
        out.push_str(&format!("{}. {item}\n", i + 1));
    }
    out.push_str("\n---\n\n");
    out.push_str(&body);
    out
}

fn user_text_fallback(msgs: &[ChatMessage]) -> String {
    msgs.last().map(|m| m.text.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::chat::{ChatMessage, ChatProvider, ChatRole, CliAvailability, preset_models};
    use crate::chrome_ui::{SidebarPicker, SidebarRenderState, render_sidebar_html};
    use blitz_dom::{Document, DocumentConfig, EventDriver, EventHandler, LocalName, NodeId};
    use blitz_html::HtmlDocument;
    use blitz_traits::events::{
        BlitzPointerEvent, BlitzPointerId, DomEvent, DomEventData, EventState, MouseEventButton, MouseEventButtons,
        PointerCoords, PointerDetails, UiEvent,
    };
    use blitz_traits::shell::{ColorScheme, Viewport};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Records clicked `data-action`s, like `SidebarEventHandler`.
    struct Rec(Rc<RefCell<Vec<String>>>);
    impl EventHandler for Rec {
        fn handle_event(&mut self, chain: &[NodeId], ev: &mut DomEvent, doc: &mut dyn Document, _: &mut EventState) {
            if let DomEventData::Click(_) = &ev.data {
                let inner = doc.inner();
                if let Some(a) = chain.iter().find_map(|&id| {
                    inner
                        .get_node(id)?
                        .element_data()?
                        .attr(LocalName::from("data-action"))
                        .map(str::to_string)
                }) {
                    self.0.borrow_mut().push(a);
                }
            }
        }
    }

    fn pev(x: f32, y: f32, buttons: MouseEventButtons) -> BlitzPointerEvent {
        BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: PointerCoords {
                screen_x: x,
                screen_y: y,
                client_x: x,
                client_y: y,
                page_x: x,
                page_y: y,
            },
            button: MouseEventButton::Main,
            buttons,
            mods: Default::default(),
            details: PointerDetails::default(),
            element: Default::default(),
            active_pointers: Default::default(),
        }
    }

    fn sidebar_doc(height: u32, show_settings: bool, msgs: &[ChatMessage]) -> HtmlDocument {
        let opts = preset_models(ChatProvider::OpenAiApi, &[]);
        let (html, _) = render_sidebar_html(&SidebarRenderState {
            provider: ChatProvider::OpenAiApi,
            avail: CliAvailability::default(),
            messages: msgs,
            is_loading: false,
            include_page: true,
            agent_control: false,
            agent_status: None,
            agent_log: &[],
            sidebar_wide: false,
            show_settings,
            picker_open: SidebarPicker::None,
            active_title: "T",
            active_url: "https://e.test",
            active_model_id: "gpt-4o-mini",
            active_model_label: "GPT-4o mini",
            control_model_id: "",
            custom_presets: &[],
            model_options: &opts,
            agy_models_loading: false,
            can_retry: false,
            api_endpoint: "https://api.openai.com/v1/chat/completions",
            api_model: "gpt-4o-mini",
            has_api_key: false,
            draft: "",
            font_size: 13,
            reveal: None,
            show_history: false,
            history: &[],
            session_id: None,
            agent_confirm: None,
            motion: Default::default(),
        });
        let mut doc = HtmlDocument::from_html(
            &html,
            DocumentConfig {
                viewport: Some(Viewport::new(380, height, 1.0, ColorScheme::Light)),
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        doc
    }

    fn center(doc: &HtmlDocument, sel: &str) -> (f32, f32) {
        let id = doc
            .query_selector(sel)
            .unwrap()
            .unwrap_or_else(|| panic!("{sel} missing"));
        let n = doc.get_node(id).unwrap();
        let p = n.absolute_position(0.0, 0.0);
        let s = n.final_layout().size;
        (p.x + s.width / 2.0, p.y + s.height / 2.0)
    }

    fn click(doc: &mut HtmlDocument, rec: &Rc<RefCell<Vec<String>>>, (x, y): (f32, f32)) {
        EventDriver::new(&mut **doc, Rec(rec.clone())).handle_ui_event(UiEvent::PointerDown(pev(
            x,
            y,
            MouseEventButtons::Primary,
        )));
        EventDriver::new(&mut **doc, Rec(rec.clone())).handle_ui_event(UiEvent::PointerUp(pev(
            x,
            y,
            MouseEventButtons::None,
        )));
    }

    /// Regression: the 저장 button and API inputs sat below the settings card's fold, so
    /// clicks there landed on the conversation behind it.
    #[test]
    fn settings_controls_are_clickable_in_a_short_window() {
        let rec = Rc::new(RefCell::new(Vec::new()));
        let mut doc = sidebar_doc(640, true, &[]);
        for sel in [
            r#"[data-action="chat-provider:anthropic_api"]"#,
            r#"[data-action="chat-save-settings"]"#,
            r#"[data-action="chat-font:inc"]"#,
        ] {
            let pos = center(&doc, sel);
            click(&mut doc, &rec, pos);
        }
        assert_eq!(
            *rec.borrow(),
            ["chat-provider:anthropic_api", "chat-save-settings", "chat-font:inc"]
        );
    }

    /// Regression: `user-select: none` on <body> was inherited by reply text, so dragging never
    /// started a selection and there was nothing to copy.
    #[test]
    fn reply_text_can_be_drag_selected() {
        let msgs = [ChatMessage {
            role: ChatRole::Assistant,
            text: "Hello world, this is a long reply paragraph for selection.".into(),
            page_title: None,
            provider_label: None,
        }];
        let mut doc = sidebar_doc(720, false, &msgs);
        let id = doc.query_selector(".msg-body p").unwrap().unwrap();
        let p = doc.get_node(id).unwrap().absolute_position(0.0, 0.0);
        let (x0, y) = (p.x + 1.0, p.y + 8.0);
        EventDriver::new(&mut *doc, Rec(Rc::default())).handle_ui_event(UiEvent::PointerDown(pev(
            x0,
            y,
            MouseEventButtons::Primary,
        )));
        EventDriver::new(&mut *doc, Rec(Rc::default())).handle_ui_event(UiEvent::PointerMove(pev(
            x0 + 70.0,
            y,
            MouseEventButtons::Primary,
        )));
        EventDriver::new(&mut *doc, Rec(Rc::default())).handle_ui_event(UiEvent::PointerUp(pev(
            x0 + 70.0,
            y,
            MouseEventButtons::None,
        )));
        let sel = doc.get_selected_text().unwrap_or_default();
        assert!(sel.starts_with("Hello"), "selected {sel:?}");
        assert!(sel.len() < msgs[0].text.len(), "only part of the reply: {sel:?}");

        // Buttons stay unselectable (dragging over them must not start a selection).
        doc.clear_text_selection();
        let (bx, by) = center(&doc, r#"[data-action="chat-copy-msg:0"]"#);
        EventDriver::new(&mut *doc, Rec(Rc::default())).handle_ui_event(UiEvent::PointerDown(pev(
            bx,
            by,
            MouseEventButtons::Primary,
        )));
        EventDriver::new(&mut *doc, Rec(Rc::default())).handle_ui_event(UiEvent::PointerMove(pev(
            bx + 40.0,
            by,
            MouseEventButtons::Primary,
        )));
        assert!(doc.get_selected_text().is_none());
    }

    #[test]
    fn typing_reveal_shows_partial_text_with_caret() {
        let msgs = vec![ChatMessage {
            role: ChatRole::Assistant,
            text: "첫 문장입니다. 두 번째 문장은 아직 안 보여야 합니다.".into(),
            page_title: None,
            provider_label: None,
        }];
        let opts = preset_models(ChatProvider::Claude, &[]);
        let state = |reveal| SidebarRenderState {
            provider: ChatProvider::Claude,
            avail: CliAvailability::default(),
            messages: &msgs,
            is_loading: false,
            include_page: false,
            agent_control: false,
            agent_status: None,
            agent_log: &[],
            sidebar_wide: false,
            show_settings: false,
            picker_open: SidebarPicker::None,
            active_title: "",
            active_url: "rust://home",
            active_model_id: "",
            active_model_label: "",
            control_model_id: "",
            custom_presets: &[],
            model_options: &opts,
            agy_models_loading: false,
            can_retry: false,
            api_endpoint: "",
            api_model: "",
            has_api_key: false,
            draft: "줄1\n줄2",
            font_size: 16,
            reveal,
            show_history: false,
            history: &[],
            session_id: None,
            agent_confirm: None,
            motion: Default::default(),
        };
        let (typing, _) = render_sidebar_html(&state(Some((0, 8))));
        assert!(
            typing.contains(r#"첫 문장입니다.<span class="type-caret"></span></p>"#),
            "{typing}"
        );
        assert!(!typing.contains("두 번째"));
        assert!(!typing.contains("chat-copy-msg:0"), "footer hidden while typing");
        let (done, _) = render_sidebar_html(&state(None));
        assert!(done.contains("두 번째") && done.contains("chat-regen") && !done.contains(r#"class="type-caret""#));
        assert!(done.contains("--chat-fs: 16px"));
        assert!(done.contains("<textarea id=\"chat-input\"") && done.contains(">줄1\n줄2</textarea>"));
    }
}
