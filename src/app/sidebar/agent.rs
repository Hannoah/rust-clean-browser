use super::{AgentLoadWait, AgentTextWait, PendingConfirm};
use crate::app::{AppEvent, BrowserApp};
use crate::chat::{
    AGENT_SNAPSHOT_JS, AgentAction, ChatMessage, ChatRole, PageContext, build_agent_action_js, build_agent_extract_js,
    build_agent_text_check_js, format_agent_snapshot,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

impl BrowserApp {
    pub(super) fn format_open_tabs_summary(&self) -> String {
        let mut s = String::from("[OPEN TABS]\n");
        for (i, t) in self.tabs.iter().enumerate() {
            let mark = if i == self.active_tab { "*" } else { " " };
            let title: String = t.title.chars().take(48).collect();
            let url: String = t.url.chars().take(90).collect();
            s.push_str(&format!("{mark}[{i}] \"{title}\" ({url})\n"));
        }
        s
    }

    pub(super) fn arm_agent_load_wait(
        &mut self,
        req_id: u64,
        tab_id: u64,
        action_summary: Option<String>,
        timeout_ms: u64,
    ) {
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

    pub(in crate::app) fn on_agent_webview_loaded(&mut self, tab_id: u64) {
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

    pub(in crate::app) fn on_agent_load_settled(&mut self, req_id: u64, wait_gen: u64) {
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

    pub(super) fn arm_chat_script_timeout(&mut self, req_id: u64) -> u64 {
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
        let script_seq = self.chat_script_seq;
        let bus = self.bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            bus.send(AppEvent::ChatScriptTimeout { req_id, script_seq });
        });
        script_seq
    }

    pub(in crate::app) fn on_chat_script_timeout(&mut self, req_id: u64, script_seq: u64) {
        if script_seq != self.chat_script_seq || self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
        if self.agent_run.as_ref().is_some_and(|r| r.req_id == req_id) {
            self.finish_agent_run(Some("페이지 스크립트 응답 시간이 초과되었습니다.".to_string()));
        } else {
            self.on_chat_finished(
                req_id,
                self.chat_config.provider.short_label().to_string(),
                Err("페이지 스크립트 응답 시간이 초과되었습니다.".to_string()),
            );
        }
    }

    pub(in crate::app) fn on_agent_extract_done(
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
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
        let extracted: String = extracted.chars().take(12_000).collect();
        let summary: String = summary.chars().take(300).collect();
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

    pub(super) fn capture_agent_snapshot_and_run(&mut self, req_id: u64, allow_load_wait: bool) {
        self.ensure_active_valid();
        let active_tab_id = self.tabs[self.active_tab].id;
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        if run.target_tab_id != active_tab_id {
            self.finish_agent_run(Some("사용자가 다른 탭으로 이동해 제어를 중단했습니다.".to_string()));
            return;
        }
        let goal = run.goal.clone();
        let extracted_opt = run.extracted_data.take();
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

        if active.is_pdf() {
            if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                run.extracted_data = extracted_opt;
                run.status = "PDF 본문 읽는 중…".to_string();
            }
            let bus = self.bus.clone();
            let cookies = Arc::clone(&self.net_provider.cookies);
            let title_c = active_title;
            let url_c = active_url;
            let cancel = self
                .chat_inflight
                .as_ref()
                .filter(|(id, _, _)| *id == req_id)
                .map(|(_, c, _)| Arc::clone(c))
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            let handle = tokio::spawn(async move {
                let extracted = match crate::pdfjs::load_pdf_bytes(&url_c, cookies).await {
                    Ok(bytes) => {
                        match tokio::task::spawn_blocking(move || crate::pdfjs::extract_pdf_text(&bytes, 14_000)).await
                        {
                            Ok(Ok(s)) if !s.trim().is_empty() => s,
                            Ok(Ok(_)) => {
                                "[이 PDF에는 추출 가능한 텍스트 레이어가 없습니다. 스캔된 이미지 PDF일 수 있습니다.]"
                                    .to_string()
                            }
                            Ok(Err(e)) => format!("[PDF 텍스트를 읽지 못했습니다: {e}]"),
                            Err(e) => format!("[PDF 텍스트 추출 작업 실패: {e}]"),
                        }
                    }
                    Err(e) => format!("[PDF 파일을 불러오지 못했습니다: {e}]"),
                };
                bus.send(AppEvent::AgentPdfSnapshot {
                    req_id,
                    title: title_c,
                    url: url_c,
                    extracted,
                });
            });
            self.chat_inflight = Some((req_id, cancel, handle.abort_handle()));
            self.last_sidebar_html.clear();
            self.rebuild_sidebar_now(true);
            return;
        }

        if self.active_webview.is_some() {
            self.arm_chat_script_timeout(req_id);
        }
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
                self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
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

    pub(in crate::app) fn on_agent_pdf_snapshot(&mut self, req_id: u64, title: String, url: String, extracted: String) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        let goal = run.goal.clone();
        let prev_extracted = run.extracted_data.take();
        let tabs_summary = self.format_open_tabs_summary();
        let mut text = format!("{tabs_summary}\n[PDF DOCUMENT TEXT]\n{extracted}");
        if let Some(ext) = prev_extracted {
            text.push_str("\n\n[EXTRACTED DATA]\n");
            text.push_str(&ext);
        }
        let ctx = PageContext { title, url, text };
        self.start_chat_task(req_id, goal, Some(ctx));
    }

    pub(in crate::app) fn on_agent_step_ready(&mut self, req_id: u64) {
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

    pub(super) fn poll_agent_text_wait(&mut self, req_id: u64, wait_gen: u64) {
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
        if self.active_webview.is_some() {
            self.arm_chat_script_timeout(req_id);
        }
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
            self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
            if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                run.awaiting_text = None;
            }
            self.on_agent_action_done(req_id, false, format!("{} (스크립트 실패)", wait.action_summary), 0);
        }
    }

    pub(in crate::app) fn on_agent_wait_check(&mut self, req_id: u64, wait_gen: u64, found: bool) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
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

    pub(in crate::app) fn on_agent_action_done(&mut self, req_id: u64, ok: bool, summary: String, need_delay_ms: u64) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        self.chat_script_seq = self.chat_script_seq.wrapping_add(1);
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
        let status_prefix = if ok { "OK" } else { "ERR" };
        run.action_log.push(format!("{status_prefix}: {summary}"));
        // A failure's message says why; a success keeps the action it started with.
        let detail = if ok { String::new() } else { summary.clone() };
        self.live_agent_step(req_id, "", &detail, Some(ok));
        let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) else {
            return;
        };
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

    pub(super) fn finish_agent_run(&mut self, override_summary: Option<String>) {
        let Some(run) = self.agent_run.take() else {
            self.chat_inflight = None;
            return;
        };
        self.chat_inflight = None;
        let summary = override_summary.unwrap_or(run.finish_summary);
        let text = format_agent_final_message(&run.finish_cleaned, &summary, &[]);
        let req_id = run.req_id;
        let provider_label = (!run.provider_label.is_empty()).then_some(run.provider_label);
        self.chat_messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text,
            page_title: None,
            provider_label,
            activity: Vec::new(),
            elapsed_ms: None,
        });
        self.persist_chat();
        self.finish_live(req_id);
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
    }

    pub(super) fn run_next_queued_agent_action(&mut self, req_id: u64) {
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
        let kind = action.kind();
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(true);
        self.live_agent_step(req_id, kind, &desc, None);
        self.execute_agent_action(req_id, action, desc, is_final, has_more_queued);
    }

    /// Hosts the agent may open without asking: the starting tab's site, a host token explicitly
    /// named in the user's request, or one already approved in this run.
    pub(super) fn agent_host_trusted(&self, req_id: u64, host: &str) -> bool {
        let Some(run) = self.agent_run.as_ref().filter(|r| r.req_id == req_id) else {
            return false;
        };
        is_agent_host_trusted(host, &run.trusted_hosts, &run.goal)
    }

    pub(super) fn ask_agent_confirm(
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
        run.status = format!("확인 필요 · {host}");
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

    pub(super) fn resolve_agent_confirm(&mut self, allow: bool) {
        let Some((req_id, pending)) = self
            .agent_run
            .as_mut()
            .and_then(|r| r.awaiting_confirm.take().map(|p| (r.req_id, p)))
        else {
            return;
        };
        if !allow {
            self.finish_agent_run(Some(format!("{} 동작을 사용자가 거부하여 중단했습니다.", pending.host)));
            return;
        }
        if matches!(
            pending.action,
            AgentAction::Navigate { .. } | AgentAction::OpenTab { .. } | AgentAction::SwitchTab { .. }
        ) && let Some(run) = self.agent_run.as_mut()
        {
            run.trusted_hosts.push(normalize_agent_host(&pending.host));
        }
        self.execute_agent_action_inner(
            req_id,
            pending.action,
            pending.desc,
            pending.is_final,
            pending.has_more,
            true,
        );
    }

    pub(super) fn execute_agent_action(
        &mut self,
        req_id: u64,
        action: AgentAction,
        desc: String,
        is_final: bool,
        has_more_queued: bool,
    ) {
        self.execute_agent_action_inner(req_id, action, desc, is_final, has_more_queued, false);
    }

    pub(super) fn execute_agent_action_inner(
        &mut self,
        req_id: u64,
        action: AgentAction,
        desc: String,
        is_final: bool,
        has_more_queued: bool,
        confirmed: bool,
    ) {
        self.ensure_active_valid();
        let active_tab_id = self.tabs[self.active_tab].id;
        if self
            .agent_run
            .as_ref()
            .filter(|r| r.req_id == req_id)
            .is_some_and(|r| r.target_tab_id != active_tab_id)
        {
            self.finish_agent_run(Some("사용자가 다른 탭으로 이동해 제어를 중단했습니다.".to_string()));
            return;
        }
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
            if !confirmed && !self.agent_host_trusted(req_id, &host) {
                self.ask_agent_confirm(req_id, action.clone(), desc, host, is_final, has_more_queued);
                return;
            }
        }
        if !confirmed
            && let AgentAction::SwitchTab { tab } = &action
            && let Some(target_tab) = self.tabs.get(*tab)
            && let Some(host) = url::Url::parse(&target_tab.url)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            && !self.agent_host_trusted(req_id, &host)
        {
            self.ask_agent_confirm(req_id, action.clone(), desc, host, is_final, has_more_queued);
            return;
        }
        if !confirmed && let AgentAction::Extract { copy: true, .. } = &action {
            self.ask_agent_confirm(
                req_id,
                action.clone(),
                desc,
                "클립보드 복사".to_string(),
                is_final,
                has_more_queued,
            );
            return;
        }
        match &action {
            AgentAction::Navigate { url } => {
                let mut target = url.trim().to_string();
                if self.prefs.https_only
                    && let Ok(mut parsed) = url::Url::parse(&target)
                    && parsed.scheme() == "http"
                    && parsed.set_scheme("https").is_ok()
                {
                    target = parsed.to_string();
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
                if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                    run.target_tab_id = tab_id;
                }
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
                if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                    run.target_tab_id = tab_id;
                }
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
                let closed_active = *tab == self.active_tab;
                self.close_tab(*tab, None);
                self.ensure_active_valid();
                let new_active_id = self.tabs[self.active_tab].id;
                if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                    run.target_tab_id = new_active_id;
                    if closed_active {
                        // Active tab changed: drop all queued actions planned for the closed tab.
                        run.pending_actions.clear();
                        run.finish_after_queue = false;
                    } else if let Some(cut) = run
                        .pending_actions
                        .iter()
                        .position(|a| matches!(a, AgentAction::SwitchTab { .. } | AgentAction::CloseTab { .. }))
                    {
                        run.pending_actions.truncate(cut);
                        run.finish_after_queue = false;
                    }
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
                        let msg: String = parsed
                            .as_ref()
                            .and_then(|v| v.get("msg"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("추출 실패")
                            .chars()
                            .take(200)
                            .collect();
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
                } else {
                    self.arm_chat_script_timeout(req_id);
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
                if self.active_webview.is_some() {
                    let desc_c = desc.clone();
                    let res = self
                        .active_webview
                        .as_ref()
                        .expect("checked above")
                        .evaluate_script_with_callback(&js, move |raw| {
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
                            let msg = parsed
                                .as_ref()
                                .and_then(|v| v.get("msg"))
                                .and_then(|v| v.as_str())
                                .map(|m| {
                                    if ok {
                                        desc_c.clone()
                                    } else {
                                        let m: String = m.chars().take(200).collect();
                                        format!("{desc_c} ({m})")
                                    }
                                })
                                .unwrap_or_else(|| {
                                    if ok {
                                        desc_c.clone()
                                    } else {
                                        format!("{desc_c} (스크립트 응답 파싱 실패)")
                                    }
                                });
                            bus.send(AppEvent::AgentActionDone {
                                req_id,
                                ok,
                                summary: msg,
                                need_delay_ms: settle_ms,
                            });
                        });
                    if res.is_err() {
                        self.on_agent_action_done(req_id, false, format!("{desc} (스크립트 실행 실패)"), 0);
                    } else {
                        self.arm_chat_script_timeout(req_id);
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

pub(super) fn format_agent_final_message(cleaned: &str, summary: &str, log: &[String]) -> String {
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

pub(super) fn user_text_fallback(msgs: &[ChatMessage]) -> String {
    msgs.last().map(|m| m.text.clone()).unwrap_or_default()
}

pub(super) fn normalize_agent_host(host: &str) -> String {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    lower.strip_prefix("www.").unwrap_or(&lower).to_string()
}

pub(crate) fn extract_goal_hosts(goal: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in goal
        .split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';'))
    {
        let token = raw.trim_matches(|c: char| matches!(c, '.' | ':' | '!' | '?'));
        if token.is_empty() {
            continue;
        }
        let candidate = if token.to_ascii_lowercase().starts_with("http://")
            || token.to_ascii_lowercase().starts_with("https://")
        {
            url::Url::parse(token)
                .ok()
                .and_then(|u| u.host_str().map(normalize_agent_host))
        } else {
            let host_part = token.split(['/', '?', '#']).next().unwrap_or(token);
            let host_no_port = host_part.split(':').next().unwrap_or(host_part);
            let labels: Vec<&str> = host_no_port.split('.').collect();
            let valid_domain = labels.len() >= 2
                && labels.iter().all(|l| {
                    !l.is_empty()
                        && !l.starts_with('-')
                        && !l.ends_with('-')
                        && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                && labels
                    .last()
                    .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
            valid_domain.then(|| normalize_agent_host(host_no_port))
        };
        if let Some(h) = candidate
            && !h.is_empty()
            && !out.contains(&h)
        {
            out.push(h);
        }
    }
    out
}

pub(crate) fn is_agent_host_trusted(host: &str, trusted_hosts: &[String], goal: &str) -> bool {
    let want = normalize_agent_host(host);
    if want.is_empty() {
        return false;
    }
    trusted_hosts.iter().any(|h| normalize_agent_host(h) == want) || extract_goal_hosts(goal).contains(&want)
}
