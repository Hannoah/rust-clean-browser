//! Korean <-> English page translation. Visible text nodes of the page HTML are sent to Google's
//! public `translate_a/t` endpoint (several `q` per request, one result each) and substituted back.
//! Direction is automatic: mostly-Hangul pages become English, everything else Korean.

use serde_json::Value;
use std::ops::Range;
use std::sync::OnceLock;
use std::time::Duration;

/// Elements whose text is code/data, never prose (or hidden templates).
const SKIP: [&str; 8] = [
    "script", "style", "pre", "code", "textarea", "svg", "noscript", "template",
];
const CHUNK_CHARS: usize = 4500;
// ponytail: ~72k chars per page; longer pages stay partly original. Raise or paginate if needed.
const MAX_CHUNKS: usize = 16;
const MAX_RESP_BYTES: usize = 2 * 1024 * 1024;

pub struct Seg {
    pub range: Range<usize>,
    pub text: String,
}

/// Translatable text runs (trimmed byte range in `html` + whitespace-collapsed, entity-decoded text).
pub fn segments(html: &str) -> Vec<Seg> {
    let lower = html.to_ascii_lowercase();
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut push = |a: usize, z: usize| {
        let raw = &html[a..z];
        let t = raw.trim();
        if t.chars().any(char::is_alphabetic) {
            let start = a + raw.len() - raw.trim_start().len();
            let text = crate::cleaner::decode_basic_entities(t)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push(Seg {
                range: start..start + t.len(),
                text,
            });
        }
    };
    let mut i = 0;
    while i < html.len() {
        let Some(lt) = lower[i..].find('<').map(|p| p + i) else {
            push(i, html.len());
            break;
        };
        if lower[lt..].starts_with("<!--") {
            push(i, lt);
            i = crate::cleaner::comment_end(&lower, lt);
            continue;
        }
        if !b
            .get(lt + 1)
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
        {
            // A lone `<` in prose (e.g., `a < b < c`) is text, not a tag: scan forward to the next real tag.
            let mut scan = lt + 1;
            let next_real = loop {
                let Some(next) = lower[scan..].find('<').map(|p| scan + p) else {
                    break html.len();
                };
                if lower[next..].starts_with("<!--")
                    || b.get(next + 1)
                        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
                {
                    break next;
                }
                scan = next + 1;
            };
            push(i, next_real);
            i = next_real;
            continue;
        }
        push(i, lt);
        let Some(gt) = crate::cleaner::tag_end(b, lt + 1) else {
            break;
        };
        i = gt + 1;
        let name: String = lower[lt + 1..gt]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let self_close = {
            let mut end = gt;
            while end > lt + 1 && b[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            end > lt + 1
                && b[end - 1] == b'/'
                && (end - 1 == lt + 1 || b[end - 2].is_ascii_whitespace() || matches!(b[end - 2], b'"' | b'\''))
        };
        if SKIP.contains(&name.as_str()) && !self_close {
            let close_pat = format!("</{name}");
            let mut scan = i;
            let mut close_pos = None;
            while let Some(rel) = lower[scan..].find(&close_pat) {
                let pos = scan + rel;
                let after = pos + close_pat.len();
                if b.get(after).is_some_and(|c| c.is_ascii_whitespace() || *c == b'>') {
                    close_pos = Some(pos);
                    break;
                }
                scan = after;
            }
            i = close_pos.unwrap_or(html.len());
        }
    }
    out
}

/// `"en"` when at least 30% of the letters are Hangul, else `"ko"`.
pub fn target_lang(segs: &[Seg]) -> &'static str {
    let (mut ko, mut all) = (0usize, 0usize);
    for c in segs.iter().flat_map(|s| s.text.chars()).filter(|c| c.is_alphabetic()) {
        all += 1;
        ko += ('\u{AC00}'..='\u{D7A3}').contains(&c) as usize;
    }
    if ko * 10 >= all * 3 { "en" } else { "ko" }
}

/// Substitute `translated[i]` for each segment (HTML-escaped).
pub fn apply(html: &str, segs: &[Seg], translated: &[String]) -> String {
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for (s, t) in segs.iter().zip(translated) {
        out.push_str(&html[last..s.range.start]);
        out.push_str(&t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"));
        last = s.range.end;
    }
    out.push_str(&html[last..]);
    out
}

/// Greedy batches of consecutive segment indices, each at most `CHUNK_CHARS` characters.
fn chunk(texts: &[String]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut len) = (0, 0usize);
    for (i, t) in texts.iter().enumerate() {
        let n = t.chars().count().min(CHUNK_CHARS);
        if len + n > CHUNK_CHARS && i > start {
            out.push(start..i);
            (start, len) = (i, 0);
        }
        len += n;
    }
    if start < texts.len() {
        out.push(start..texts.len());
    }
    out
}

