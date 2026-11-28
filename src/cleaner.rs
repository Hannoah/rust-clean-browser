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
    let tags: &[&str] = if strip_iframes {
        &["script", "iframe"]
    } else {
        &["script"]
    };
    let no_comments = strip_large_comments(html, 256);
    let stripped = strip_tag_blocks_fixed_point(&no_comments, tags);
    sanitize_dangerous_attrs(&stripped)
}

/// Sanitize untrusted remote SVG favicons: strip DTD/ENTITY declarations, dangerous/beacon
/// elements (`script`, `foreignObject`, `style`, `animate`, `feImage`, etc.), event handlers,
/// and external `href`/`xlink:href`/`src` URLs (keeping local `#id` references only).
pub fn sanitize_svg(svg: &str) -> String {
    let no_dtd = strip_dtd_declarations(svg);
    let stripped = strip_tag_blocks_fixed_point(
        &no_dtd,
        &[
            "script",
            "iframe",
            "foreignobject",
            "style",
            "animate",
            "set",
            "feimage",
            "handler",
        ],
    );
    sanitize_attrs_with_policy(&stripped, true)
}

fn strip_tag_blocks_fixed_point(html: &str, tags: &[&str]) -> String {
    let mut cur = strip_tag_blocks(html, tags);
    for _ in 0..3 {
        let next = strip_tag_blocks(&cur, tags);
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

fn strip_dtd_declarations(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut scan = 0;
    while let Some(rel) = lower[scan..].find("<!") {
        let start = scan + rel;
        if lower[start..].starts_with("<!--") {
            scan = comment_end(&lower, start);
            continue;
        }
        if lower[start..].starts_with("<!doctype") || lower[start..].starts_with("<!entity") {
            out.push_str(&input[cursor..start]);
            let mut i = start + 2;
            let mut bracket_depth = 0usize;
            let mut quote = 0u8;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'[' {
                    bracket_depth += 1;
                } else if c == b']' {
                    bracket_depth = bracket_depth.saturating_sub(1);
                } else if c == b'>' && bracket_depth == 0 {
                    i += 1;
                    break;
                }
                i += 1;
            }
            cursor = i;
            scan = i;
            continue;
        }
        scan = start + 2;
    }
    if cursor < input.len() {
        out.push_str(&input[cursor..]);
    }
    out
}

pub(crate) fn comment_end(html: &str, start: usize) -> usize {
    let rest = &html[start..];
    if rest.starts_with("<!-->") {
        return start + 5;
    }
    if rest.starts_with("<!--->") {
        return start + 6;
    }
    let body = &html[start + 4..];
    let a = body.find("-->").map(|p| start + 4 + p + 3);
    let b = body.find("--!>").map(|p| start + 4 + p + 4);
    match (a, b) {
        (Some(x), Some(y)) => x.min(y),
        (x, y) => x.or(y).unwrap_or(html.len()),
    }
}

/// Removes inline event-handler attributes (`on*`) and `javascript:` URLs from
/// `href`/`src`/`action`/`xlink:href`, while leaving the rest of the markup intact.
/// Tag-name stripping alone leaves `onerror`/`javascript:` payloads executable.
pub fn sanitize_dangerous_attrs(html: &str) -> String {
    sanitize_attrs_with_policy(html, false)
}

fn sanitize_attrs_with_policy(html: &str, svg_local_refs_only: bool) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < bytes.len() {
        // Copy text (and comments, which can't execute) up to the next tag-looking `<`.
        if bytes[i] != b'<' {
            let next = bytes[i..]
                .iter()
                .position(|&b| b == b'<')
                .map_or(bytes.len(), |r| i + r);
            out.push_str(&html[i..next]);
            i = next;
            continue;
        }
        if html[i..].starts_with("<!--") {
            let end = comment_end(html, i);
            out.push_str(&html[i..end]);
            i = end;
        } else if bytes
            .get(i + 1)
            .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?'))
        {
            // Quote-aware: a `>` inside `title="a>b"` must not end the tag early and hide a later `onclick`.
            let Some(end) = tag_end(bytes, i + 1) else {
                // EOF inside a tag: HTML parsers drop it, so drop the rest instead of passing it unsanitized.
                break;
            };
            out.push_str(&sanitize_tag(&html[i..=end], svg_local_refs_only));
            i = end + 1;
        } else {
            // `a < b`: a lone `<` is text, not a tag.
            out.push('<');
            i += 1;
        }
    }
    out
}

/// Index of the `>` closing the tag whose body starts at `from`, following HTML5 attribute quote rules.
pub(crate) fn tag_end(b: &[u8], from: usize) -> Option<usize> {
    scan_tag_end(b, from, false)
}

fn scan_tag_end(b: &[u8], from: usize, stop_on_unquoted_lt: bool) -> Option<usize> {
    if matches!(b.get(from), Some(b'!' | b'?')) {
        return b[from..].iter().position(|&c| c == b'>').map(|r| from + r);
    }
    let mut quote = 0u8;
    let mut in_tag_name = true;
    let mut after_eq = false;
    for (i, &c) in b.iter().enumerate().skip(from) {
        if quote != 0 {
            if c == quote {
                quote = 0;
                after_eq = false;
            }
            continue;
        }
        if stop_on_unquoted_lt && c == b'<' {
            return None;
        }
        if c == b'>' {
            return Some(i);
        }
        if in_tag_name {
            if c.is_ascii_whitespace() || c == b'/' {
                in_tag_name = false;
            }
            continue;
        }
        if c == b'=' {
            after_eq = true;
        } else if c.is_ascii_whitespace() {
            // Keep `after_eq` across whitespace between `=` and the attribute value quote.
        } else if after_eq && (c == b'"' || c == b'\'') {
            quote = c;
            after_eq = false;
        } else {
            after_eq = false;
        }
    }
    None
}

