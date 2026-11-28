//! Executors for the browser agent's data tools. Results go through `on_agent_extract_done`, so
//! the next round sees them as [EXTRACTED DATA] (same channel as `extract`).

use crate::app::{AppEvent, BrowserApp};
use crate::chat::{AgentAction, extract_page_context_from_html};
use std::time::{Duration, Instant};

fn parse_js_json(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| match v {
            serde_json::Value::String(s) => serde_json::from_str(&s).ok(),
            other => Some(other),
        })
}

fn nonce() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
}

impl BrowserApp {
    fn agent_data(&mut self, req_id: u64, ok: bool, summary: String, data: String) {
        self.on_agent_extract_done(req_id, ok, summary, data, false);
    }

    /// Handles the data tools; returns `false` for any other action.
    pub(super) fn execute_agent_tool(&mut self, req_id: u64, action: &AgentAction, desc: String) -> bool {
        let js_deadline = |secs| Instant::now() + Duration::from_secs(secs);
        match action {
            AgentAction::FindInPage { text } => {
                self.run_agent_data_js(req_id, crate::chat::find_in_page_js(text), desc, js_deadline(4));
            }
            AgentAction::PdfPage { page } => {
                self.run_agent_data_js(req_id, crate::chat::pdf_page_js(*page, nonce()), desc, js_deadline(15));
            }
            AgentAction::PdfSearch { text } => {
                self.run_agent_data_js(req_id, crate::chat::pdf_search_js(text, nonce()), desc, js_deadline(30));
            }
            AgentAction::FillForm { fields } => {
                self.run_agent_data_js(req_id, crate::chat::fill_form_js(fields), desc, js_deadline(4));
            }
            AgentAction::WaitForNetworkIdle { timeout_ms } => {
                if self.active_webview.is_none() {
                    // Clean-mode page: the app itself knows when loading is over.
                    let ok = !self.tabs[self.active_tab].is_loading;
                    self.on_agent_action_done(req_id, ok, desc, if ok { 0 } else { 800 });
                } else {
                    let deadline = Instant::now() + Duration::from_millis(*timeout_ms);
                    self.run_agent_data_js(req_id, crate::chat::network_idle_js(nonce()), desc, deadline);
                }
            }
            AgentAction::BookmarksSearch { query } => {
                let hits = self.bookmarks.search(query);
                let data = if hits.is_empty() {
                    format!("북마크에서 \"{query}\" 결과 없음")
                } else {
                    hits.iter()
                        .take(15)
                        .map(|b| format!("- {} — {}", b.title, b.url))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                self.agent_data(req_id, true, format!("{desc}: {}건", hits.len().min(15)), data);
            }
            AgentAction::HistorySearch { query } => {
                let hits = self.history.search_history(query, 15);
                let data = if hits.is_empty() {
                    format!("방문 기록에서 \"{query}\" 결과 없음")
                } else {
                    hits.iter()
                        .map(|h| format!("- {} — {} ({}회)", h.title, h.url, h.count))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                self.agent_data(req_id, true, format!("{desc}: {}건", hits.len()), data);
            }
            AgentAction::Cite { style } => {
                let tab = &self.tabs[self.active_tab];
                match crate::cite::citation(style, &tab.title, &tab.url, tab.raw_html.as_deref()) {
                    Some(c) => self.agent_data(req_id, true, desc, c),
                    None => self.agent_data(
                        req_id,
                        false,
                        format!("{desc} (이 페이지는 인용할 수 없습니다)"),
                        String::new(),
                    ),
                }
            }
            AgentAction::ReadTab { tab } => self.agent_read_tab(req_id, *tab, desc),
            AgentAction::Screenshot => self.agent_screenshot(req_id, desc),
            _ => return false,
        }
        true
    }

    /// Runs a tool script in the active WebView. `{pending:true}` → re-run until `deadline`.
    pub(in crate::app) fn run_agent_data_js(&mut self, req_id: u64, js: String, desc: String, deadline: Instant) {
        let Some(wv) = &self.active_webview else {
            self.on_agent_action_done(req_id, false, format!("{desc} (JS 모드 웹페이지가 필요합니다)"), 0);
            return;
        };
        let bus = self.bus.clone();
        let (js_c, desc_c) = (js.clone(), desc.clone());
        let res = wv.evaluate_script_with_callback(&js, move |raw| {
            let v = parse_js_json(&raw).unwrap_or_default();
            if v.get("pending").and_then(|p| p.as_bool()) == Some(true) {
                bus.send(AppEvent::AgentToolPending {
                    req_id,
                    js: js_c.clone(),
                    desc: desc_c.clone(),
                    deadline,
                });
                return;
            }
            let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
            let msg: String = v
                .get("msg")
                .and_then(|x| x.as_str())
                .unwrap_or(if ok { "" } else { "스크립트 응답 파싱 실패" })
                .chars()
                .take(200)
                .collect();
            let summary = match (ok, msg.is_empty()) {
                (true, true) => desc_c.clone(),
                (true, false) => format!("{desc_c}: {msg}"),
                (false, _) => format!("{desc_c} ({msg})"),
            };
            let data = v.get("data").and_then(|x| x.as_str()).unwrap_or("").to_string();
            bus.send(AppEvent::AgentExtractDone {
                req_id,
                ok,
                summary,
                extracted: data,
                copy: false,
            });
        });
        if res.is_err() {
            self.on_agent_action_done(req_id, false, format!("{desc} (스크립트 실행 실패)"), 0);
        } else {
            self.arm_chat_script_timeout(req_id);
        }
    }

    pub(in crate::app) fn on_agent_tool_pending(&mut self, req_id: u64, js: String, desc: String, deadline: Instant) {
        if self.chat_inflight.as_ref().is_none_or(|(id, _, _)| *id != req_id) {
            return;
        }
        if Instant::now() >= deadline {
            self.on_agent_action_done(req_id, false, format!("{desc} (시간 초과)"), 0);
            return;
        }
        let bus = self.bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            bus.send(AppEvent::AgentToolRerun {
                req_id,
                js,
                desc,
                deadline,
            });
        });
    }

