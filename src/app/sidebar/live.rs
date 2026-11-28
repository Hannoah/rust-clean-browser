//! Live "what is the agent doing" state for the request in flight, and attaching the finished
//! steps to the reply (rendered as "도구 N개 사용 ›" cards).

use crate::app::{AppEvent, BrowserApp};
use crate::chat::{ActivityStep, ChatRole, Progress, apply_step};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(in crate::app) struct LiveActivity {
    pub req_id: u64,
    pub started: Instant,
    /// Reply text has started arriving.
    pub writing: bool,
    pub steps: Vec<ActivityStep>,
    /// Stops the once-a-second label ticker.
    stop: Arc<AtomicBool>,
}

impl LiveActivity {
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

impl Drop for LiveActivity {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Status line text: the running tool, else the control-mode status, else writing/thinking.
pub(in crate::app) fn live_label(live: &LiveActivity, agent_status: Option<&str>) -> String {
    let secs = live.started.elapsed().as_secs();
    let base = if let Some(s) = live.steps.iter().rev().find(|s| s.running) {
        if s.detail.is_empty() {
            format!("@{} 실행 중", s.name)
        } else {
            format!("@{} 실행 중 · {}", s.name, s.detail)
        }
    } else if let Some(st) = agent_status {
        st.trim_end_matches('…').to_string()
    } else if live.writing {
        "응답 작성 중".to_string()
    } else {
        "생각 중".to_string()
    };
    if secs < 3 {
        format!("{base}…")
    } else {
        format!("{base} ({secs}s)…")
    }
}

impl BrowserApp {
    /// Starts tracking `req_id` (kept across the steps of one control-mode run).
    pub(in crate::app) fn begin_live(&mut self, req_id: u64) {
        if self.chat_live.as_ref().is_some_and(|l| l.req_id == req_id) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.chat_live = Some(LiveActivity {
            req_id,
            started: Instant::now(),
            writing: false,
            steps: Vec::new(),
            stop: Arc::clone(&stop),
        });
        let bus = self.bus.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if stop.load(Ordering::Acquire) {
                    break;
                }
                bus.send(AppEvent::ChatLiveTick(req_id));
            }
        });
    }

    pub(in crate::app) fn on_chat_progress(&mut self, req_id: u64, p: Progress) {
        let Some(live) = self.chat_live.as_mut().filter(|l| l.req_id == req_id) else {
            return;
        };
        match p {
            Progress::Writing if live.writing => return,
            Progress::Writing => live.writing = true,
            Progress::Step(step) => {
                let at = live.elapsed_ms();
                apply_step(&mut live.steps, step, at);
            }
        }
        self.last_sidebar_html.clear();
        self.rebuild_sidebar(false);
    }

    /// Browser-control action started (`running`) or finished.
    pub(in crate::app) fn live_agent_step(&mut self, req_id: u64, name: &str, detail: &str, done: Option<bool>) {
        let step = ActivityStep {
            name: name.to_string(),
            detail: detail.to_string(),
            ok: done.unwrap_or(false),
            running: done.is_none(),
            ..Default::default()
        };
        self.on_chat_progress(req_id, Progress::Step(step));
    }

    /// Once a second: refresh just the status line text (no full rebuild).
    pub(in crate::app) fn on_chat_live_tick(&mut self, req_id: u64) {
        let Some(live) = self.chat_live.as_ref().filter(|l| l.req_id == req_id) else {
            return;
        };
        let status = self.agent_run.as_ref().map(|r| r.status.as_str());
        let label = crate::cleaner::html_escape(&live_label(live, status));
        let anim_time = self.anim_time();
        if let Some(doc) = &mut self.sidebar_doc
            && let Some(id) = doc.get_element_by_id("act-live-txt")
        {
            doc.mutate().set_inner_html(id, &label);
            doc.resolve(anim_time);
            self.request_redraw();
        }
    }

    /// The request ended: move its steps and duration onto the reply it produced.
    pub(in crate::app) fn finish_live(&mut self, req_id: u64) {
        if self.chat_inflight.is_some() || self.chat_live.as_ref().is_none_or(|l| l.req_id != req_id) {
            return;
        }
        let Some(mut live) = self.chat_live.take() else {
            return;
        };
        let total = live.elapsed_ms();
        for s in live.steps.iter_mut().filter(|s| s.running) {
            s.running = false;
            s.ms = total.saturating_sub(s.ms);
        }
        if let Some(msg) = self.chat_messages.last_mut()
            && msg.role == ChatRole::Assistant
            && msg.elapsed_ms.is_none()
        {
            msg.activity = std::mem::take(&mut live.steps);
            msg.elapsed_ms = Some(total);
            self.persist_chat();
            self.last_sidebar_html.clear();
            self.rebuild_sidebar(false);
        }
    }
}