/// `javascript:` / `vbscript:` / `data:text/html` / `rust:` URL, tolerating the tricks browsers
/// accept: leading or embedded tab/newline/control chars and numeric (even semicolon-less /
/// zero-padded) or named entities.
fn is_script_url(v: &str) -> bool {
    let mut s = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(p) = rest.find('&') {
        s.push_str(&rest[..p]);
        rest = &rest[p..];
        if let Some(after_hash) = rest.strip_prefix("&#") {
            let (hex, digits_src) = match after_hash.strip_prefix(['x', 'X']) {
                Some(h) => (true, h),
                None => (false, after_hash),
            };
            let digit_len = digits_src
                .bytes()
                .take_while(|b| if hex { b.is_ascii_hexdigit() } else { b.is_ascii_digit() })
                .count();
            if digit_len > 0 {
                let raw_digits = &digits_src[..digit_len];
                let trimmed = raw_digits.trim_start_matches('0');
                let code = if trimmed.is_empty() {
                    Some(0u32)
                } else if trimmed.len() <= 8 {
                    u32::from_str_radix(trimmed, if hex { 16 } else { 10 }).ok()
                } else {
                    None
                };
                let consumed = (if hex { 3 } else { 2 })
                    + digit_len
                    + usize::from(digits_src.as_bytes().get(digit_len) == Some(&b';'));
                if let Some(ch) = code.and_then(char::from_u32) {
                    s.push(ch);
                }
                rest = &rest[consumed..];
                continue;
            }
        }
        let Some(semi) = rest.find(';').filter(|&e| e <= 10) else {
            s.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = rest[1..semi].to_ascii_lowercase();
        match name.as_str() {
            "colon" => s.push(':'),
            "tab" | "newline" => {}
            _ => s.push_str(&rest[..=semi]),
        }
        rest = &rest[semi + 1..];
    }
    s.push_str(rest);
    let s: String = s
        .chars()
        .filter(|c| !c.is_ascii_control() && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    s.starts_with("javascript:")
        || s.starts_with("vbscript:")
        || s.starts_with("data:text/html")
        || s.starts_with("data:image/svg+xml")
        || s.starts_with("data:application/xhtml+xml")
        || s.starts_with("rust:")
}

fn sanitize_tag(tag: &str, svg_local_refs_only: bool) -> String {
    if tag.starts_with("</") || tag.starts_with("<!") || tag.starts_with("<?") {
        return tag.to_string();
    }
    let bytes = tag.as_bytes();
    let mut out = String::with_capacity(tag.len());
    let mut i = 0;
    // Copy `<tagname` verbatim (by slice: pushing bytes as chars would mangle multi-byte names).
    i += 1;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
        i += 1;
    }
    out.push_str(&tag[..i]);
    loop {
        // Copy whitespace.
        let ws_start = i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        out.push_str(&tag[ws_start..i]);
        if i >= bytes.len() || bytes[i] == b'>' {
            out.push_str(&tag[i..]);
            break;
        }
        if bytes[i] == b'/' {
            // Only `/>` or trailing `/` closes the tag; `/` mid-tag acts as an attribute separator in HTML5.
            if i + 1 >= bytes.len() || bytes[i + 1] == b'>' {
                out.push_str(&tag[i..]);
                break;
            }
            i += 1;
            out.push(' ');
            continue;
        }
        // Parse attribute name.
        let name_start = i;
        while i < bytes.len()
            && bytes[i] != b'='
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'>'
            && bytes[i] != b'/'
        {
            i += 1;
        }
        let name = tag[name_start..i].to_ascii_lowercase();
        // Skip whitespace before `=`.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value: Option<String> = None;
        let mut raw_attr = tag[name_start..i].to_string();
        if i < bytes.len() && bytes[i] == b'=' {
            raw_attr.push('=');
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                raw_attr.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i] as char;
                raw_attr.push(q);
                i += 1;
                let v_start = i;
                while i < bytes.len() && (bytes[i] as char) != q {
                    i += 1;
                }
                value = Some(tag[v_start..i].to_string());
                raw_attr.push_str(&tag[v_start..i]);
                if i < bytes.len() {
                    raw_attr.push(q);
                    i += 1;
                }
            } else {
                let v_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                value = Some(tag[v_start..i].to_string());
                raw_attr.push_str(&tag[v_start..i]);
            }
        }
        // Drop event handlers and inline frame documents entirely.
        if (name.starts_with("on") && name.len() > 2) || name == "srcdoc" {
            continue;
        }
        if name == "http-equiv"
            && value
                .as_deref()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("refresh"))
        {
            continue;
        }
        // Neutralize dangerous URLs (and external resource beacons inside untrusted SVGs).
        if matches!(
            name.as_str(),
            "href" | "src" | "action" | "xlink:href" | "formaction" | "data" | "poster"
        ) && let Some(v) = &value
        {
            if is_script_url(v) {
                continue;
            }
            if svg_local_refs_only && !v.trim().starts_with('#') {
                continue;
            }
        }
        out.push_str(&raw_attr);
    }
    out
}

