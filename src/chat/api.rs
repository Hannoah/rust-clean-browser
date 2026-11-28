use serde_json::{Value, json};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::context::{format_user_turn, neutralize_untrusted};
use super::prompt::{format_agent_instructions, format_agent_state_block};
use super::{
    AgentContext, ChatConfig, ChatMessage, ChatProvider, ChatRole, MAX_HISTORY_TURNS, MAX_TURN_CHARS, PageContext,
    SYSTEM_PROMPT, is_valid_model_id,
};

/// `localhost` / `127.x` / `::1` endpoint (host compared exactly, not by string prefix).
pub fn is_loopback_endpoint(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

pub(super) static API_HTTP_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

pub(super) fn api_http_client() -> Result<&'static reqwest::Client, String> {
    match API_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .pool_idle_timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("HTTP 클라이언트 생성 실패: {e}"))
    }) {
        Ok(client) => Ok(client),
        Err(err) => Err(err.clone()),
    }
}

pub(super) fn validate_api_endpoint_url(endpoint: &str, api_key: &str) -> Result<(url::Url, bool), String> {
    let parsed = url::Url::parse(endpoint)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
        .ok_or_else(|| "API 엔드포인트는 http:// 또는 https://로 시작해야 합니다.".to_string())?;
    let is_local = is_loopback_endpoint(&parsed);
    if parsed.scheme() == "http" && !is_local && !api_key.is_empty() {
        return Err("API 키는 암호화되지 않은 http:// 주소로 보내지 않습니다. https:// 엔드포인트를 쓰거나 로컬(localhost) 주소만 http로 사용하세요.".to_string());
    }
    Ok((parsed, is_local))
}

