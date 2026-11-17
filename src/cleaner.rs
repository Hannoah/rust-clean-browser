pub fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Strip `<script>` blocks (and optionally ad/embed `<iframe>`s) plus large HTML comments so
/// bloated inline JS bundles never enter the DOM or allocate parser nodes.
pub fn strip_scripts_and_bloat(html: &str, strip_iframes: bool) -> String {
    let tags: &[&str] = if strip_iframes { &["script", "iframe"] } else { &["script"] };
    strip_large_comments(&strip_tag_blocks(html, tags), 256)
}

/// Decode a fetched document to UTF-8: BOM, then `<meta charset>` sniffing, then UTF-8,
/// falling back to EUC-KR (windows-949) for undeclared legacy Korean pages.
pub fn decode_html_bytes(bytes: &[u8]) -> String {
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    if let Some(enc) = sniff_meta_charset(&bytes[..bytes.len().min(2048)]) {
        if enc != encoding_rs::UTF_8 {
            return enc.decode_without_bom_handling(bytes).0.into_owned();
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            let (text, _, had_errors) = encoding_rs::EUC_KR.decode(bytes);
            if had_errors {
                String::from_utf8_lossy(bytes).into_owned()
            } else {
                text.into_owned()
            }
        }
    }
}

fn sniff_meta_charset(head: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let lower = head.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = find_bytes(&lower[from..], b"charset") {
        let mut i = from + rel + 7;
        from = i;
        while i < lower.len() && matches!(lower[i], b' ' | b'=' | b'"' | b'\'') {
            i += 1;
        }
        let start = i;
        while i < lower.len() && (lower[i].is_ascii_alphanumeric() || matches!(lower[i], b'-' | b'_')) {
            i += 1;
        }
        if i > start {
            if let Some(enc) = encoding_rs::Encoding::for_label(&lower[start..i]) {
                return Some(enc);
            }
        }
    }
    None
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decode, classify and clean a fetched document in one go (runs on the network thread).
/// Returns `(clean_html, is_spa_shell)`.
pub fn process_fetched_document(bytes: &[u8], url: &str, strip_iframes: bool) -> Result<(String, bool), String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"%PDF-")
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"GIF8")
    {
        return Err("HTML 문서가 아닌 바이너리/이미지/PDF 파일입니다.".to_string());
    }
    let text = decode_html_bytes(bytes);
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Err("빈 응답을 받았습니다.".to_string());
    }
    let looks_like_html = trimmed.starts_with('<')
        || trimmed.get(..512).is_some_and(|head| {
            let h = head.to_ascii_lowercase();
            h.contains("<!doctype") || h.contains("<html") || h.contains("<head") || h.contains("<body")
        });
    if looks_like_html {
        let spa = is_js_spa_shell(&text);
        Ok((strip_scripts_and_bloat(&text, strip_iframes), spa))
    } else {
        Ok((
            format!(
                r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>{url}</title><style>:root {{ color-scheme: light dark; }} body {{ margin: 24px; font-family: Consolas, "Cascadia Mono", monospace; font-size: 13px; }} pre {{ white-space: pre-wrap; word-break: break-word; }}</style></head><body><pre>{body}</pre></body></html>"#,
                url = html_escape(url),
                body = html_escape(&text),
            ),
            false,
        ))
    }
}