/// `[["translated","src-lang"], ..]` -> translated strings, one per requested text.
fn parse_chunk(v: &Value, expected: usize) -> Result<Vec<String>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| "번역 응답 형식이 올바르지 않습니다.".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let txt = item
            .get(0)
            .and_then(Value::as_str)
            .ok_or_else(|| "번역 응답 항목이 비정상입니다.".to_string())?;
        out.push(txt.trim().to_string());
    }
    if out.len() == expected {
        Ok(out)
    } else {
        Err(format!("응답 항목 수 불일치 ({}/{expected})", out.len()))
    }
}

static TRANSLATE_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

fn translate_client() -> Result<&'static reqwest::Client, String> {
    match TRANSLATE_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("Mozilla/5.0")
            .build()
            .map_err(|e| e.to_string())
    }) {
        Ok(c) => Ok(c),
        Err(e) => Err(e.clone()),
    }
}

async fn post(client: reqwest::Client, texts: Vec<String>, tl: &str) -> Result<Vec<String>, String> {
    // Built in its own scope: the serializer is not `Send` and must not live across `.await`.
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for t in &texts {
            form.append_pair("q", &t.chars().take(CHUNK_CHARS).collect::<String>());
        }
        form.finish()
    };
    let tl_enc: String = url::form_urlencoded::byte_serialize(tl.as_bytes()).collect();
    let url = format!("https://clients5.google.com/translate_a/t?client=dict-chrome-ex&sl=auto&tl={tl_enc}");
    let mut resp = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESP_BYTES {
            return Err("번역 응답이 너무 큽니다.".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    // An HTML "Sorry..." page instead of JSON means Google rate-limited / blocked this client.
    let json =
        serde_json::from_slice(&bytes).map_err(|_| "번역 서버가 요청을 거부함(차단 또는 요청 제한)".to_string())?;
    parse_chunk(&json, texts.len())
}

/// Translate every text; texts whose batch failed stay original. Err only if all batches failed.
pub async fn translate(texts: Vec<String>, tl: &'static str) -> Result<Vec<String>, String> {
    if texts.is_empty() {
        return Ok(texts);
    }
    let client = translate_client()?.clone();
    let jobs: Vec<_> = chunk(&texts)
        .into_iter()
        .take(MAX_CHUNKS)
        .map(|range| {
            let (c, batch) = (client.clone(), texts[range.clone()].to_vec());
            tokio::spawn(async move { (range, post(c, batch, tl).await) })
        })
        .collect();
    let (mut out, mut ok, mut last_err) = (texts, 0, String::new());
    for job in jobs {
        match job.await {
            Ok((range, Ok(tr))) => {
                ok += 1;
                for (i, t) in range.zip(tr).filter(|(_, t)| !t.is_empty()) {
                    out[i] = t;
                }
            }
            Ok((_, Err(e))) => last_err = e,
            Err(e) => last_err = e.to_string(),
        }
    }
    if ok == 0 {
        Err(format!("번역 실패: {last_err}"))
    } else {
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_skip_code_and_roundtrip() {
        let html = r#"<html><title>Hello</title><body><p class="a>b">Hi &amp; bye</p><pre>let x = 1;</pre><script>var a = "t";</script><p>  42 </p><p>World</p></body>"#;
        let segs = segments(html);
        let texts: Vec<_> = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Hello", "Hi & bye", "World"]);
        let tr: Vec<String> = ["안녕", "안녕 & 잘가", "세계 <3"].map(String::from).into();
        let out = apply(html, &segs, &tr);
        assert!(out.contains("<title>안녕</title>") && out.contains("세계 &lt;3"));
        assert!(out.contains("<pre>let x = 1;</pre>") && out.contains(r#"class="a>b""#));
        assert_eq!(target_lang(&segs), "ko");
        assert_eq!(target_lang(&segments("<p>안녕하세요 반갑습니다</p>")), "en");
    }

    #[test]
    fn chunks_and_parses_per_item() {
        let texts: Vec<String> = vec!["a".repeat(3000), "b".repeat(3000), "c".into()];
        assert_eq!(chunk(&texts), [0..1, 1..3]);
        let resp: Value = serde_json::from_str(r#"[["하나 ","en"],["둘","en"]]"#).unwrap();
        assert_eq!(parse_chunk(&resp, 2).unwrap(), ["하나", "둘"]);
        assert!(parse_chunk(&resp, 3).is_err());
        let flat: Value = serde_json::from_str(r#"["하나","둘"]"#).unwrap();
        assert!(parse_chunk(&flat, 2).is_err());

        let lone = r#"<p>a < b < c</p><pre-formatted>Keep me</pre-formatted><template>Hidden</template>"#;
        let segs = segments(lone);
        let texts: Vec<_> = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["a < b < c", "Keep me"]);
    }

    /// Hits the real endpoint: `cargo test live_translate -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_translate() {
        let out = translate(vec!["Hello world".into(), "Good morning, everyone.".into()], "ko")
            .await
            .unwrap();
        assert!(
            out.iter()
                .all(|t| t.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))),
            "{out:?}"
        );
        let back = translate(vec!["안녕하세요 세상".into()], "en").await.unwrap();
        assert!(back[0].to_lowercase().contains("hello"), "{back:?}");
    }
}
