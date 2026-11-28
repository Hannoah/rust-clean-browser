//! Docked right-hand AI Assistant sidebar: lifecycle, resizing, prompt submission,
//! active webpage context extraction (Clean & JS modes), and clipboard copy.

mod actions;
mod agent;
mod live;
mod tools;

pub(in crate::app) use live::LiveActivity;

use super::bus::SidebarEventHandler;
use super::{AppEvent, BrowserApp, UiRegion};
use crate::chat::{
    AgentAction, ChatHistory, ChatMessage, ChatProvider, ChatRole, ModelOption, display_model_label, fetch_agy_models,
    preset_models,
};
use crate::chrome_ui::{SidebarPicker, SidebarRenderState, render_sidebar_html};
use crate::memory::trim_process_working_set;
use blitz_dom::{EventDriver, ScrollBehavior};
use blitz_html::HtmlDocument;
use blitz_traits::events::UiEvent;
use blitz_traits::shell::Viewport;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;

pub(super) const MIN_SIDEBAR_WIDTH: u32 = 280;
pub(super) const MAX_SIDEBAR_WIDTH: u32 = 960;
pub(super) const MIN_CHAT_FONT: u32 = 11;
pub(super) const MAX_CHAT_FONT: u32 = 20;
const REVEAL_TICK_MS: u64 = 30;
const REVEAL_FRAMES: usize = 15;
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
    pub target_tab_id: u64,
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
    /// PNG (base64) from the `screenshot` tool, sent with the next round only.
    pub screenshot: Option<String>,
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
            || Self::is_input_composing(doc, "chat-api-key")
            || Self::is_input_composing(doc, "chat-custom-model")
            || Self::is_input_composing(doc, "chat-preset-title")
            || Self::is_input_composing(doc, "chat-preset-prompt")
    }

    pub(super) fn dispatch_sidebar_ui(&mut self, event: UiEvent) -> bool {
        let Some(doc) = &mut self.sidebar_doc else {
            return false;
        };
        let mut handled = false;
        let handler = SidebarEventHandler {
            bus: self.bus.clone(),
            handled: &mut handled,
        };
        EventDriver::new(doc, handler).handle_ui_event(event);
        handled
    }

    pub(super) fn sidebar_action_at(&self, pos: PhysicalPosition<f64>) -> Option<String> {
        let doc = self.sidebar_doc.as_ref()?;
        let c = self.coords_for_region(pos, UiRegion::Sidebar);
        let mut node_id = doc.element_from_point(c.client_x, c.client_y);
        while let Some(id) = node_id {
            let node = doc.get_node(id)?;
            if let Some(el) = node.element_data()
                && let Some(action) = el.attr(blitz_dom::LocalName::from("data-action"))
            {
                return Some(action.to_string());
            }
            node_id = node.parent;
        }
        None
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
            self.sync_ime_transform();
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

    /// Fetches the active provider's real model list (agy CLI or API endpoint).
    pub(super) fn maybe_fetch_models(&mut self, force: bool) {
        let provider = self.chat_config.provider;
        if provider == ChatProvider::Agy {
            self.maybe_fetch_agy_models(force);
            return;
        }
        if provider.is_cli() || self.api_models_loading || (!force && self.api_models.contains_key(&provider)) {
            return;
        }
        self.api_models_loading = true;
        let config = self.chat_config.clone();
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let result = crate::chat::fetch_api_models(&config).await;
            bus.send(AppEvent::ApiModelsLoaded { provider, result });
        });
        if self.sidebar_open {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }

    pub(super) fn on_api_models_loaded(&mut self, provider: ChatProvider, result: Result<Vec<ModelOption>, String>) {
        self.api_models_loading = false;
        self.api_models.insert(provider, result);
        if self.sidebar_open {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }

    /// `true` while the active provider's model list is being fetched.
    fn models_loading(&self) -> bool {
        if self.chat_config.provider == ChatProvider::Agy {
            self.agy_models_loading
        } else {
            self.api_models_loading
        }
    }

    /// Error from the last API model listing (presets are shown instead).
    fn models_error(&self) -> Option<&str> {
        match self.api_models.get(&self.chat_config.provider) {
            Some(Err(e)) if !self.chat_config.provider.is_cli() => Some(e.as_str()),
            _ => None,
        }
    }

    fn current_model_options(&self) -> Vec<ModelOption> {
        let provider = self.chat_config.provider;
        // A successful listing from the endpoint replaces the hard-coded presets:
        // only models this key can actually use are offered.
        let mut opts = match self.api_models.get(&provider) {
            Some(Ok(list)) if !provider.is_cli() => list.clone(),
            _ => preset_models(provider, &self.agy_models),
        };
        if !self.chat_config.provider.is_cli() {
            for id in self.chat_config.custom_models_for(self.chat_config.provider) {
                if !opts.iter().any(|o| o.id == id) {
                    opts.push(ModelOption {
                        id: id.clone(),
                        label: id,
                    });
                }
            }
        }
        opts
    }

    pub(super) fn warm_cli_for_current_mode(&self) {
        let agent_mode = self.chat_config.agent_control;
        match self.chat_config.provider {
            ChatProvider::Agy if self.chat_cli_avail.agy => {
                let model = if agent_mode {
                    self.chat_config.effective_control_model(ChatProvider::Agy)
                } else {
                    self.chat_config.selected_model(ChatProvider::Agy)
                };
                let effort = self.chat_config.effective_effort(ChatProvider::Agy, agent_mode);
                crate::chat::warm_agy_worker(model, effort);
            }
            ChatProvider::Claude if self.chat_cli_avail.claude => {
                let model = if agent_mode {
                    self.chat_config.effective_control_model(ChatProvider::Claude)
                } else {
                    self.chat_config.selected_model(ChatProvider::Claude)
                };
                let effort = self.chat_config.effective_effort(ChatProvider::Claude, agent_mode);
                crate::chat::warm_claude_worker(model, effort, agent_mode);
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
            self.rebuild_sidebar_now(true);
            self.focus_sidebar_input("chat-input");
        } else {
            self.sidebar_drag = false;
            self.sidebar_pressed_action = None;
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
        self.submit_chat_message(
            "현재 웹페이지의 핵심 내용을 이해하기 쉽게 요약해 주세요.".to_string(),
            true,
            false,
        );
    }

    /// Background-safe rebuild: defers if a sidebar text field is mid-IME composition.
    pub(super) fn rebuild_sidebar(&mut self, scroll_to_bottom: bool) {
        self.rebuild_sidebar_inner(scroll_to_bottom, true);
    }

    /// Immediate rebuild for explicit user interactions (clicking Settings, History, etc.):
    /// never suppressed by stray/stale IME composing flags in `#chat-input`.
    pub(super) fn rebuild_sidebar_now(&mut self, scroll_to_bottom: bool) {
        self.rebuild_sidebar_inner(scroll_to_bottom, false);
    }

    fn rebuild_sidebar_inner(&mut self, scroll_to_bottom: bool, defer_if_composing: bool) {
        if !self.sidebar_open {
            return;
        }
        if defer_if_composing && self.sidebar_input_composing() {
            self.sidebar_dirty = true;
            self.sidebar_dirty_scroll |= scroll_to_bottom;
            return;
        }
        self.sidebar_dirty = false;
        self.sidebar_dirty_scroll = false;
        self.ensure_active_valid();
        let motion = self.sidebar_motion();
        let focused_input_id = [
            "chat-input",
            "chat-api-endpoint",
            "chat-api-key",
            "chat-custom-model",
            "chat-preset-title",
            "chat-preset-prompt",
        ]
        .into_iter()
        .find(|id| self.is_sidebar_input_focused(id));
        let draft = self.read_sidebar_input("chat-input").unwrap_or_default();
        // After a provider/model chip click the inputs must show the new choice, not the old draft
        // (which 저장/적용 would otherwise write back over the selection).
        let reset_drafts = std::mem::take(&mut self.sidebar_reset_drafts);
        let endpoint_draft = self.read_sidebar_input("chat-api-endpoint").filter(|_| !reset_drafts);
        let key_draft = self
            .read_sidebar_input("chat-api-key")
            .filter(|s| !reset_drafts && !s.is_empty());
        let custom_model_draft = self
            .read_sidebar_input("chat-custom-model")
            .filter(|s| !reset_drafts && !s.is_empty());
        let preset_title_draft = self
            .read_sidebar_input("chat-preset-title")
            .filter(|s| !reset_drafts && !s.is_empty());
        let preset_prompt_draft = self
            .read_sidebar_input("chat-preset-prompt")
            .filter(|s| !reset_drafts && !s.is_empty());
        let active = &self.tabs[self.active_tab];
        let model_options = self.current_model_options();
        let custom_models = if self.chat_config.provider.is_cli() {
            Vec::new()
        } else {
            self.chat_config.custom_models_for(self.chat_config.provider)
        };
        let active_model_id = self.chat_config.selected_model(self.chat_config.provider);
        let active_model_label = display_model_label(self.chat_config.provider, &active_model_id, &self.agy_models);
        // Offer only usable entries: installed CLI binaries, connected (key saved
        // or loopback) API providers, and always the active one. The legacy
        // `gemini` CLI entry was folded into the `agy` (Gemini) agent.
        let visible_providers: Vec<ChatProvider> = ChatProvider::ALL
            .iter()
            .copied()
            .filter(|&p| {
                p != ChatProvider::Gemini
                    && self
                        .chat_config
                        .is_usable(p, self.chat_config.provider, self.chat_cli_avail)
            })
            .collect();
        let api_endpoint = endpoint_draft
            .as_deref()
            .unwrap_or_else(|| self.chat_config.effective_endpoint());
        let can_retry = self.chat_inflight.is_none()
            && self.chat_messages.last().is_some_and(|m| m.role == ChatRole::Error)
            && self.chat_messages.iter().any(|m| m.role == ChatRole::User);

        let (html, code_blocks) = render_sidebar_html(&SidebarRenderState {
            provider: self.chat_config.provider,
            avail: self.chat_cli_avail,
            providers: &visible_providers,
            picker_open: self.sidebar_picker,
            active_model_id: &active_model_id,
            active_model_label: &active_model_label,
            control_model_id: &self.chat_config.selected_control_model(self.chat_config.provider),
            effort: &self.chat_config.selected_effort(self.chat_config.provider),
            custom_presets: &self.chat_config.custom_presets,
            model_options: &model_options,
            agy_models_loading: self.models_loading(),
            models_error: self.models_error(),
            custom_models: &custom_models,
            context: (!self.chat_config.provider.is_cli())
                .then(|| crate::chat::api_context_usage(&self.chat_config, &self.chat_messages)),
            compacting: self.chat_compacting.is_some(),
            can_compact: self
                .chat_messages
                .iter()
                .rev()
                .take_while(|m| m.role != ChatRole::Summary)
                .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
                .count()
                >= 2,
            messages: &self.chat_messages,
            is_loading: self.chat_inflight.is_some(),
            can_retry,
            include_page: self.chat_config.include_page,
            agent_control: self.chat_config.agent_control,
            live: self.chat_live.as_ref().map(|l| {
                (
                    live::live_label(l, self.agent_run.as_ref().map(|r| r.status.as_str())),
                    l.steps.as_slice(),
                )
            }),
            activity_open: &self.chat_activity_open,
            sidebar_wide: self.sidebar_logical_width() >= 520,
            show_settings: self.chat_show_settings,
            active_title: &active.title,
            active_url: &active.url,
            api_endpoint,
            has_api_key: !self.chat_config.effective_api_key().is_empty(),
            api_key_hint: &self.chat_config.masked_api_key_hint(),
            api_conn: match &self.api_conn {
                Some((pid, ok, msg)) if *pid == self.chat_config.provider.id() => Some((*ok, msg.as_str())),
                _ => None,
            },
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
        if let Some(doc) = &mut self.sidebar_doc {
            for (input_id, val) in [
                ("chat-api-key", key_draft),
                ("chat-custom-model", custom_model_draft),
                ("chat-preset-title", preset_title_draft),
                ("chat-preset-prompt", preset_prompt_draft),
            ] {
                if let Some(val) = val
                    && let Some(nid) = doc.get_element_by_id(input_id)
                {
                    doc.with_text_input(nid, |mut driver| {
                        driver.select_all();
                        driver.insert_or_replace_selection(&val);
                    });
                }
            }
        }
        self.sync_key_mask();
        if let Some(fid) = focused_input_id {
            self.focus_sidebar_input_inner(fid, false);
            if let Some(doc) = &mut self.sidebar_doc
                && let Some(id) = doc.get_element_by_id(fid)
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

    pub(super) fn cancel_chat_inflight(&mut self, record_note: bool) {
        self.agent_run = None;
        self.chat_live = None;
        self.chat_stream_req = None;
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
                    activity: Vec::new(),
                    elapsed_ms: None,
                });
                self.persist_chat();
                self.last_sidebar_html.clear();
                self.rebuild_sidebar_now(true);
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
        // Re-read before writing: every browser window is its own process with its own copy, and
        // saving a stale copy used to erase conversations another window had just saved.
        let mut history = ChatHistory::load();
        history.upsert(id, &self.chat_messages);
        history.save();
        self.chat_history = Some(history);
    }

    fn load_chat_session(&mut self, id: u64) {
        let Some(session) = self.chat_history.get_or_insert_with(ChatHistory::load).get(id).cloned() else {
            return;
        };
        self.cancel_chat_inflight(false);
        self.chat_messages = session.messages;
        self.chat_session_id = Some(id);
        self.chat_activity_open.clear();
        self.chat_show_history = false;
        // The persistent Claude worker holds the previous conversation: start fresh so the
        // loaded history is sent with the next turn.
        crate::chat::reset_all_cli_workers();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar_now(true);
    }

    /// Deletes one saved conversation, or all of them with `None`.
    fn delete_chat_session(&mut self, id: Option<u64>) {
        let mut history = ChatHistory::load(); // fresh copy, see `persist_chat`
        match id {
            Some(id) => history.remove(id),
            None => history.sessions.clear(),
        }
        history.save();
        self.chat_history = Some(history);
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
        self.rebuild_sidebar_now(false);
    }

    /// Drops the last assistant reply and asks again.
    fn regenerate_last_reply(&mut self) {
        if self.chat_inflight.is_some() {
            return;
        }
        self.chat_stream_req = None;
        self.finish_reveal();
        if self.chat_messages.last().is_some_and(|m| m.role == ChatRole::Assistant) {
            self.chat_messages.pop();
        }
        crate::chat::reset_all_cli_workers();
        self.retry_last_chat();
    }

    // ── Typing effect ──

    fn start_reveal(&mut self, msg_idx: usize) {
        self.chat_reveal_gen = self.chat_reveal_gen.wrapping_add(1);
        self.chat_reveal = Some((msg_idx, 0));
        self.schedule_reveal_tick(self.chat_reveal_gen);
    }

    fn schedule_reveal_tick(&self, reveal_gen: u64) {
        let bus = self.bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(REVEAL_TICK_MS)).await;
            bus.send(AppEvent::ChatRevealTick(reveal_gen));
        });
    }

    pub(super) fn on_chat_reveal_tick(&mut self, reveal_gen: u64) {
        if reveal_gen != self.chat_reveal_gen {
            return;
        }
        let Some((idx, shown)) = self.chat_reveal else {
            return;
        };
        let total = self.chat_messages.get(idx).map_or(0, |m| m.text.chars().count());
        let next = shown + (total / REVEAL_FRAMES).max(4);
        if next >= total {
            self.chat_reveal = None;
        } else {
            self.chat_reveal = Some((idx, next));
            self.schedule_reveal_tick(reveal_gen);
        }
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    /// Shows the rest of a reply that is still typing out (new request, stop, history switch).
    fn finish_reveal(&mut self) {
        self.chat_reveal_gen = self.chat_reveal_gen.wrapping_add(1);
        if self.chat_reveal.take().is_some() {
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }

    /// Blitz draws `type=password` text in the clear, so the key input's text is transparent
    /// and this overlay shows one dot per typed character (or the placeholder when empty).
    pub(super) fn sync_key_mask(&mut self) {
        let Some(len) = self.read_sidebar_input("chat-api-key").map(|s| s.chars().count()) else {
            return;
        };
        let anim_time = self.anim_time();
        let Some(doc) = &mut self.sidebar_doc else {
            return;
        };
        let (Some(dots), Some(ph)) = (
            doc.get_element_by_id("chat-api-key-dots"),
            doc.get_element_by_id("chat-api-key-ph"),
        ) else {
            return;
        };
        doc.mutate().set_inner_html(dots, &"•".repeat(len.min(64)));
        doc.set_style_property(ph, "display", if len == 0 { "inline" } else { "none" });
        doc.resolve(anim_time);
        self.request_redraw();
    }

    /// Grows the chat textarea with its content (Shift+Enter lines), up to `MAX_INPUT_LINES`.
    ///
    /// The editor scroll is pinned back to the top after growing: Blitz keeps the
    /// caret-driven scroll offset from before the box grew, which otherwise leaves
    /// the earlier lines scrolled out of view after a Shift+Enter newline.
    pub(super) fn sync_chat_input_height(&mut self) {
        let Some(text) = self.read_sidebar_input("chat-input") else {
            return;
        };
        let anim_time = self.anim_time();
        let Some(doc) = &mut self.sidebar_doc else {
            return;
        };
        let Some(id) = doc.get_element_by_id("chat-input") else {
            return;
        };
        // Visual lines from the editor's own layout, so soft-wrapped text grows the box
        // too (counting only '\n' left the first wrapped line scrolled out of view).
        doc.resolve(anim_time);
        // `driver.layout()` re-lays out the editor first: right after a paste (applied inside the
        // Ctrl+V keydown) the cached layout is still the pre-paste one, which kept the box short
        // and scrolled the first line away until the next key event.
        let mut wrapped = None;
        doc.with_text_input(id, |mut driver| wrapped = Some(driver.layout().len()));
        let lines = wrapped
            .unwrap_or_else(|| text.split('\n').count())
            .clamp(1, MAX_INPUT_LINES);
        if lines == self.chat_input_lines {
            return;
        }
        self.chat_input_lines = lines;
        {
            doc.set_style_property(
                id,
                "height",
                &format!("{}px", crate::chrome_ui::chat_input_height(lines)),
            );
            doc.resolve(anim_time);
            doc.scroll_to(id, 0.0, 0.0, ScrollBehavior::Instant);
            self.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::agent::is_agent_host_trusted;
    use crate::chat::{ChatMessage, ChatProvider, ChatRole, CliAvailability, preset_models};
    use crate::chrome_ui::{SidebarPicker, SidebarRenderState, render_sidebar_html};
    use blitz_dom::{Document, DocumentConfig, EventDriver, EventHandler, LocalName, NodeId, ScrollBehavior};
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
            live: None,
            activity_open: &Default::default(),
            sidebar_wide: false,
            show_settings,
            picker_open: SidebarPicker::None,
            active_title: "T",
            active_url: "https://e.test",
            active_model_id: "gpt-4o-mini",
            active_model_label: "GPT-4o mini",
            control_model_id: "",
            effort: "",
            custom_presets: &[],
            model_options: &opts,
            agy_models_loading: false,
            models_error: None,
            custom_models: &[],
            context: None,
            compacting: false,
            can_compact: false,
            can_retry: false,
            api_endpoint: "https://api.openai.com/v1/chat/completions",
            has_api_key: false,
            providers: &[
                ChatProvider::Agy,
                ChatProvider::Claude,
                ChatProvider::Codex,
                ChatProvider::OpenAiApi,
                ChatProvider::AnthropicApi,
                ChatProvider::GeminiApi,
            ],
            api_key_hint: "",
            api_conn: None,
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
        ] {
            let pos = center(&doc, sel);
            click(&mut doc, &rec, pos);
        }
        // The card grows with more sections (custom models, connection status):
        // scroll it to the font row first, like a user would.
        let set_id = doc.get_element_by_id("chat-settings").unwrap();
        doc.scroll_to(set_id, 0.0, 1.0e9, ScrollBehavior::Instant);
        doc.resolve(0.05);
        let pos = center(&doc, r#"[data-action="chat-font:inc"]"#);
        click(&mut doc, &rec, pos);
        assert_eq!(
            *rec.borrow(),
            ["chat-provider:anthropic_api", "chat-save-settings", "chat-font:inc"]
        );
    }

    /// Regression: a pasted long line wraps, but the line count used to size the chat box came
    /// from the pre-paste layout, so the first line scrolled away until the next key event.
    #[test]
    fn pasted_text_line_count_is_fresh() {
        let mut doc = sidebar_doc(720, false, &[]);
        let id = doc.get_element_by_id("chat-input").unwrap();
        doc.set_focus_to(id);
        let pasted = "a fairly long pasted sentence that has to wrap inside the narrow sidebar box ".repeat(3);
        doc.with_text_input(id, |mut d| d.insert_or_replace_selection(&pasted));
        let mut lines = 0;
        doc.with_text_input(id, |mut d| lines = d.layout().len());
        assert!(lines >= 2, "wrapped paste should count as several lines, got {lines}");
    }

    /// Regression: Blitz hit-testing does not clip to a scroll container, so once the message
    /// list (or settings panel) was scrolled past the header's height, its scrolled-away content
    /// "covered" the header and 새 대화 / 기록 / 설정 / 닫기 stopped responding.
    #[test]
    fn header_stays_clickable_over_a_scrolled_conversation() {
        let msgs: Vec<ChatMessage> = (0..40)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                text: format!("message {i} with enough text to take up a couple of lines in the sidebar"),
                page_title: None,
                provider_label: None,
                activity: Vec::new(),
                elapsed_ms: None,
            })
            .collect();
        for (show_settings, scroller) in [(false, "chat-messages"), (true, "chat-settings")] {
            let rec = Rc::new(RefCell::new(Vec::new()));
            let mut doc = sidebar_doc(480, show_settings, &msgs);
            let id = doc.get_element_by_id(scroller).unwrap();
            doc.scroll_to(id, 0.0, 1.0e9, ScrollBehavior::Instant);
            doc.resolve(0.05);
            assert!(
                doc.get_node(id).unwrap().scroll_offset().y > 100.0,
                "{scroller} must scroll"
            );
            let actions = ["chat-clear", "chat-tab:history", "chat-tab:settings", "toggle-sidebar"];
            for a in actions {
                let pos = center(&doc, &format!(r#"[data-action="{a}"]"#));
                click(&mut doc, &rec, pos);
            }
            assert_eq!(*rec.borrow(), actions, "scrolled {scroller}");
        }
        // Mirror case: list scrolled to the top, its overflow sits under the composer.
        let rec = Rc::new(RefCell::new(Vec::new()));
        let mut doc = sidebar_doc(480, false, &msgs);
        let id = doc.get_element_by_id("chat-messages").unwrap();
        doc.scroll_to(id, 0.0, 0.0, ScrollBehavior::Instant);
        doc.resolve(0.05);
        let actions = ["chat-picker:model", "chat-toggle-page", "chat-send"];
        for a in actions {
            let pos = center(&doc, &format!(r#"[data-action="{a}"]"#));
            click(&mut doc, &rec, pos);
        }
        assert_eq!(*rec.borrow(), actions, "composer over an unscrolled list");
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
            activity: Vec::new(),
            elapsed_ms: None,
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
            activity: Vec::new(),
            elapsed_ms: None,
        }];
        let opts = preset_models(ChatProvider::Claude, &[]);
        let no_open = std::collections::HashSet::new();
        let state = |reveal| SidebarRenderState {
            provider: ChatProvider::Claude,
            avail: CliAvailability::default(),
            messages: &msgs,
            is_loading: false,
            include_page: false,
            agent_control: false,
            live: None,
            activity_open: &no_open,
            sidebar_wide: false,
            show_settings: false,
            picker_open: SidebarPicker::None,
            active_title: "",
            active_url: "rust://home",
            active_model_id: "",
            active_model_label: "",
            control_model_id: "",
            effort: "",
            custom_presets: &[],
            model_options: &opts,
            agy_models_loading: false,
            models_error: None,
            custom_models: &[],
            context: None,
            compacting: false,
            can_compact: false,
            can_retry: false,
            api_endpoint: "",
            has_api_key: false,
            providers: &[ChatProvider::Claude],
            api_key_hint: "",
            api_conn: None,
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
        // A two-line Shift+Enter draft renders a grown textarea so the first line stays visible.
        assert!(done.contains("style=\"height:48px\""));
    }

    #[test]
    fn agent_host_trust_requires_exact_host_not_substring() {
        let trusted = vec!["www.github.com".to_string()];
        let goal = "github.com에서 이슈를 확인해줘";
        assert!(is_agent_host_trusted("github.com", &trusted, goal));
        assert!(is_agent_host_trusted("www.github.com", &trusted, goal));
        assert!(!is_agent_host_trusted("github.co", &trusted, goal));
        assert!(!is_agent_host_trusted("hub.com", &trusted, goal));
        assert!(!is_agent_host_trusted("evil-github.com", &trusted, goal));

        let url_goal = "https://docs.rs/tokio/latest/tokio/ 열어줘";
        assert!(is_agent_host_trusted("docs.rs", &[], url_goal));
        assert!(!is_agent_host_trusted("docs.r", &[], url_goal));
    }

    struct SidebarTestOpts<'a> {
        width: u32,
        height: u32,
        scale: f32,
        show_settings: bool,
        show_history: bool,
        picker_open: SidebarPicker,
        agent_control: bool,
        history: &'a [crate::chat::ChatSession],
        shell_ms: Option<u64>,
    }

    fn sidebar_doc_with_options(o: SidebarTestOpts<'_>) -> HtmlDocument {
        let opts = preset_models(ChatProvider::OpenAiApi, &[]);
        let (html, _) = render_sidebar_html(&SidebarRenderState {
            provider: ChatProvider::OpenAiApi,
            avail: CliAvailability::default(),
            messages: &[],
            is_loading: false,
            include_page: true,
            agent_control: o.agent_control,
            live: None,
            activity_open: &Default::default(),
            sidebar_wide: o.width >= 520,
            show_settings: o.show_settings,
            picker_open: o.picker_open,
            active_title: "Test Page Title That Is Fairly Long",
            active_url: "https://example.test",
            active_model_id: "gpt-4o-mini",
            active_model_label: "GPT-4o mini",
            control_model_id: "",
            effort: "",
            custom_presets: &[],
            model_options: &opts,
            agy_models_loading: false,
            models_error: None,
            custom_models: &[],
            context: None,
            compacting: false,
            can_compact: false,
            can_retry: false,
            api_endpoint: "https://api.openai.com/v1/chat/completions",
            has_api_key: false,
            providers: &[
                ChatProvider::Agy,
                ChatProvider::Claude,
                ChatProvider::Codex,
                ChatProvider::OpenAiApi,
                ChatProvider::AnthropicApi,
                ChatProvider::GeminiApi,
            ],
            api_key_hint: "",
            api_conn: None,
            draft: "",
            font_size: 13,
            reveal: None,
            show_history: o.show_history,
            history: o.history,
            session_id: o.history.first().map(|s| s.id),
            agent_confirm: None,
            motion: crate::chrome_ui::SidebarMotion {
                msg: None,
                panel_ms: o.shell_ms,
                shell_ms: o.shell_ms,
            },
        });
        let phys_w = ((o.width as f32) * o.scale).round() as u32;
        let phys_h = ((o.height as f32) * o.scale).round() as u32;
        let mut doc = HtmlDocument::from_html(
            &html,
            DocumentConfig {
                viewport: Some(Viewport::new(phys_w, phys_h, o.scale, ColorScheme::Light)),
                style_threading: blitz_dom::StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        doc
    }

    fn click_resolved(doc: &mut HtmlDocument, rec: &Rc<RefCell<Vec<String>>>, (x, y): (f32, f32)) {
        EventDriver::new(&mut **doc, Rec(rec.clone())).handle_ui_event(UiEvent::PointerMove(pev(
            x,
            y,
            MouseEventButtons::None,
        )));
        EventDriver::new(&mut **doc, Rec(rec.clone())).handle_ui_event(UiEvent::PointerDown(pev(
            x,
            y,
            MouseEventButtons::Primary,
        )));
        // Simulate a real frame paint resolving `:active` between mouse-down and mouse-up.
        doc.resolve(0.05);
        EventDriver::new(&mut **doc, Rec(rec.clone())).handle_ui_event(UiEvent::PointerUp(pev(
            x,
            y,
            MouseEventButtons::None,
        )));
    }

    fn edge_point(doc: &HtmlDocument, sel: &str) -> (f32, f32) {
        let id = doc
            .query_selector(sel)
            .unwrap()
            .unwrap_or_else(|| panic!("{sel} missing"));
        let n = doc.get_node(id).unwrap();
        let p = n.absolute_position(0.0, 0.0);
        (p.x + 2.0, p.y + 2.0)
    }

    #[test]
    fn all_sidebar_header_composer_history_and_picker_buttons_are_clickable_across_scales_and_edges() {
        for (width, scale, agent_control, shell_ms) in [
            (280, 1.0_f32, false, None),
            (380, 1.25_f32, true, Some(0)),
            (380, 1.5_f32, true, None),
            (600, 2.0_f32, false, Some(20)),
        ] {
            let mut doc = sidebar_doc_with_options(SidebarTestOpts {
                width,
                height: 720,
                scale,
                show_settings: false,
                show_history: false,
                picker_open: SidebarPicker::None,
                agent_control,
                history: &[],
                shell_ms,
            });
            let rec = Rc::new(RefCell::new(Vec::new()));
            let header_and_composer = [
                ("chat-clear", r#"[data-action="chat-clear"]"#),
                ("chat-tab:history", r#"[data-action="chat-tab:history"]"#),
                ("chat-toggle-width", r#"[data-action="chat-toggle-width"]"#),
                ("chat-tab:settings", r#"[data-action="chat-tab:settings"]"#),
                ("toggle-sidebar", r#"[data-action="toggle-sidebar"]"#),
                ("chat-picker:provider", r#"[data-action="chat-picker:provider"]"#),
                ("chat-picker:model", r#"[data-action="chat-picker:model"]"#),
                ("chat-toggle-page", r#"[data-action="chat-toggle-page"]"#),
                ("chat-toggle-control", r#"[data-action="chat-toggle-control"]"#),
                ("chat-send", r#"[data-action="chat-send"]"#),
            ];
            for (expected, sel) in header_and_composer {
                rec.borrow_mut().clear();
                let c = center(&doc, sel);
                click_resolved(&mut doc, &rec, c);
                assert_eq!(
                    *rec.borrow(),
                    [expected],
                    "center click failed for {expected} w={width} scale={scale}"
                );

                rec.borrow_mut().clear();
                let e = edge_point(&doc, sel);
                click_resolved(&mut doc, &rec, e);
                assert_eq!(
                    *rec.borrow(),
                    [expected],
                    "edge click failed for {expected} w={width} scale={scale}"
                );
            }
        }

        // History buttons (row, delete, delete-all)
        let sessions = vec![crate::chat::ChatSession {
            id: 42,
            title: "Saved Chat".to_string(),
            updated: 1_700_000_000,
            messages: vec![ChatMessage {
                role: ChatRole::User,
                text: "hello".to_string(),
                page_title: None,
                provider_label: None,
                activity: Vec::new(),
                elapsed_ms: None,
            }],
        }];
        let mut hdoc = sidebar_doc_with_options(SidebarTestOpts {
            width: 380,
            height: 720,
            scale: 1.25,
            show_settings: false,
            show_history: true,
            picker_open: SidebarPicker::None,
            agent_control: false,
            history: &sessions,
            shell_ms: None,
        });
        let hrec = Rc::new(RefCell::new(Vec::new()));
        for (expected, sel) in [
            ("chat-load:42", r#"[data-action="chat-load:42"]"#),
            ("chat-del:42", r#"[data-action="chat-del:42"]"#),
            ("chat-del-all", r#"[data-action="chat-del-all"]"#),
        ] {
            hrec.borrow_mut().clear();
            let pos = center(&hdoc, sel);
            click_resolved(&mut hdoc, &hrec, pos);
            assert_eq!(*hrec.borrow(), [expected], "history click failed for {expected}");
        }

        // Picker sheet rows
        let mut pdoc = sidebar_doc_with_options(SidebarTestOpts {
            width: 380,
            height: 720,
            scale: 1.5,
            show_settings: false,
            show_history: false,
            picker_open: SidebarPicker::Provider,
            agent_control: false,
            history: &[],
            shell_ms: None,
        });
        let prec = Rc::new(RefCell::new(Vec::new()));
        let pos = center(&pdoc, r#"[data-action="chat-provider:claude"]"#);
        click_resolved(&mut pdoc, &prec, pos);
        assert_eq!(*prec.borrow(), ["chat-provider:claude"]);
    }

    #[test]
    fn ime_preedit_in_chat_input_does_not_block_forced_sidebar_rebuild() {
        let mut doc = sidebar_doc_with_options(SidebarTestOpts {
            width: 380,
            height: 720,
            scale: 1.0,
            show_settings: false,
            show_history: false,
            picker_open: SidebarPicker::None,
            agent_control: false,
            history: &[],
            shell_ms: None,
        });
        let nid = doc.get_element_by_id("chat-input").unwrap();
        doc.set_focus_to(nid);
        EventDriver::new(&mut *doc, Rec(Rc::default())).handle_ui_event(UiEvent::Ime(
            blitz_traits::events::BlitzImeEvent::Preedit("한".to_string(), Some((3, 3))),
        ));
        assert!(super::super::BrowserApp::is_input_composing(&doc, "chat-input"));
        let txt = doc
            .get_node(nid)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap()
            .editor
            .raw_text()
            .to_string();
        assert_eq!(txt, "한");
    }
}