/// Removes `<tag ...>...</tag>` blocks for every tag in `tags` in a single pass.
fn strip_tag_blocks(input: &str, tags: &[&str]) -> String {
    let lower = input.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let mut scan = 0usize;

    while let Some(rel) = lower[scan..].find('<') {
        let start = scan + rel;
        scan = start + 1;
        let Some(tag) = tags.iter().find(|t| {
            let after = start + 1 + t.len();
            lower[start + 1..].starts_with(**t)
                && lb.get(after).is_some_and(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/')
        }) else {
            continue;
        };
        let after_tag = start + 1 + tag.len();
        out.push_str(&input[cursor..start]);

        let tag_end = lower[after_tag..].find('>').map(|r| after_tag + r);
        if let Some(te) = tag_end {
            if lower[after_tag..te].trim_end().ends_with('/') {
                cursor = te + 1;
                scan = cursor;
                continue;
            }
        }
        let close_pat = format!("</{tag}");
        match lower[after_tag..].find(&close_pat) {
            Some(rel_close) => {
                let close_start = after_tag + rel_close;
                cursor = lower[close_start..].find('>').map_or(input.len(), |r| close_start + r + 1);
            }
            None => cursor = input.len(),
        }
        scan = cursor;
        if cursor >= input.len() {
            break;
        }
    }
    if cursor < input.len() {
        out.push_str(&input[cursor..]);
    }
    out
}

fn strip_large_comments(input: &str, max_keep: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rel_start) = input[cursor..].find("<!--") {
        let start = cursor + rel_start;
        out.push_str(&input[cursor..start]);
        if let Some(rel_end) = input[start + 4..].find("-->") {
            let end = start + 4 + rel_end + 3;
            if end - start <= max_keep {
                out.push_str(&input[start..end]);
            }
            cursor = end;
        } else {
            cursor = input.len();
            break;
        }
    }
    if cursor < input.len() {
        out.push_str(&input[cursor..]);
    }
    out
}

/// Detect client-side-rendered JavaScript SPA shells (such as Naver PC `<div id="root"></div>`)
/// that contain virtually no static HTML body until JS executes.
pub fn is_js_spa_shell(raw_html: &str) -> bool {
    let lower = raw_html.to_ascii_lowercase();
    if !lower.contains("<script") {
        return false;
    }

    let body_fragment = extract_primary_fragment(&lower);
    let cleaned = strip_tag_blocks(body_fragment, &["script", "style", "noscript", "svg"]);

    let visible_len = count_visible_text_chars(&cleaned);
    let has_empty_spa_mount = lower.contains("id=\"root\"></div>")
        || lower.contains("id='root'></div>")
        || lower.contains("id=\"app\"></div>")
        || lower.contains("id='app'></div>")
        || lower.contains("id=\"__next\"></div>");

    if has_empty_spa_mount && visible_len < 600 {
        return true;
    }

    let script_count = lower.matches("<script").count();
    script_count >= 2 && visible_len < 100 && raw_html.len() > 4096
}

fn count_visible_text_chars(html: &str) -> usize {
    let mut count = 0usize;
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => count += 1,
            _ => {}
        }
    }
    count
}

pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let gt = lower[start..].find('>')? + start + 1;
    let end = lower[gt..].find("</title>")? + gt;
    let raw = html[gt..end].trim();
    if raw.is_empty() {
        None
    } else {
        Some(decode_basic_entities(raw))
    }
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Produce a distraction-free Reader Mode HTML document when pages are messy or cluttered.
pub fn build_reader_html(raw_html: &str, url: &str) -> String {
    let title = extract_title(raw_html).unwrap_or_else(|| url.to_string());
    let cleaned = strip_tag_blocks(
        raw_html,
        &["script", "style", "noscript", "nav", "aside", "footer", "header", "iframe", "form"],
    );

    let body_fragment = extract_primary_fragment(&cleaned).to_string();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{
    margin: 0;
    padding: 32px 24px 72px;
    background: #fafafa;
    color: #1f2328;
    font-family: "Segoe UI", "Malgun Gothic", -apple-system, BlinkMacSystemFont, sans-serif;
    line-height: 1.7;
    font-size: 16px;
  }}
  .reader-shell {{
    max-width: 760px;
    margin: 0 auto;
  }}
  .reader-banner {{
    font-size: 12px;
    color: #57606a;
    border-bottom: 1px solid #d8dee4;
    padding-bottom: 10px;
    margin-bottom: 22px;
  }}
  .reader-title {{
    font-size: 28px;
    line-height: 1.3;
    margin: 0 0 18px 0;
    color: #0d1117;
  }}
  img {{ max-width: 100%; height: auto; border-radius: 6px; }}
  pre, code {{
    background: #f0f3f6;
    border-radius: 4px;
    font-family: Consolas, "Cascadia Mono", monospace;
    font-size: 14px;
  }}
  pre {{ padding: 12px; overflow-x: auto; }}
  a {{ color: #0969da; }}
  blockquote {{
    border-left: 3px solid #d0d7de;
    margin: 16px 0;
    padding: 4px 16px;
    color: #57606a;
  }}
  @media (prefers-color-scheme: dark) {{
    body {{ background: #1b1c1f; color: #e3e5e8; }}
    .reader-title {{ color: #f1f3f4; }}
    .reader-banner {{ color: #9aa0a6; border-color: #3c4043; }}
    pre, code {{ background: #2a2b2f; }}
    a {{ color: #8ab4f8; }}
    blockquote {{ border-color: #5f6368; color: #bdc1c6; }}
  }}
</style>
</head>
<body>
  <div class="reader-shell">
    <div class="reader-banner">Reader Mode · {url}</div>
    <h1 class="reader-title">{title}</h1>
    <div class="reader-content">{body_fragment}</div>
  </div>
</body>
</html>"#,
        title = html_escape(&title),
        url = html_escape(url),
        body_fragment = body_fragment,
    )
}

fn extract_primary_fragment(html: &str) -> &str {
    let lower = html.to_ascii_lowercase();
    for tag in ["<article", "<main", "<body"] {
        if let Some(start) = lower.find(tag) {
            if let Some(gt_rel) = lower[start..].find('>') {
                let inner_start = start + gt_rel + 1;
                let close_tag = format!("</{}", &tag[1..]);
                if let Some(end_rel) = lower[inner_start..].rfind(&close_tag) {
                    return &html[inner_start..inner_start + end_rel];
                }
                return &html[inner_start..];
            }
        }
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_and_keeps_article_content() {
        let raw = r#"<html><head><title>Test &amp; Clean</title><script>alert(1)</script></head><body><script src="x.js"></script><p>Hello World</p></body></html>"#;
        let stripped = strip_scripts_and_bloat(raw, true);
        assert!(!stripped.contains("alert(1)"));
        assert!(!stripped.contains("x.js"));
        assert!(stripped.contains("<p>Hello World</p>"));
        assert_eq!(extract_title(raw).as_deref(), Some("Test & Clean"));
    }

    #[test]
    fn decodes_declared_and_undeclared_euc_kr() {
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("<html><head><meta charset=\"euc-kr\"></head><body>한글</body></html>");
        assert!(decode_html_bytes(&bytes).contains("한글"));
        let (bare, _, _) = encoding_rs::EUC_KR.encode("<p>안녕하세요</p>");
        assert!(decode_html_bytes(&bare).contains("안녕하세요"));
        assert_eq!(decode_html_bytes("<p>é</p>".as_bytes()), "<p>é</p>");
    }

    #[test]
    fn strips_multiple_tags_in_one_pass() {
        let raw = r#"<p>a</p><SCRIPT type="x">1</SCRIPT><iframe src="ad"></iframe><scripts>keep</scripts><script/>b"#;
        let out = strip_scripts_and_bloat(raw, true);
        assert_eq!(out, "<p>a</p><scripts>keep</scripts>b");
        assert!(strip_scripts_and_bloat(raw, false).contains("<iframe"));
    }

    #[test]
    fn builds_reader_mode_without_nav_or_scripts() {
        let raw = r#"<html><head><title>Article</title></head><body><nav>Menu</nav><article><p>Body text</p><script>bad()</script></article></body></html>"#;
        let reader = build_reader_html(raw, "https://example.com");
        assert!(!reader.contains("Menu"));
        assert!(!reader.contains("bad()"));
        assert!(reader.contains("Body text"));
    }

    #[test]
    fn detects_empty_js_spa_shells_accurately() {
        let naver_like = r#"<!doctype html><html><head><script src="a.js"></script><script src="b.js"></script></head><body><div id="header">검색</div><div id="container"><div id="root"></div></div></body></html>"#;
        assert!(is_js_spa_shell(naver_like));

        let article_like = r#"<!doctype html><html><head><script src="a.js"></script></head><body><article><h1>Rust Browser</h1><p>This is a full server-rendered article with plenty of readable content and links.</p></article></body></html>"#;
        assert!(!is_js_spa_shell(article_like));
    }
}