/// Extracts one streamed text delta from a single SSE `data:` payload (without the
/// `data:` prefix). Returns `None` for control payloads (`[DONE]`, role/usage
/// events) and for payloads that carry no text.
pub(super) fn extract_sse_delta(provider: ChatProvider, data: &str) -> Option<String> {
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    let val: Value = serde_json::from_str(data).ok()?;
    match provider {
        ChatProvider::OpenAiApi => val
            .pointer("/choices/0/delta/content")
            .or_else(|| val.pointer("/choices/0/text"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        ChatProvider::AnthropicApi => {
            let is_text_delta = val.get("type").and_then(|v| v.as_str()) == Some("content_block_delta");
            if is_text_delta {
                val.pointer("/delta/text").and_then(|v| v.as_str()).map(str::to_string)
            } else {
                None
            }
        }
        ChatProvider::GeminiApi => {
            // `streamGenerateContent?alt=sse` emits one JSON object per line; be lenient
            // and also accept a bare array of such objects.
            let objs: Vec<&Value> = match &val {
                Value::Array(arr) => arr.iter().collect(),
                obj => vec![obj],
            };
            let mut out = String::new();
            for obj in objs {
                if let Some(parts) = obj.pointer("/candidates/0/content/parts").and_then(|v| v.as_array()) {
                    for part in parts {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            out.push_str(t);
                        }
                    }
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        _ => None,
    }
}

/// Reads an SSE response chunk by chunk (`Response::chunk` needs no extra stream
/// combinator deps), forwarding text deltas through `stream_cb` as they arrive so
/// the UI paints while the model is still generating. Returns the assembled reply.
async fn stream_sse_reply(
    provider: ChatProvider,
    mut resp: reqwest::Response,
    cancel: &AtomicBool,
    stream_cb: &Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
) -> Result<String, String> {
    let mut line_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut full = String::new();
    let mut done = false;
    while !done {
        if cancel.load(Ordering::Acquire) {
            return Err("요청이 중지되었습니다.".to_string());
        }
        match resp.chunk().await.map_err(|e| format!("응답 읽기 실패: {e}"))? {
            Some(bytes) => {
                for &b in bytes.iter() {
                    if b == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        let line = line.trim();
                        if let Some(payload) = line.strip_prefix("data:") {
                            let payload = payload.trim();
                            if payload == "[DONE]" {
                                done = true;
                                break;
                            }
                            if let Some(delta) = extract_sse_delta(provider, payload)
                                && !delta.is_empty()
                            {
                                full.push_str(&delta);
                                if let Some(cb) = stream_cb {
                                    cb(delta);
                                }
                            }
                        }
                        line_buf.clear();
                    } else if b != b'\r' {
                        line_buf.push(b);
                        if line_buf.len() > 256 * 1024 {
                            return Err("API 스트리밍 한 줄이 너무 깁니다.".to_string());
                        }
                    }
                }
            }
            None => break,
        }
    }
    match full.trim().to_string().as_str() {
        "" => Err("API 응답에서 텍스트를 추출하지 못했습니다.".to_string()),
        text => Ok(text.to_string()),
    }
}

pub(super) async fn run_api_chat(
    config: &ChatConfig,
    history: &[ChatMessage],
    user_text: &str,
    page: Option<&PageContext>,
    agent: Option<&AgentContext>,
    cancel: &AtomicBool,
    stream_cb: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
) -> Result<String, String> {
    let endpoint = config.effective_endpoint().to_string();
    let model = if agent.is_some() {
        config.effective_control_model(config.provider)
    } else {
        config.selected_model(config.provider)
    };
    if !is_valid_model_id(&model) {
        return Err("유효하지 않은 모델 ID입니다.".to_string());
    }
    let api_key = config.effective_api_key().to_string();

    let (_parsed, is_local) = validate_api_endpoint_url(&endpoint, &api_key)?;
    if api_key.is_empty() && !is_local {
        return Err(format!(
            "{} 키가 설정되지 않았습니다. 우측 상단 설정(⚙)에서 API 키를 입력하세요.",
            config.provider.label()
        ));
    }

    let client = api_http_client()?;

    let (max_turns, max_turn_chars) = if agent.is_some() {
        (2, MAX_TURN_CHARS.min(600))
    } else {
        (MAX_HISTORY_TURNS, MAX_TURN_CHARS)
    };
    let turns: Vec<&ChatMessage> = history
        .iter()
        .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
        .rev()
        .take(max_turns)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let user_turn = format_user_turn(user_text, page);
    let final_user = if let Some(ctx) = agent {
        format!("{}{}", format_agent_state_block(ctx), user_turn)
    } else {
        user_turn
    };
    let system_prompt = if let Some(ctx) = agent {
        format!("{SYSTEM_PROMPT}\n\n{}", format_agent_instructions(ctx))
    } else {
        SYSTEM_PROMPT.to_string()
    };

    let (target_url, body_bytes, builder) = match config.provider {
        ChatProvider::OpenAiApi => {
            let mut msgs = vec![json!({ "role": "system", "content": &system_prompt })];
            for m in turns {
                let role = if m.role == ChatRole::User { "user" } else { "assistant" };
                let txt: String = neutralize_untrusted(&m.text).chars().take(max_turn_chars).collect();
                msgs.push(json!({ "role": role, "content": txt }));
            }
            msgs.push(json!({ "role": "user", "content": final_user }));
            let payload = json!({ "model": model, "messages": msgs, "stream": true });
            let bytes = serde_json::to_vec(&payload).map_err(|e| format!("JSON 직렬화 실패: {e}"))?;
            let mut req = client.post(&endpoint).header("Content-Type", "application/json");
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {api_key}"));
            }
            (endpoint, bytes, req)
        }
        ChatProvider::AnthropicApi => {
            let mut msgs = Vec::new();
            for m in turns {
                let role = if m.role == ChatRole::User { "user" } else { "assistant" };
                let txt: String = neutralize_untrusted(&m.text).chars().take(max_turn_chars).collect();
                msgs.push(json!({ "role": role, "content": txt }));
            }
            msgs.push(json!({ "role": "user", "content": final_user }));
            let payload = json!({
                "model": model,
                "max_tokens": if agent.is_some() { 768 } else { 2048 },
                "stream": true,
                "system": [{
                    "type": "text",
                    "text": &system_prompt,
                    "cache_control": { "type": "ephemeral" }
                }],
                "messages": msgs
            });
            let bytes = serde_json::to_vec(&payload).map_err(|e| format!("JSON 직렬화 실패: {e}"))?;
            let req = client
                .post(&endpoint)
                .header("Content-Type", "application/json")
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
            (endpoint, bytes, req)
        }
        ChatProvider::GeminiApi => {
            let base = endpoint.trim_end_matches('/');
            let stream_url = if let Some(prefix) = base.strip_suffix(":generateContent") {
                format!("{prefix}:streamGenerateContent")
            } else {
                format!("{base}/{model}:streamGenerateContent")
            };
            // `alt=sse` makes the streaming response SSE-framed (`data: {...}` per line).
            let full_url = format!(
                "{stream_url}{}alt=sse",
                if stream_url.contains('?') { "&" } else { "?" }
            );
            let _ = validate_api_endpoint_url(&full_url, &api_key)?;
            let mut contents = Vec::new();
            for m in turns {
                let role = if m.role == ChatRole::User { "user" } else { "model" };
                let txt: String = m.text.chars().take(max_turn_chars).collect();
                contents.push(json!({ "role": role, "parts": [{ "text": txt }] }));
            }
            contents.push(json!({ "role": "user", "parts": [{ "text": final_user }] }));
            let payload = json!({
                "system_instruction": { "parts": [{ "text": &system_prompt }] },
                "contents": contents
            });
            let bytes = serde_json::to_vec(&payload).map_err(|e| format!("JSON 직렬화 실패: {e}"))?;
            let req = client
                .post(&full_url)
                .header("Content-Type", "application/json")
                .header("x-goog-api-key", &api_key);
            (full_url, bytes, req)
        }
        _ => return Err("잘못된 API 제공자입니다.".to_string()),
    };

    let resp = builder
        .body(body_bytes)
        .send()
        .await
        .map_err(|e| format!("API 연결 실패 ({target_url}): {e}"))?;
    if cancel.load(Ordering::Acquire) {
        return Err("요청이 중지되었습니다.".to_string());
    }
    let status = resp.status();
    // Stream (`text/event-stream`) when the caller wants live deltas and the server
    // honors `stream: true`; otherwise fall back to the buffered full-JSON read.
    let wants_stream = stream_cb.is_some();
    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));
    if wants_stream && is_sse {
        return stream_sse_reply(config.provider, resp, cancel, &stream_cb).await;
    }
    let _ = stream_cb;
    let resp_bytes = resp.bytes().await.map_err(|e| format!("응답 읽기 실패: {e}"))?;
    if !status.is_success() {
        let preview: String = String::from_utf8_lossy(&resp_bytes).chars().take(360).collect();
        return Err(format!("API 오류 HTTP {status}: {preview}"));
    }
    let val: Value = serde_json::from_slice(&resp_bytes).map_err(|e| format!("JSON 파싱 실패: {e}"))?;

    let reply = match config.provider {
        ChatProvider::OpenAiApi => val
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        ChatProvider::AnthropicApi => val.get("content").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        }),
        ChatProvider::GeminiApi => val
            .pointer("/candidates/0/content/parts")
            .and_then(|v| v.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        _ => None,
    };

    match reply.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(text) => Ok(text),
        None => Err("API 응답에서 텍스트를 추출하지 못했습니다.".to_string()),
    }
}