/// Rewrites `http://` subresource/form URLs to `https://` on https pages
/// (upgrade-insecure-requests lite). Plain hyperlinks (`<a href>`) are left alone
/// so navigation targets never change; only `src`/`srcset`/`poster`/`data`,
/// form `action`/`formaction`, and `<link href>` (stylesheets/icons) are upgraded.
pub fn upgrade_mixed_content(html: &str, page_is_https: bool) -> String {
    if !page_is_https {
        return html.to_string();
    }
    let lower = html.to_ascii_lowercase();
    if !lower.contains("http://") {
        return html.to_string();
    }
    const ATTRS: &[&str] = &["src", "srcset", "poster", "data", "action", "formaction"];
    let bytes = lower.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("http://") {
        let abs = pos + rel;
        let mut replaced = false;
        // Must sit inside a tag: no `>` between the last `<` and here.
        if let Some(tag_start) = lower[..abs].rfind('<')
            && !lower[tag_start..abs].contains('>')
        {
            // Expect: attr-name WS* `=` WS* quote directly before `http://`.
            let mut q = abs;
            // step back over the quote
            if q > 0 && (bytes[q - 1] == b'"' || bytes[q - 1] == b'\'') {
                q -= 1;
                while q > tag_start && bytes[q - 1].is_ascii_whitespace() {
                    q -= 1;
                }
                if q > tag_start && bytes[q - 1] == b'=' {
                    q -= 1;
                    while q > tag_start && bytes[q - 1].is_ascii_whitespace() {
                        q -= 1;
                    }
                    let name_end = q;
                    while q > tag_start && !bytes[q - 1].is_ascii_whitespace() && bytes[q - 1] != b'<' {
                        q -= 1;
                    }
                    let name = &lower[q..name_end];
                    let is_link_href = name == "href" && lower[tag_start..abs].starts_with("<link");
                    if ATTRS.contains(&name) || is_link_href {
                        out.push_str(&html[pos..abs]);
                        out.push_str("https://");
                        pos = abs + "http://".len();
                        replaced = true;
                    }
                }
            }
        }
        if !replaced {
            out.push_str(&html[pos..abs + "http://".len()]);
            pos = abs + "http://".len();
        }
    }
    out.push_str(&html[pos..]);
    out
}

