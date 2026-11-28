//! What an agent is doing right now (live status line) and what it did (tool cards under a reply).
//! Fed by the stream events the CLI workers emit, the API stream, and the browser-control loop.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// One tool call / browser action, shown as a card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityStep {
    /// Tool or action name (`read_file`, `click`, …), shown in monospace.
    pub name: String,
    /// Short human summary of the input (a file name, a URL, the element clicked).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ms: u64,
    /// Correlates a start event with its end event (step index / tool_use id). Not persisted.
    #[serde(skip)]
    pub key: String,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// A progress event from a running request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Reply text started arriving.
    Writing,
    /// A tool started (`running`) or finished. Finishes may omit `name` (Claude's tool_result).
    Step(ActivityStep),
}

pub type ProgressCb = Arc<dyn Fn(Progress) + Send + Sync>;

/// Short summary of a tool's input: the first string parameter, paths reduced to the file name.
fn summarize_input(params: Option<&Value>) -> String {
    let Some(obj) = params.and_then(Value::as_object) else {
        return String::new();
    };
    let s = obj.values().find_map(Value::as_str).unwrap_or("").trim();
    let s = if s.contains(['\\', '/']) && !s.contains("://") && !s.contains(' ') {
        s.rsplit(['\\', '/']).next().unwrap_or(s)
    } else {
        s
    };
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one_line.chars().take(60).collect();
    if one_line.chars().count() > 60 {
        out.push('…');
    }
    out
}

/// `agy --output-format stream-json` line → progress (`step_update` events).
pub(crate) fn parse_agy_progress(line: &str) -> Option<Progress> {
    let val: Value = serde_json::from_str(line.trim()).ok()?;
    if val.get("event")?.as_str()? != "step_update" {
        return None;
    }
    let su = val.get("step_update")?;
    let state = su.get("state").and_then(Value::as_str).unwrap_or("");
    match su.get("step_type").and_then(Value::as_str)? {
        "agent_response"
            if su
                .get("text_delta")
                .and_then(Value::as_str)
                .is_some_and(|t| !t.trim().is_empty()) =>
        {
            Some(Progress::Writing)
        }
        "tool" => {
            let name = su
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let running = state == "ACTIVE";
            Some(Progress::Step(ActivityStep {
                detail: summarize_input(su.pointer("/tool_info/parameters")),
                ok: state == "DONE",
                running,
                ms: su
                    .get("duration_seconds")
                    .and_then(Value::as_f64)
                    .map_or(0, |s| (s * 1000.0) as u64),
                key: su.get("step_index").map(|v| v.to_string()).unwrap_or_default(),
                name,
            }))
        }
        _ => None,
    }
}

/// `claude --output-format stream-json` line → progress (`tool_use` / `tool_result` / text).
pub(crate) fn parse_claude_progress(line: &str) -> Vec<Progress> {
    let Ok(val) = serde_json::from_str::<Value>(line.trim()) else {
        return Vec::new();
    };
    let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
    let items = val.pointer("/message/content").and_then(Value::as_array);
    let mut out = Vec::new();
    for item in items.into_iter().flatten() {
        match (kind, item.get("type").and_then(Value::as_str)) {
            ("assistant", Some("text"))
                if item
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !t.trim().is_empty()) =>
            {
                out.push(Progress::Writing);
            }
            ("assistant", Some("tool_use")) => out.push(Progress::Step(ActivityStep {
                name: item.get("name").and_then(Value::as_str).unwrap_or("tool").to_string(),
                detail: summarize_input(item.get("input")),
                running: true,
                key: item.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                ..Default::default()
            })),
            ("user", Some("tool_result")) => out.push(Progress::Step(ActivityStep {
                ok: !item.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                key: item
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                ..Default::default()
            })),
            _ => {}
        }
    }
    out
}

/// Applies a step event to a list: a finish updates its matching running card (keeping the
/// name/detail it started with), anything else is appended.
pub fn apply_step(steps: &mut Vec<ActivityStep>, step: ActivityStep, elapsed_ms: u64) {
    if !step.running
        && let Some(prev) = steps
            .iter_mut()
            .rev()
            .find(|s| s.running && (s.key == step.key || step.key.is_empty()))
    {
        prev.running = false;
        prev.ok = step.ok;
        prev.ms = if step.ms > 0 {
            step.ms
        } else {
            elapsed_ms.saturating_sub(prev.ms)
        };
        if !step.detail.is_empty() {
            prev.detail = step.detail;
        }
        return;
    }
    let mut step = step;
    if step.running {
        // While running, `ms` holds the start offset so the finish can compute a duration.
        step.ms = elapsed_ms;
    }
    steps.push(step);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agy_tool_steps_and_text() {
        // Captured from `agy --output-format stream-json` (paths shortened).
        let start = r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"Path":"C:\\tmp\\evt\\note.txt"}}}}"#;
        let done = r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_name":"read_file","duration_seconds":0.48,"tool_info":{"name":"read_file","parameters":{"Path":"C:\\tmp\\evt\\note.txt"}}}}"#;
        let text = r#"{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"pong"}}"#;
        let mut steps = Vec::new();
        let Some(Progress::Step(s)) = parse_agy_progress(start) else {
            panic!("start")
        };
        assert_eq!(
            (s.name.as_str(), s.detail.as_str(), s.running),
            ("read_file", "note.txt", true)
        );
        apply_step(&mut steps, s, 1000);
        let Some(Progress::Step(d)) = parse_agy_progress(done) else {
            panic!("done")
        };
        apply_step(&mut steps, d, 1600);
        assert_eq!(steps.len(), 1);
        assert!(steps[0].ok && !steps[0].running && steps[0].ms == 480);
        assert_eq!(parse_agy_progress(text), Some(Progress::Writing));
        assert_eq!(parse_agy_progress(r#"{"event":"result","result":{}}"#), None);
    }

    #[test]
    fn claude_tool_use_then_result() {
        let use_line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"WebFetch","input":{"url":"https://example.com/a"}}]}}"#;
        let res_line =
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true}]}}"#;
        let mut steps = Vec::new();
        for p in parse_claude_progress(use_line)
            .into_iter()
            .chain(parse_claude_progress(res_line))
        {
            if let Progress::Step(s) = p {
                apply_step(&mut steps, s, 2000);
            }
        }
        assert_eq!(steps.len(), 1);
        assert_eq!(
            (steps[0].name.as_str(), steps[0].detail.as_str()),
            ("WebFetch", "https://example.com/a")
        );
        assert!(!steps[0].ok && !steps[0].running);
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parse_claude_progress(text), vec![Progress::Writing]);
    }
}
