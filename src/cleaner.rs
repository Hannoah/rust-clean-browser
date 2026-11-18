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

/// Produce a distraction-free Reader Mode HTML document with default options (used by tests).
#[cfg(test)]
pub fn build_reader_html(raw_html: &str, url: &str) -> String {
    build_reader_html_with_options(raw_html, url, ReaderOptions::default(), false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderTheme {
    #[default]
    Auto,
    Light,
    Sepia,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderFont {
    #[default]
    Sans,
    Serif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderWidth {
    Narrow,
    #[default]
    Medium,
    Wide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReaderOptions {
    pub theme: ReaderTheme,
    pub font: ReaderFont,
    pub font_size: u32,
    pub width: ReaderWidth,
}

impl Default for ReaderOptions {
    fn default() -> Self {
        Self {
            theme: ReaderTheme::Auto,
            font: ReaderFont::Sans,
            font_size: 18,
            width: ReaderWidth::Medium,
        }
    }
}

pub fn build_reader_html_with_options(
    raw_html: &str,
    url: &str,
    opts: ReaderOptions,
    os_dark: bool,
) -> String {
    let title = extract_title(raw_html).unwrap_or_else(|| url.to_string());
    let cleaned = strip_tag_blocks(
        raw_html,
        &["script", "style", "noscript", "nav", "aside", "footer", "header", "iframe", "form"],
    );

    let body_fragment = extract_primary_fragment(&cleaned).to_string();
    let visible_chars = count_visible_text_chars(&body_fragment);
    let minutes = (visible_chars / 750).max(1);
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);

    let eff_theme = match opts.theme {
        ReaderTheme::Auto => if os_dark { ReaderTheme::Dark } else { ReaderTheme::Light },
        other => other,
    };
    let (bg, bar_bg, text, muted, link, border, code_bg, active_bg, active_fg) = match eff_theme {
        ReaderTheme::Sepia => (
            "#f6eedb", "#ece0c6", "#3b2f26", "#746251", "#9a4d16", "#dfd1b4", "#ebe0c7", "#9a4d16", "#fcf8ef",
        ),
        ReaderTheme::Dark => (
            "#16181c", "#1e2127", "#dcdfe6", "#8d96a6", "#8ab4f8", "#2c313b", "#22262e", "#1b3863", "#d6e6ff",
        ),
        ReaderTheme::Light | ReaderTheme::Auto => (
            "#fcfcfd", "#f2f4f8", "#1c2026", "#5a6372", "#0b57d0", "#e2e6ee", "#f1f4f8", "#dbeafe", "#0f3a8a",
        ),
    };
    let font_stack = match opts.font {
        ReaderFont::Sans => {
            r#""Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", "Apple SD Gothic Neo", "Noto Sans KR", "Malgun Gothic", sans-serif"#
        }
        ReaderFont::Serif => {
            r#""Noto Serif KR", "Batang", "Georgia", "Charter", "AppleMyungjo", serif"#
        }
    };
    let max_ch = match opts.width {
        ReaderWidth::Narrow => 56,
        ReaderWidth::Medium => 66,
        ReaderWidth::Wide => 78,
    };
    let font_size = opts.font_size.clamp(14, 24);
    let active_cls = |cond: bool| if cond { "rpill on" } else { "rpill" };

    format!(
        r#"<!DOCTYPE html>
<html lang="ko">
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>
  :root {{ color-scheme: light dark; }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    padding: 20px 24px 88px;
    background: {bg};
    color: {text};
    font-family: {font_stack};
    line-height: 1.72;
    font-size: {font_size}px;
  }}
  .reader-shell {{
    max-width: {max_ch}ch;
    margin: 0 auto;
  }}
  .reader-bar {{
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    margin-bottom: 28px;
    border-radius: 14px;
    background: {bar_bg};
    border: 1px solid {border};
    font-family: "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", "Malgun Gothic", sans-serif;
    font-size: 12px;
  }}
  .rgroup {{ display: flex; align-items: center; gap: 4px; }}
  .rsep {{ width: 1px; height: 16px; background: {border}; margin: 0 4px; }}
  .rpill {{
    display: inline-flex;
    align-items: center;
    height: 26px;
    padding: 0 10px;
    border-radius: 13px;
    color: {muted};
    text-decoration: none;
    font-weight: 500;
  }}
  .rpill:hover {{ background: {border}; color: {text}; }}
  .rpill.on {{ background: {active_bg}; color: {active_fg}; font-weight: 600; }}
  .rsize {{ color: {text}; font-variant-numeric: tabular-nums; padding: 0 4px; font-weight: 600; }}
  .reader-meta {{
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: {muted};
    margin-bottom: 10px;
    font-family: "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", "Malgun Gothic", sans-serif;
  }}
  .rmeta-chip {{
    padding: 2px 9px;
    border-radius: 999px;
    background: {bar_bg};
    border: 1px solid {border};
    color: {text};
    font-weight: 500;
  }}
  .reader-title {{
    font-size: 1.75em;
    line-height: 1.28;
    font-weight: 700;
    letter-spacing: -0.02em;
    margin: 0 0 22px 0;
    padding-bottom: 18px;
    border-bottom: 1px solid {border};
    color: {text};
  }}
  .reader-content h1, .reader-content h2, .reader-content h3, .reader-content h4 {{
    line-height: 1.35;
    margin: 1.6em 0 0.6em;
    font-weight: 700;
  }}
  .reader-content p {{ margin: 0 0 1.2em; }}
  .reader-content ul, .reader-content ol {{ margin: 0 0 1.2em; padding-left: 1.5em; }}
  .reader-content li {{ margin-bottom: 0.45em; }}
  .reader-content img {{ max-width: 100%; height: auto; border-radius: 10px; margin: 16px 0; }}
  .reader-content pre, .reader-content code {{
    background: {code_bg};
    border-radius: 6px;
    font-family: "Cascadia Code", Consolas, "JetBrains Mono", monospace;
    font-size: 0.85em;
  }}
  .reader-content code {{ padding: 2px 5px; }}
  .reader-content pre {{ padding: 14px 16px; overflow-x: auto; border: 1px solid {border}; line-height: 1.55; }}
  .reader-content pre code {{ padding: 0; background: transparent; }}
  .reader-content a {{ color: {link}; text-decoration: underline; text-underline-offset: 3px; }}
  .reader-content blockquote {{
    border-left: 3px solid {link};
    background: {bar_bg};
    margin: 20px 0;
    padding: 10px 18px;
    border-radius: 0 8px 8px 0;
    color: {muted};
  }}
  .reader-content hr {{ border: none; border-top: 1px solid {border}; margin: 28px 0; }}
  .reader-content table {{ width: 100%; border-collapse: collapse; margin: 18px 0; font-size: 0.9em; }}
  .reader-content th, .reader-content td {{ border: 1px solid {border}; padding: 8px 12px; text-align: left; }}
  .reader-content th {{ background: {bar_bg}; font-weight: 600; }}
</style>
</head>
<body>
  <div class="reader-shell">
    <div class="reader-bar">
      <div class="rgroup">
        <a class="{t_light}" href="rust://reader-opt?theme=light">라이트</a>
        <a class="{t_sepia}" href="rust://reader-opt?theme=sepia">세피아</a>
        <a class="{t_dark}" href="rust://reader-opt?theme=dark">다크</a>
        <div class="rsep"></div>
        <a class="rpill" href="rust://reader-opt?size=dec">A-</a>
        <span class="rsize">{font_size}px</span>
        <a class="rpill" href="rust://reader-opt?size=inc">A+</a>
        <div class="rsep"></div>
        <a class="{f_sans}" href="rust://reader-opt?font=sans">고딕</a>
        <a class="{f_serif}" href="rust://reader-opt?font=serif">명조</a>
        <div class="rsep"></div>
        <a class="{w_narrow}" href="rust://reader-opt?width=narrow">좁게</a>
        <a class="{w_med}" href="rust://reader-opt?width=medium">보통</a>
        <a class="{w_wide}" href="rust://reader-opt?width=wide">넓게</a>
      </div>
      <div class="rgroup">
        <a class="rpill" href="rust://reader-opt?exit=1">원본 보기</a>
      </div>
    </div>
    <div class="reader-meta">
      <span class="rmeta-chip">Reader Mode</span>
      <span>{host}</span>
      <span>·</span>
      <span>약 {minutes}분 읽기</span>
    </div>
    <h1 class="reader-title">{title}</h1>
    <div class="reader-content">{body_fragment}</div>
  </div>
</body>
</html>"#,
        title = html_escape(&title),
        host = html_escape(host),
        t_light = active_cls(eff_theme == ReaderTheme::Light),
        t_sepia = active_cls(eff_theme == ReaderTheme::Sepia),
        t_dark = active_cls(eff_theme == ReaderTheme::Dark),
        f_sans = active_cls(opts.font == ReaderFont::Sans),
        f_serif = active_cls(opts.font == ReaderFont::Serif),
        w_narrow = active_cls(opts.width == ReaderWidth::Narrow),
        w_med = active_cls(opts.width == ReaderWidth::Medium),
        w_wide = active_cls(opts.width == ReaderWidth::Wide),
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
        assert!(reader.contains("약 1분 읽기"));

        let sepia = build_reader_html_with_options(
            raw,
            "https://example.com",
            ReaderOptions {
                theme: ReaderTheme::Sepia,
                font: ReaderFont::Serif,
                font_size: 20,
                width: ReaderWidth::Narrow,
            },
            false,
        );
        assert!(sepia.contains("#f6eedb"));
        assert!(sepia.contains("max-width: 56ch"));
        assert!(sepia.contains("font-size: 20px"));
        assert!(sepia.contains("Noto Serif KR"));
    }

    #[test]
    fn detects_empty_js_spa_shells_accurately() {
        let naver_like = r#"<!doctype html><html><head><script src="a.js"></script><script src="b.js"></script></head><body><div id="header">검색</div><div id="container"><div id="root"></div></div></body></html>"#;
        assert!(is_js_spa_shell(naver_like));

        let article_like = r#"<!doctype html><html><head><script src="a.js"></script></head><body><article><h1>Rust Browser</h1><p>This is a full server-rendered article with plenty of readable content and links.</p></article></body></html>"#;
        assert!(!is_js_spa_shell(article_like));
    }
}
