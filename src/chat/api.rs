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

    let build_request = |streamed: bool| -> Result<(String, Vec<u8>, reqwest::RequestBuilder), String> {
        match config.provider {
            ChatProvider::OpenAiApi => {
                let mut msgs = vec![json!({ "role": "system", "content": &system_prompt })];
                for m in &turns {
                    let role = if m.role == ChatRole::User { "user" } else { "assistant" };
                    let txt: String = neutralize_untrusted(&m.text).chars().take(max_turn_chars).collect();
                    msgs.push(json!({ "role": role, "content": txt }));
                }
                msgs.push(json!({ "role": "user", "content": final_user.clone() }));
                let payload = json!({ "model": model.clone(), "messages": msgs.clone(), "stream": streamed });
                let bytes = serde_json::to_vec(&payload).map_err(|e| format!("JSON 직렬화 실패: {e}"))?;
                let mut req = client.post(&endpoint).header("Content-Type", "application/json");
                if !api_key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {api_key}"));
                }
                Ok((endpoint.clone(), bytes, req))
            }
            ChatProvider::AnthropicApi => {
                let mut msgs = Vec::new();
                for m in &turns {
                    let role = if m.role == ChatRole::User { "user" } else { "assistant" };
                    let txt: String = neutralize_untrusted(&m.text).chars().take(max_turn_chars).collect();
                    msgs.push(json!({ "role": role, "content": txt }));
                }
                msgs.push(json!({ "role": "user", "content": final_user.clone() }));
                let payload = json!({
                    "model": model.clone(),
                    "max_tokens": if agent.is_some() { 768 } else { 2048 },
                    "stream": streamed,
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
                Ok((endpoint.clone(), bytes, req))
            }
            ChatProvider::GeminiApi => {
                let full_url = gemini_url(&endpoint, &model, streamed)?;
                let _ = validate_api_endpoint_url(&full_url, &api_key)?;
                let mut contents = Vec::new();
                for m in &turns {
                    let role = if m.role == ChatRole::User { "user" } else { "model" };
                    let txt: String = m.text.chars().take(max_turn_chars).collect();
                    contents.push(json!({ "role": role, "parts": [{ "text": txt }] }));
                }
                contents.push(json!({ "role": "user", "parts": [{ "text": final_user.clone() }] }));
                let payload = json!({
                    "system_instruction": { "parts": [{ "text": &system_prompt }] },
                    "contents": contents
                });
                let bytes = serde_json::to_vec(&payload).map_err(|e| format!("JSON 직렬화 실패: {e}"))?;
                let req = client
                    .post(&full_url)
                    .header("Content-Type", "application/json")
                    .header("x-goog-api-key", &api_key);
                Ok((full_url.clone(), bytes, req))
            }
            _ => Err("잘못된 API 제공자입니다.".to_string()),
        }
    };

    // Some servers answer a streaming request with an empty 200 instead of SSE
    // or JSON. Retry once as plain JSON so such endpoints still work.
    let mut streamed = true;
    let (status, raw_body, full, got_delta) = loop {
        let (target_url, body_bytes, builder) = build_request(streamed)?;
        let resp = builder
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| format!("API 연결 실패 ({target_url}): {e}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("요청이 중지되었습니다.".to_string());
        }
        let (status, raw_body, full, got_delta) =
            read_response_body(config.provider, resp, cancel, stream_cb.as_deref()).await?;
        if status.is_success() && raw_body.iter().all(|b| b.is_ascii_whitespace()) && streamed {
            streamed = false;
            continue;
        }
        break (status, raw_body, full, got_delta);
    };
    if !status.is_success() {
        let preview: String = String::from_utf8_lossy(&raw_body).chars().take(360).collect();
        return Err(format!("API 오류 HTTP {status}: {preview}"));
    }
    if got_delta {
        return match full.trim().to_string().as_str() {
            "" => Err("API 응답에서 텍스트를 추출하지 못했습니다.".to_string()),
            text => Ok(text.to_string()),
        };
    }
    let val: Value = serde_json::from_slice(&raw_body).map_err(|e| format!("JSON 파싱 실패: {e}"))?;

    let reply = extract_buffered_reply(config.provider, &val);

    match reply.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(text) => Ok(text),
        None => Err("API 응답에서 텍스트를 추출하지 못했습니다.".to_string()),
    }
}

/// Incremental SSE framer: feed it one trimmed line at a time; it collects text
/// deltas and forwards them through `emit`. Works for label-correct streams and
/// mislabeled ones alike, since framing is sniffed from `data:` lines.
pub(super) struct SseStream {
    provider: ChatProvider,
    full: String,
    got_delta: bool,
    done: bool,
}

impl SseStream {
    pub(super) fn new(provider: ChatProvider) -> Self {
        Self {
            provider,
            full: String::new(),
            got_delta: false,
            done: false,
        }
    }

    /// Feeds one line; returns `true` once the `data: [DONE]` terminator arrived.
    pub(super) fn push_line(&mut self, line: &str, emit: Option<&(dyn Fn(String) + Send + Sync)>) -> bool {
        if self.done {
            return true;
        }
        if let Some(payload) = line.strip_prefix("data:") {
            let payload = payload.trim();
            if payload == "[DONE]" {
                self.done = true;
                return true;
            }
            if let Some(delta) = extract_sse_delta(self.provider, payload)
                && !delta.is_empty()
            {
                self.full.push_str(&delta);
                self.got_delta = true;
                if let Some(emit) = emit {
                    emit(delta);
                }
            }
        }
        false
    }

    pub(super) fn finish(self) -> (String, bool) {
        (self.full, self.got_delta)
    }
}