    /// Another tab's text without switching: its live DOM / cached HTML, else its parked WebView.
    fn agent_read_tab(&mut self, req_id: u64, idx: usize, desc: String) {
        if idx >= self.tabs.len() {
            self.on_agent_action_done(req_id, false, format!("존재하지 않는 탭 인덱스 [{idx}]"), 0);
            return;
        }
        if idx == self.active_tab {
            self.on_agent_action_done(req_id, false, format!("{desc} (현재 탭입니다: extract를 쓰세요)"), 0);
            return;
        }
        let (title, url, id) = {
            let t = &self.tabs[idx];
            (t.title.clone(), t.url.clone(), t.id)
        };
        let key = url.split('#').next().unwrap_or(&url).to_string();
        let html = self.tabs[idx]
            .shown_html()
            .map(|h| h.to_string())
            .or_else(|| self.html_cache.get(&key).map(|c| c.raw_html.to_string()))
            .or_else(|| self.page_cache.get(&key).map(|c| c.html.to_string()));
        if let Some(html) = html {
            let ctx = extract_page_context_from_html(&html, &title, &url);
            let data = format!("[탭 {idx}] {title}\n{url}\n\n{}", ctx.text);
            self.agent_data(req_id, true, desc, data);
            return;
        }
        let Some((_, wv)) = self.parked_webviews.iter().find(|(tid, _)| *tid == id) else {
            self.on_agent_action_done(
                req_id,
                false,
                format!("{desc} (아직 불러오지 않은 탭입니다: switch_tab 후 extract)"),
                0,
            );
            return;
        };
        // Parked WebViews are suspended; wake it so the script can run (it stays hidden).
        crate::app::helpers::resume_webview(wv);
        let bus = self.bus.clone();
        let header = format!("[탭 {idx}] {title}\n{url}\n\n");
        let res = wv.evaluate_script_with_callback(
            "JSON.stringify({ok:true,data:(document.body?document.body.innerText:'').slice(0,12000)})",
            move |raw| {
                let v = parse_js_json(&raw).unwrap_or_default();
                let text = v.get("data").and_then(|x| x.as_str()).unwrap_or("");
                bus.send(AppEvent::AgentExtractDone {
                    req_id,
                    ok: !text.is_empty(),
                    summary: desc.clone(),
                    extracted: format!("{header}{text}"),
                    copy: false,
                });
            },
        );
        if res.is_err() {
            self.on_agent_action_done(req_id, false, "탭 읽기 스크립트 실행 실패".to_string(), 0);
        } else {
            self.arm_chat_script_timeout(req_id);
        }
    }

    /// Captures the visible page area; the next round's model call carries it as an image.
    fn agent_screenshot(&mut self, req_id: u64, desc: String) {
        let Some(hwnd) = self
            .window
            .as_ref()
            .and_then(|w| crate::app::helpers::window_hwnd(&**w))
        else {
            self.on_agent_action_done(req_id, false, format!("{desc} (창 핸들을 얻지 못했습니다)"), 0);
            return;
        };
        let (content_w, _) = self.content_phys_size();
        let top = self.chrome_phys_height();
        let h = self.win_size.height.saturating_sub(top);
        match crate::screenshot::capture_region_png(hwnd, 0, top as i32, content_w, h) {
            Ok(png) => {
                let kb = png.len() / 1024;
                if let Some(run) = self.agent_run.as_mut().filter(|r| r.req_id == req_id) {
                    run.screenshot = Some(crate::screenshot::base64(&png));
                }
                self.on_agent_action_done(req_id, true, format!("{desc} ({kb}KB, 다음 단계에 이미지로 전달)"), 0);
            }
            Err(e) => self.on_agent_action_done(req_id, false, format!("{desc} ({e})"), 0),
        }
    }
}