/// Decode a fetched document to UTF-8: BOM, then `<meta charset>` sniffing, then UTF-8,
/// falling back to EUC-KR (windows-949) for undeclared legacy Korean pages.
pub fn decode_html_bytes(bytes: &[u8]) -> String {
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    if let Some(enc) = sniff_meta_charset(&bytes[..bytes.len().min(2048)])
        && enc != encoding_rs::UTF_8
    {
        return enc.decode_without_bom_handling(bytes).0.into_owned();
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
    let mut pos = 0;
    while let Some(rel) = find_bytes(&lower[pos..], b"<meta") {
        let after_name = pos + rel + 5;
        pos = after_name;
        if !lower
            .get(after_name)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/')
        {
            continue;
        }
        let tag_end = find_bytes(&lower[after_name..], b">").map_or(lower.len(), |r| after_name + r);
        pos = tag_end;
        let tag = &lower[after_name..tag_end];
        let mut from = 0;
        while let Some(crel) = find_bytes(&tag[from..], b"charset") {
            let cpos = from + crel;
            let prev_ok = cpos == 0 || matches!(tag[cpos - 1], b' ' | b'\t' | b'\r' | b'\n' | b';' | b'"' | b'\'');
            let mut i = cpos + 7;
            from = i;
            if !prev_ok {
                continue;
            }
            while i < tag.len() && tag[i].is_ascii_whitespace() {
                i += 1;
            }
            if tag.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while i < tag.len() && matches!(tag[i], b' ' | b'\t' | b'"' | b'\'') {
                i += 1;
            }
            let start = i;
            while i < tag.len() && (tag[i].is_ascii_alphanumeric() || matches!(tag[i], b'-' | b'_')) {
                i += 1;
            }
            if i > start
                && let Some(enc) = encoding_rs::Encoding::for_label(&tag[start..i])
            {
                return Some(enc);
            }
        }
    }
    None
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Error for PDF responses without a `.pdf` extension; the app hands those to WebView2's PDF viewer.
pub const PDF_RESPONSE_ERR: &str = "PDF 문서입니다.";
/// Error for image/archive responses; the app hands those to WebView2's built-in viewers.
pub const NOT_HTML_ERR: &str = "HTML 문서가 아닌 바이너리/이미지/PDF 파일입니다.";

/// Pages embedding a social or enterprise SSO sign-in SDK (Google/Apple/Kakao/Naver/Okta/Auth0),
/// Keycloak/SAML sign-in forms, or Cloudflare-style JS gates only work with scripts and WebView2 cookies.
/// Deliberately specific: a generic `type="password"` or "enable JavaScript" `<noscript>` notice
/// appears on a large share of ordinary pages (header login boxes) and would turn Clean mode off.
pub fn uses_social_login_sdk(raw_html: &str) -> bool {
    const MARKERS: &[&str] = &[
        "accounts.google.com/gsi/client",
        "apis.google.com/js/platform.js",
        "apis.google.com/js/api:client",
        "accounts.google.com/o/oauth2",
        "appleid.cdn-apple.com/appleauth",
        "developers.kakao.com/sdk",
        "static.nid.naver.com/js/naveridlogin",
        "okta-signin-widget",
        "id=\"okta-login-container\"",
        "cdn.auth0.com/js/",
    ];
    const INTERACTIVE_JS_MARKERS: &[&str] = &[
        "id=\"kc-form-login\"",
        "class=\"login-pf\"",
        "name=\"samlrequest\"",
        "name=\"samlresponse\"",
        "cf-browser-verification",
        "/cdn-cgi/challenge-platform",
        "enable javascript and cookies",
        "data-sap-theme=",
        "id=\"__nuxt\"",
    ];
    let lower = raw_html.to_ascii_lowercase();
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    lower.contains("<script") && INTERACTIVE_JS_MARKERS.iter().any(|m| lower.contains(m))
}

/// A local `.html` you opened yourself (resume builder, dashboard, tool) that runs scripts or has
/// inline handlers is an app, not an article: Clean mode would strip its JS and kill every button.
pub fn is_local_script_app(url: &str, raw_html: &str) -> bool {
    if !url.starts_with("file://") {
        return false;
    }
    let lower = raw_html.to_ascii_lowercase();
    // `<script type="application/json">` data blocks alone don't count.
    let runs_js = lower.match_indices("<script").any(|(i, _)| {
        let tag = &lower[i..lower[i..].find('>').map_or(lower.len(), |e| i + e)];
        !(tag.contains("application/json") || tag.contains("ld+json"))
    });
    runs_js || lower.contains(" onclick=")
}

/// Decode, classify and clean a fetched document in one go (runs on the network thread).
/// Returns `(clean_html, is_spa_shell)`.
pub fn process_fetched_document(bytes: &[u8], url: &str, strip_iframes: bool) -> Result<(String, bool), String> {
    if bytes.starts_with(b"%PDF-") {
        return Err(PDF_RESPONSE_ERR.to_string());
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"GIF8")
    {
        return Err(NOT_HTML_ERR.to_string());
    }
    let text = decode_html_bytes(bytes);
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Err("빈 응답을 받았습니다.".to_string());
    }
    let head_end = trimmed
        .char_indices()
        .take_while(|(idx, _)| *idx < 512)
        .last()
        .map_or(0, |(idx, ch)| idx + ch.len_utf8());
    let head_lower = trimmed[..head_end].to_ascii_lowercase();
    let looks_like_html = trimmed.starts_with('<')
        || head_lower.contains("<!doctype")
        || head_lower.contains("<html")
        || head_lower.contains("<head")
        || head_lower.contains("<body");
    if looks_like_html {
        let spa = is_js_spa_shell(&text) || uses_social_login_sdk(&text) || is_local_script_app(url, &text);
        let cleaned = strip_scripts_and_bloat(&text, strip_iframes);
        let cleaned = upgrade_mixed_content(&cleaned, url.starts_with("https://"));
        Ok((cleaned, spa))
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

fn is_tag_boundary(b: Option<&u8>) -> bool {
    b.is_some_and(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/')
}

/// Scans from `from` (just after `<tag`) to the closing `>` of the opening tag, respecting quotes.
/// Returns `(gt_index, is_self_closing)`.
fn find_open_tag_end(bytes: &[u8], from: usize) -> Option<(usize, bool)> {
    let mut i = from;
    let mut quote: Option<u8> = None;
    let mut after_eq = false;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
                after_eq = false;
            }
        } else if b == b'=' {
            after_eq = true;
        } else if b.is_ascii_whitespace() {
            // Preserve `after_eq` across whitespace.
        } else if after_eq && (b == b'"' || b == b'\'') {
            quote = Some(b);
            after_eq = false;
        } else if b == b'>' {
            // Trim whitespace before `>` to see if a `/` closes the tag; ignore `/` that ends
            // an unquoted attribute token (e.g. `data-url=https://a.test/>`).
            let mut end = i;
            while end > from && bytes[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            let self_close = end > from
                && bytes[end - 1] == b'/'
                && (end - 1 == from || bytes[end - 2].is_ascii_whitespace() || matches!(bytes[end - 2], b'"' | b'\''));
            return Some((i, self_close));
        } else {
            after_eq = false;
        }
        i += 1;
    }
    None
}

fn find_matching_close_tag(lower: &str, lb: &[u8], tag: &str, from: usize) -> usize {
    let raw_text = matches!(tag, "script" | "style" | "noscript" | "iframe");
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}");
    let mut scan = from;
    let mut depth = 1usize;

    while let Some(rel) = lower[scan..].find('<') {
        let pos = scan + rel;
        if !raw_text && lower[pos..].starts_with("<!--") {
            scan = comment_end(lower, pos);
            continue;
        }
        if lower[pos..].starts_with(&close_pat) && is_tag_boundary(lb.get(pos + close_pat.len())) {
            let gt = lower[pos + close_pat.len()..]
                .find('>')
                .map_or(lower.len(), |r| pos + close_pat.len() + r + 1);
            depth -= 1;
            if depth == 0 || raw_text {
                return gt;
            }
            scan = gt;
            continue;
        }
        if !raw_text && lower[pos..].starts_with(&open_pat) && is_tag_boundary(lb.get(pos + open_pat.len())) {
            let after_open = pos + open_pat.len();
            if let Some((te, self_close)) = find_open_tag_end(lb, after_open) {
                if !self_close {
                    depth += 1;
                }
                scan = te + 1;
                continue;
            }
            return lower.len();
        }
        if !raw_text
            && lb
                .get(pos + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?'))
            && let Some(te) = scan_tag_end(lb, pos + 1, true)
        {
            scan = te + 1;
            continue;
        }
        scan = pos + 1;
    }
    lower.len()
}

/// Removes `<tag ...>...</tag>` blocks for every tag in `tags` in a single pass.
pub(crate) fn strip_tag_blocks(input: &str, tags: &[&str]) -> String {
    let lower = input.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let mut scan = 0usize;

    while let Some(rel) = lower[scan..].find('<') {
        let start = scan + rel;
        if lower[start..].starts_with("<!--") {
            scan = comment_end(&lower, start);
            continue;
        }
        scan = start + 1;
        let Some(tag) = tags.iter().find(|t| {
            let after = start + 1 + t.len();
            lower[start + 1..].starts_with(**t) && is_tag_boundary(lb.get(after))
        }) else {
            if lb
                .get(start + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?'))
                && let Some(te) = scan_tag_end(lb, start + 1, true)
            {
                scan = te + 1;
            }
            continue;
        };
        let after_tag = start + 1 + tag.len();
        out.push_str(&input[cursor..start]);

        if let Some((te, self_close)) = find_open_tag_end(lb, after_tag) {
            if self_close {
                cursor = te + 1;
                scan = cursor;
                continue;
            }
            cursor = find_matching_close_tag(&lower, lb, tag, te + 1);
        } else {
            cursor = input.len();
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
        let end = comment_end(input, start);
        if end - start <= max_keep {
            out.push_str(&input[start..end]);
        }
        cursor = end;
        if cursor >= input.len() {
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
    let cleaned = strip_tag_blocks(body_fragment, &["script", "style", "noscript", "svg", "title"]);

    let visible_len = count_visible_text_chars(&cleaned);
    let has_empty_spa_mount = lower.contains("id=\"root\"></div>")
        || lower.contains("id='root'></div>")
        || lower.contains("id=\"app\"></div>")
        || lower.contains("id='app'></div>")
        || lower.contains("id=\"__next\"></div>");

    if has_empty_spa_mount && visible_len < 600 {
        return true;
    }

    if visible_len == 0
        && !body_fragment.contains("<svg")
        && !cleaned.contains("<img")
        && !cleaned.contains("<video")
        && !cleaned.contains("<audio")
        && !cleaned.contains("<iframe")
        && !cleaned.contains("<canvas")
        && !cleaned.contains("<object")
        && !cleaned.contains("<embed")
        && !cleaned.contains("<input")
        && !cleaned.contains("<form")
    {
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
    let stripped = strip_tag_blocks(html, &["script", "style", "svg"]);
    let lower = stripped.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut scan = 0;
    while let Some(rel) = lower[scan..].find("<title") {
        let start = scan + rel;
        let after = start + 6;
        if !is_tag_boundary(lb.get(after)) {
            scan = after;
            continue;
        }
        let gt = lower[after..].find('>')? + after + 1;
        let end = lower[gt..].find("</title>")? + gt;
        let raw = stripped[gt..end].trim();
        return if raw.is_empty() {
            None
        } else {
            Some(decode_basic_entities(raw))
        };
    }
    None
}

pub(crate) fn decode_basic_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
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

pub fn build_reader_html_with_options(raw_html: &str, url: &str, opts: ReaderOptions, os_dark: bool) -> String {
    let title = extract_title(raw_html).unwrap_or_else(|| url.to_string());
    let cleaned = strip_tag_blocks_fixed_point(
        raw_html,
        &[
            "script", "style", "noscript", "nav", "aside", "footer", "header", "iframe",
        ],
    );

    // Extract the primary content container before stripping `<form>` so outer wrapper forms
    // (e.g. ASP.NET WebForms) do not wipe out `<article>`/`<main>`.
    let body_fragment = sanitize_dangerous_attrs(&strip_tag_blocks_fixed_point(
        extract_primary_fragment(&cleaned),
        &["form"],
    ));
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
        ReaderTheme::Auto => {
            if os_dark {
                ReaderTheme::Dark
            } else {
                ReaderTheme::Light
            }
        }
        other => other,
    };
    let (bg, bar_bg, text, muted, link, border, code_bg, active_bg, active_fg) = match eff_theme {
        ReaderTheme::Sepia => (
            "#f6eedb", "#ede4d0", "#382c22", "#756352", "#9a4d16", "#dfd2b7", "#ede4d0", "#9a4d16", "#fcf8ef",
        ),
        ReaderTheme::Dark => (
            "#12141a",
            "#171a21",
            "#e6e8ee",
            "#9299a8",
            "#7c8cff",
            "#252a36",
            "#1b1f28",
            "rgba(124, 140, 255, 0.16)",
            "#cdd5ff",
        ),
        ReaderTheme::Light | ReaderTheme::Auto => (
            "#f8f9fb",
            "#ffffff",
            "#16181d",
            "#5c6370",
            "#4f63f6",
            "#e4e7ee",
            "#eef0f4",
            "rgba(79, 99, 246, 0.11)",
            "#3444c2",
        ),
    };
    let font_stack = match opts.font {
        ReaderFont::Sans => {
            r#""KoPubWorldDotum", "KoPubWorld돋움체", "KoPubWorldDotum Medium", "KoPub Dotum", "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", "Apple SD Gothic Neo", "Noto Sans KR", "Malgun Gothic", sans-serif"#
        }
        ReaderFont::Serif => r#""Noto Serif KR", "Batang", "Georgia", "Charter", "AppleMyungjo", serif"#,
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
    padding: 24px 24px 88px;
    background: {bg};
    color: {text};
    font-family: {font_stack};
    line-height: 1.78;
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
    padding: 7px 11px;
    margin-bottom: 32px;
    border-radius: 12px;
    background: {bar_bg};
    border: 1px solid {border};
    box-shadow: 0 2px 8px rgba(15, 23, 42, 0.05);
    font-family: "KoPubWorldDotum", "KoPubWorld돋움체", "KoPubWorldDotum Medium", "KoPub Dotum", "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", "Malgun Gothic", sans-serif;
    font-size: 12px;
  }}
  .rgroup {{ display: flex; align-items: center; gap: 3px; }}
  .rsep {{ width: 1px; height: 15px; background: {border}; margin: 0 4px; }}
  .rpill {{
    display: inline-flex;
    align-items: center;
    height: 26px;
    padding: 0 9px;
    border-radius: 7px;
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
    font-family: "KoPubWorldDotum", "KoPubWorld돋움체", "KoPubWorldDotum Medium", "KoPub Dotum", "Pretendard Variable", "Pretendard", "Segoe UI Variable Text", "Segoe UI", "Malgun Gothic", sans-serif;
  }}
  .rmeta-chip {{
    padding: 2px 9px;
    border-radius: 6px;
    background: {bar_bg};
    border: 1px solid {border};
    color: {text};
    font-weight: 500;
  }}
  .reader-title {{
    font-size: 1.75em;
    line-height: 1.28;
    font-weight: 700;
    letter-spacing: -0.025em;
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
  .reader-content p {{ margin: 0 0 1.25em; }}
  .reader-content ul, .reader-content ol {{ margin: 0 0 1.25em; padding-left: 1.5em; }}
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
    padding: 12px 18px;
    border-radius: 8px;
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
    let lb = lower.as_bytes();
    for tag in ["<article", "<main", "<body"] {
        let tag_name = &tag[1..];
        let mut scan = 0;
        while let Some(rel) = lower[scan..].find(tag) {
            let start = scan + rel;
            let after = start + tag.len();
            if !is_tag_boundary(lb.get(after)) {
                scan = after;
                continue;
            }
            if let Some((gt, self_close)) = find_open_tag_end(lb, after) {
                if self_close {
                    scan = gt + 1;
                    continue;
                }
                let inner_start = gt + 1;
                let close_start = find_matching_close_tag_start(&lower, lb, tag_name, inner_start);
                return &html[inner_start..close_start];
            }
            break;
        }
    }
    html
}

/// Byte index of the `<` in the matching `</tag>` for an opening `<tag>` whose body starts at `from`.
/// Tracks nested `<tag>` elements instead of grabbing up to the last `</tag>` in the document.
fn find_matching_close_tag_start(lower: &str, lb: &[u8], tag: &str, from: usize) -> usize {
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}");
    let mut scan = from;
    let mut depth = 1usize;

    while let Some(rel) = lower[scan..].find('<') {
        let pos = scan + rel;
        if lower[pos..].starts_with(&close_pat) && is_tag_boundary(lb.get(pos + close_pat.len())) {
            depth -= 1;
            if depth == 0 {
                return pos;
            }
            scan = lower[pos + close_pat.len()..]
                .find('>')
                .map_or(lower.len(), |r| pos + close_pat.len() + r + 1);
            continue;
        }
        if lower[pos..].starts_with(&open_pat) && is_tag_boundary(lb.get(pos + open_pat.len())) {
            let after_open = pos + open_pat.len();
            if let Some((te, self_close)) = find_open_tag_end(lb, after_open) {
                if !self_close {
                    depth += 1;
                }
                scan = te + 1;
                continue;
            }
            return lower.len();
        }
        scan = pos + 1;
    }
    lower.len()
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
        let (bytes, _, _) =
            encoding_rs::EUC_KR.encode("<html><head><meta charset=\"euc-kr\"></head><body>한글</body></html>");
        assert!(decode_html_bytes(&bytes).contains("한글"));
        let (bare, _, _) = encoding_rs::EUC_KR.encode("<p>안녕하세요</p>");
        assert!(decode_html_bytes(&bare).contains("안녕하세요"));
        assert_eq!(decode_html_bytes("<p>é</p>".as_bytes()), "<p>é</p>");
    }

    #[test]
    fn strips_event_handlers_and_javascript_urls() {
        let raw = r#"<p>Hello</p><img src="x.png" onerror="alert(1)"><a href="javascript:alert(2)">x</a><a href="https://a.test">ok</a>"#;
        let out = strip_scripts_and_bloat(raw, true);
        assert!(!out.contains("onerror"));
        assert!(!out.contains("javascript:"));
        assert!(out.contains("https://a.test"));
        // Multibyte text survives sanitization intact.
        let kr = "<p>안녕하세요</p><img src=\"x.png\" onerror=\"alert(1)\">";
        let kr_out = sanitize_dangerous_attrs(kr);
        assert!(kr_out.contains("안녕하세요"));
        assert!(!kr_out.contains("onerror"));
    }

    #[test]
    fn sanitizer_handles_quoted_gt_obfuscated_urls_and_lone_lt() {
        // `>` inside a quoted attribute must not end the tag before the handler is seen.
        let out = sanitize_dangerous_attrs(r#"<a title="x>y" onclick="evil()" href="/ok">t</a>"#);
        assert!(!out.contains("onclick") && out.contains(r#"title="x>y""#) && out.contains(r#"href="/ok""#));
        // Whitespace / entity tricks in the scheme.
        for bad in [
            "java	script:alert(1)",
            "&#106;avascript:alert(1)",
            "javascript&colon;alert(1)",
            " 
 JaVaScRiPt:x",
            "&#x6A;avascript:x",
            "rust://history?clear=1",
            "rust&colon;//open-js?url=file:///C:/x",
        ] {
            let out = sanitize_dangerous_attrs(&format!(r#"<a href="{bad}">x</a>"#));
            assert!(!out.contains("href"), "{bad} -> {out}");
        }
        assert!(sanitize_dangerous_attrs(r#"<a href="https://a.test/?q=a&amp;b=1">x</a>"#).contains("href"));
        // A lone `<` is text and must not swallow (or skip sanitizing) what follows.
        let out = sanitize_dangerous_attrs(r#"1 < 2 "q" <img src=x onerror=alert(1)>"#);
        assert!(out.starts_with(r#"1 < 2 "q" "#) && !out.contains("onerror"));
        // Unterminated tag is dropped; comments pass through untouched; multi-byte tag names survive.
        assert_eq!(sanitize_dangerous_attrs(r#"<p>a</p><img onerror="x"#), "<p>a</p>");
        assert!(sanitize_dangerous_attrs("<!-- it's > a \"comment\" --><p>b</p>").ends_with("--><p>b</p>"));
        assert!(sanitize_dangerous_attrs("<a가 onclick=x>t</a가>").contains("<a가 >t"));
    }

    #[test]
    fn upgrades_subresources_but_not_hyperlinks_on_https_pages() {
        let raw = r#"<img src="http://a.test/i.png"><link rel="stylesheet" href="http://a.test/s.css"><form action="http://a.test/submit"><a href="http://a.test/page">x</a><p>http://a.test/text</p>"#;
        let out = upgrade_mixed_content(raw, true);
        assert!(out.contains(r#"src="https://a.test/i.png""#));
        assert!(out.contains(r#"href="https://a.test/s.css""#));
        assert!(out.contains(r#"action="https://a.test/submit""#));
        assert!(out.contains(r#"<a href="http://a.test/page">"#));
        assert!(out.contains("<p>http://a.test/text</p>"));
        assert_eq!(upgrade_mixed_content(raw, false), raw);
        // Uppercase scheme in subresource attributes is also upgraded.
        assert!(
            upgrade_mixed_content(r#"<img src="HTTP://a.test/i.png">"#, true).contains(r#"src="https://a.test/i.png""#)
        );
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

        let okta_like = r#"<!doctype html><html><head><script src="https://ok6static2.oktacdn.com/assets/js/sdk/okta-signin-widget/7.51.0/js/okta-sign-in.min.js"></script></head><body><div id="okta-login-container"></div><div class="hidden">Cookies are required. Cookies are disabled on your browser. Please enable Cookies and refresh this page to continue signing in.</div></body></html>"#;
        assert!(uses_social_login_sdk(okta_like));

        let keycloak_like = r#"<!doctype html><html><head><script src="/auth/main.js"></script></head><body><form id="kc-form-login" method="post"><input type="text" name="username"><input type="password" name="password"></form><p>Long legal notice text...</p></body></html>"#;
        assert!(uses_social_login_sdk(keycloak_like));

        let cf_gate = r#"<!doctype html><html><head><script src="/cdn-cgi/challenge-platform/h/b/orchestrate"></script></head><body><p>Enable JavaScript and cookies to continue</p></body></html>"#;
        assert!(uses_social_login_sdk(cf_gate));
        assert!(!uses_social_login_sdk(article_like));

        let header_login_article = r#"<!doctype html><html><head><script src="/app.js"></script></head><body><form class="login"><input type="password" name="pw"></form><noscript>Please enable JavaScript for comments.</noscript><article><h1>News</h1><p>Plenty of readable article text here.</p></article></body></html>"#;
        assert!(!uses_social_login_sdk(header_login_article));

        let small_redirect_shell = r#"<html><head><title>Studio</title><script>window.location = 'signin?locale=en_US';</script></head><body></body></html>"#;
        assert!(is_js_spa_shell(small_redirect_shell));

        let image_only_with_script = r#"<html><body><img src="photo.png"><script>init()</script></body></html>"#;
        assert!(!is_js_spa_shell(image_only_with_script));
    }

    #[test]
    fn local_script_apps_open_in_js_mode() {
        let app = r#"<html><body><button onclick="go()">x</button><script>function go(){}</script></body></html>"#;
        assert!(is_local_script_app("file:///C:/a/index.html", app));
        assert!(!is_local_script_app("https://e.test/", app));
        let data_only = r#"<html><body><p>hi</p><script type="application/json">{}</script></body></html>"#;
        assert!(!is_local_script_app("file:///C:/a.html", data_only));
        assert!(!is_local_script_app(
            "file:///C:/a.html",
            "<html><body>static</body></html>"
        ));
    }

    #[test]
    fn ignores_charset_outside_meta_and_handles_short_and_nested_html() {
        let utf8_with_script_charset = r#"<!doctype html><html><head><script src="legacy.js" charset="euc-kr"></script></head><body><h1>한글 정상 출력</h1></body></html>"#;
        assert_eq!(
            decode_html_bytes(utf8_with_script_charset.as_bytes()),
            utf8_with_script_charset
        );

        let short_prefixed = "  note <!DOCTYPE html><html><body><p>Hi</p></body></html>";
        let (cleaned, _) = process_fetched_document(short_prefixed.as_bytes(), "https://a.test", true).unwrap();
        assert!(
            !cleaned.contains("<pre>"),
            "short HTML with prefix must not become <pre>: {cleaned}"
        );

        let boundary_prefixed = format!(
            "{}<!DOCTYPE html><html><body><p>OK {}</p></body></html>",
            "가".repeat(150),
            "가".repeat(20)
        );
        let (cleaned2, _) = process_fetched_document(boundary_prefixed.as_bytes(), "https://a.test", true).unwrap();
        assert!(!cleaned2.contains("<pre>"));

        let tricky_script =
            r#"<p>a</p><script data-url=https://a.test/>const x="</scripts>";secret();</script><p>b</p>"#;
        assert_eq!(strip_scripts_and_bloat(tricky_script, true), "<p>a</p><p>b</p>");

        let nested_and_wrapped = r#"<html><head><script>var t="<title>Fake</title>";</script><title>Real</title></head><body><header><header>inner</header>outer</header><form id="aspnetForm"><article-card>skip</article-card><main><p>Main body</p><form><input/></form></main></form></body></html>"#;
        let reader = build_reader_html(nested_and_wrapped, "https://a.test");
        assert!(reader.contains("Real") && !reader.contains("Fake"));
        assert!(!reader.contains("outer") && !reader.contains("inner"));
        assert!(!reader.contains("<form"));
        assert!(reader.contains("Main body"));

        // Sibling/nested `<article>` blocks: only the first article (with its nested child) is kept.
        let multi = r#"<html><body><article><p>First</p><article><p>Nested</p></article></article><div class="ad">Ad noise</div><article><p>Second</p></article></body></html>"#;
        let reader_multi = build_reader_html(multi, "https://a.test");
        assert!(reader_multi.contains("First") && reader_multi.contains("Nested"));
        assert!(!reader_multi.contains("Ad noise") && !reader_multi.contains("Second"));

        assert_eq!(
            process_fetched_document(b"%PDF-1.7\n...", "https://a.test/doc", true).unwrap_err(),
            PDF_RESPONSE_ERR
        );
        assert_eq!(
            process_fetched_document(b"PK\x03\x04...", "https://a.test/archive", true).unwrap_err(),
            NOT_HTML_ERR
        );
    }

    #[test]
    fn closes_slash_separator_quote_differential_and_tag_reconstruction_bypasses() {
        // `/` mid-tag acts as an attribute separator in HTML5.
        let slash = strip_scripts_and_bloat(
            r#"<img/onerror="alert(1)" src="x"><img src="x"/onerror="alert(1)"><a/href="rust://history?clear=1">x</a>"#,
            true,
        );
        assert!(!slash.contains("onerror"), "{slash}");
        assert!(!slash.contains("rust://"), "{slash}");

        // Quote inside an unquoted attribute must not desync `tag_end` vs `sanitize_tag`.
        let qdiff = sanitize_dangerous_attrs(r#"<a x=a"><img src=x onerror="alert(1)">"#);
        assert!(!qdiff.contains("onerror"), "{qdiff}");

        // Nested/comment-split tags cannot reassemble `<script>` or `<style>`.
        let nested = strip_scripts_and_bloat("<scr<script></script>ipt>alert(1)</script>", true);
        assert!(!nested.to_ascii_lowercase().contains("<script"), "{nested}");
        let big_comment = format!("<scr<!-- {} -->ipt>alert(1)</script>", "x".repeat(300));
        let split_comment = strip_scripts_and_bloat(&big_comment, true);
        assert!(
            !split_comment.to_ascii_lowercase().contains("<script"),
            "{split_comment}"
        );
        let reader_style = build_reader_html(
            "<article><st<style></style>yle>body{}</style><p>hi</p></article>",
            "https://a.test",
        );
        assert!(!reader_style.contains("body{}"), "{reader_style}");

        // HTML5 comment closures `<!-->` and `--!>`, plus semicolon-less & zero-padded entities.
        let comments = sanitize_dangerous_attrs("<!--><img src=x onerror=1><!-- a --!><img src=y onclick=2>");
        assert!(
            !comments.contains("onerror") && !comments.contains("onclick"),
            "{comments}"
        );
        for bad in [
            "&#106avascript:alert(1)",
            "&#000000000106;avascript:alert(1)",
            "vbscript:msgbox(1)",
            "data:image/svg+xml,<svg onload=1>",
        ] {
            let out = sanitize_dangerous_attrs(&format!(r#"<a href="{bad}">x</a>"#));
            assert!(!out.contains("href"), "{bad} -> {out}");
        }

        // `decode_basic_entities` must not double-unescape `&amp;lt;`.
        assert_eq!(decode_basic_entities("&amp;lt;b&amp;gt;"), "&lt;b&gt;");

        // `sanitize_svg` strips DTD/ENTITY, `<foreignObject>`, and external beacon refs while keeping `#id`.
        let evil_svg = r##"<!DOCTYPE svg [<!ENTITY xxe "bad">]><svg><defs><linearGradient id="g"/></defs><use href="#g"/><image href="https://tracker.test/p.png"/><foreignObject><div onclick="x()">hi</div></foreignObject></svg>"##;
        let clean_svg = sanitize_svg(evil_svg);
        assert!(
            !clean_svg.contains("DOCTYPE") && !clean_svg.contains("ENTITY"),
            "{clean_svg}"
        );
        assert!(
            !clean_svg.contains("foreignObject") && !clean_svg.contains("tracker.test"),
            "{clean_svg}"
        );
        assert!(clean_svg.contains(r##"href="#g""##), "{clean_svg}");
    }
}