/// Reads the whole body chunk by chunk, framing SSE lines as they arrive (live
/// deltas via `emit`) while buffering everything for the JSON fallback and for
/// error previews. A trailing line without `\n` is flushed at the end.
async fn read_response_body(
    provider: ChatProvider,
    mut resp: reqwest::Response,
    cancel: &AtomicBool,
    emit: Option<&(dyn Fn(String) + Send + Sync)>,
) -> Result<(reqwest::StatusCode, Vec<u8>, String, bool), String> {
    let status = resp.status();
    let mut sse = SseStream::new(provider);
    let mut line_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut raw_body: Vec<u8> = Vec::new();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("요청이 중지되었습니다.".to_string());
        }
        match resp.chunk().await.map_err(|e| format!("응답 읽기 실패: {e}"))? {
            Some(bytes) => {
                raw_body.extend_from_slice(&bytes);
                for &b in bytes.iter() {
                    if b == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        sse.push_line(line.trim(), emit);
                        line_buf.clear();
                    } else if b != b'\r' {
                        line_buf.push(b);
                        if line_buf.len() > 256 * 1024 {
                            return Err("API 응답 한 줄이 너무 깁니다.".to_string());
                        }
                    }
                }
            }
            None => break,
        }
    }
    if !line_buf.is_empty() {
        let line = String::from_utf8_lossy(&line_buf);
        sse.push_line(line.trim(), emit);
    }
    let (full, got_delta) = sse.finish();
    Ok((status, raw_body, full, got_delta))
}

/// Builds a Gemini `generateContent` URL (`streaming` selects the SSE variant).
pub(super) fn gemini_url(endpoint: &str, model: &str, streaming: bool) -> Result<String, String> {
    let base = endpoint.trim_end_matches('/');
    let (plain, stream) = if let Some(prefix) = base.strip_suffix(":generateContent") {
        (
            format!("{prefix}:generateContent"),
            format!("{prefix}:streamGenerateContent"),
        )
    } else {
        (
            format!("{base}/{model}:generateContent"),
            format!("{base}/{model}:streamGenerateContent"),
        )
    };
    let chosen = if streaming { stream } else { plain };
    if streaming {
        // `alt=sse` makes the streaming response SSE-framed (`data: {...}` per line).
        Ok(format!(
            "{chosen}{}alt=sse",
            if chosen.contains('?') { "&" } else { "?" }
        ))
    } else {
        Ok(chosen)
    }
}

/// Sends a minimal 1-token request to verify the endpoint, key, and model all
/// work end to end. Returns a short human summary (`"연결됨 · 320ms · gpt-4o"`).
pub async fn test_api_connection(config: &ChatConfig, cancel: &AtomicBool) -> Result<String, String> {
    use std::time::Instant;

    let provider = config.provider;
    if provider.is_cli() {
        return Err("CLI 에이전트는 연결 테스트가 필요 없습니다.".to_string());
    }
    if !matches!(
        provider,
        ChatProvider::OpenAiApi | ChatProvider::AnthropicApi | ChatProvider::GeminiApi
    ) {
        return Err("잘못된 API 제공자입니다.".to_string());
    }
    let model = config.selected_model(provider);
    if !is_valid_model_id(&model) {
        return Err("모델 ID를 입력하세요.".to_string());
    }
    let api_key = config.effective_api_key().to_string();
    let endpoint = config.effective_endpoint().to_string();
    let (parsed, _is_local) = validate_api_endpoint_url(&endpoint, &api_key)?;
    if api_key.is_empty() && !is_loopback_endpoint(&parsed) {
        return Err("API 키를 입력하세요.".to_string());
    }
    let client = api_http_client()?;

    let (target_url, body_bytes, req) = match provider {
        ChatProvider::OpenAiApi => {
            let payload = serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": "ping"}],
                "max_tokens": 1,
            });
            let bytes = serde_json::to_vec(&payload).map_err(|e| format!("JSON 직렬화 실패: {e}"))?;
            let mut req = client.post(&endpoint).header("Content-Type", "application/json");
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {api_key}"));
            }
            (endpoint, bytes, req)
        }
        ChatProvider::AnthropicApi => {
            let payload = serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
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
            let full_url = gemini_url(&endpoint, &model, false)?;
            let _ = validate_api_endpoint_url(&full_url, &api_key)?;
            let payload = serde_json::json!({
                "contents": [{"role": "user", "parts": [{"text": "ping"}]}],
                "generationConfig": {"maxOutputTokens": 1},
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
    let _ = target_url;

    let started = Instant::now();
    let resp = tokio::time::timeout(Duration::from_secs(20), req.body(body_bytes).send())
        .await
        .map_err(|_| "연결 시간 초과 (20초)".to_string())?
        .map_err(|e| format!("API 연결 실패: {e}"))?;
    if cancel.load(Ordering::Acquire) {
        return Err("요청이 중지되었습니다.".to_string());
    }
    let status = resp.status();
    let resp_bytes = resp.bytes().await.map_err(|e| format!("응답 읽기 실패: {e}"))?;
    if !status.is_success() {
        let preview: String = String::from_utf8_lossy(&resp_bytes).chars().take(200).collect();
        return Err(format!("연결 실패 HTTP {status}: {preview}"));
    }
    let val: Value = serde_json::from_slice(&resp_bytes).map_err(|e| format!("JSON 파싱 실패: {e}"))?;
    if extract_buffered_reply(provider, &val).is_none_or(|s| s.trim().is_empty()) {
        return Err("응답은 왔으나 텍스트가 비어 있습니다.".to_string());
    }
    Ok(format!("연결됨 · {}ms · {model}", started.elapsed().as_millis()))
}

fn extract_buffered_reply(provider: ChatProvider, val: &Value) -> Option<String> {
    match provider {
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
    }
}
