//! Korean <-> English page translation. Visible text nodes of the page HTML are sent to Google's
//! public `translate_a/t` endpoint (several `q` per request, one result each) and substituted back.
//! Direction is automatic: mostly-Hangul pages become English, everything else Korean.

use serde_json::Value;
use std::ops::Range;
use std::time::Duration;

/// Elements whose text is code/data, never prose.
const SKIP: [&str; 7] = ["script", "style", "pre", "code", "textarea", "svg", "noscript"];
const CHUNK_CHARS: usize = 4500;
// ponytail: ~72k chars per page; longer pages stay partly original. Raise or paginate if needed.
const MAX_CHUNKS: usize = 16;

pub struct Seg {
    pub range: Range<usize>,
    pub text: String,
}

/// Index of the `>` closing the tag that starts at `from`, honouring quoted attribute values.
fn tag_end(b: &[u8], from: usize) -> Option<usize> {
    let mut quote = 0u8;
    for (i, &c) in b.iter().enumerate().skip(from) {
        match (quote, c) {
            (0, b'"' | b'\'') => quote = c,
            (0, b'>') => return Some(i),
            (q, c) if q == c => quote = 0,
            _ => {}
        }
    }
    None
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
            let text = crate::cleaner::decode_basic_entities(t).split_whitespace().collect::<Vec<_>>().join(" ");
            out.push(Seg { range: start..start + t.len(), text });
        }
    };
    let mut i = 0;
    while i < html.len() {
        let Some(lt) = lower[i..].find('<').map(|p| p + i) else {
            push(i, html.len());
            break;
        };
        push(i, lt);
        if lower[lt..].starts_with("<!--") {
            i = lower[lt + 4..].find("-->").map_or(html.len(), |p| lt + 4 + p + 3);
            continue;
        }
        let Some(gt) = tag_end(b, lt + 1) else { break };
        i = gt + 1;
        let name: String = lower[lt + 1..gt].chars().take_while(char::is_ascii_alphanumeric).collect();
        if SKIP.contains(&name.as_str()) && !lower[lt..gt].ends_with('/') {
            i = lower[i..].find(&format!("</{name}")).map_or(html.len(), |p| i + p);
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
    let out: Vec<String> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| e.get(0).and_then(Value::as_str).unwrap_or_default().trim().to_string())
        .collect();
    if out.len() == expected { Ok(out) } else { Err(format!("응답 항목 수 불일치 ({}/{expected})", out.len())) }
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
    let url = format!("https://clients5.google.com/translate_a/t?client=dict-chrome-ex&sl=auto&tl={tl}");
    let resp = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    // An HTML "Sorry..." page instead of JSON means Google rate-limited / blocked this client.
    let json = serde_json::from_slice(&bytes).map_err(|_| "번역 서버가 요청을 거부함(차단 또는 요청 제한)".to_string())?;
    parse_chunk(&json, texts.len())
}

/// Translate every text; texts whose batch failed stay original. Err only if all batches failed.
pub async fn translate(texts: Vec<String>, tl: &'static str) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("Mozilla/5.0")
        .build()
        .map_err(|e| e.to_string())?;
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
    if ok == 0 { Err(format!("번역 실패: {last_err}")) } else { Ok(out) }
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
    }

    /// Hits the real endpoint: `cargo test live_translate -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_translate() {
        let out = translate(vec!["Hello world".into(), "Good morning, everyone.".into()], "ko").await.unwrap();
        assert!(out.iter().all(|t| t.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))), "{out:?}");
        let back = translate(vec!["안녕하세요 세상".into()], "en").await.unwrap();
        assert!(back[0].to_lowercase().contains("hello"), "{back:?}");
    }
}
