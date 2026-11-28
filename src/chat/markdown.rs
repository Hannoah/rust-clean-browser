use crate::cleaner::html_escape;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ListKind {
    Unordered,
    Ordered,
}

pub(super) fn close_list(out: &mut String, in_list: &mut Option<ListKind>) {
    match in_list.take() {
        Some(ListKind::Unordered) => out.push_str("</ul>"),
        Some(ListKind::Ordered) => out.push_str("</ol>"),
        None => {}
    }
}

pub(super) fn flush_quote(out: &mut String, quote_buf: &mut Vec<String>) {
    if quote_buf.is_empty() {
        return;
    }
    out.push_str("<blockquote>");
    for line in quote_buf.drain(..) {
        if line.is_empty() {
            continue;
        }
        out.push_str("<p>");
        out.push_str(&render_inline_md(&line));
        out.push_str("</p>");
    }
    out.push_str("</blockquote>");
}

pub(super) fn flush_code_block(
    out: &mut String,
    code_lang: &mut String,
    code_buf: &mut String,
    code_blocks: &mut Vec<String>,
) {
    let idx = code_blocks.len();
    let raw_code = std::mem::take(code_buf);
    let escaped_code = html_escape(&raw_code);
    code_blocks.push(raw_code);
    let lang_lbl = if code_lang.is_empty() {
        "code".to_string()
    } else {
        html_escape(code_lang)
    };
    code_lang.clear();
    out.push_str(&format!(
        r#"<div class="chat-code"><div class="chat-code-hdr"><span>{lang_lbl}</span><span class="chat-copy" data-action="chat-copy-code:{idx}">복사</span></div><pre><code>{escaped_code}</code></pre></div>"#
    ));
}

pub(super) fn is_hr_line(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() < 3 {
        return false;
    }
    let first = compact.chars().next().unwrap_or('\0');
    matches!(first, '-' | '*' | '_') && compact.chars().all(|c| c == first)
}

pub(super) fn is_table_separator(t: &str) -> bool {
    !t.is_empty()
        && t.contains('|')
        && t.contains("---")
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

pub(super) fn split_table_cells(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed.strip_prefix('|').unwrap_or(trimmed));
    inner.split('|').map(str::trim).collect()
}

/// Safe subset markdown -> HTML renderer for assistant replies.
pub fn render_markdown_to_html(md: &str, code_blocks: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(md.len() + 256);
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut in_list: Option<ListKind> = None;
    let mut quote_buf: Vec<String> = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0usize;

    while i < lines.len() {
        let raw_line = lines[i];
        let trimmed = raw_line.trim_end();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_code {
                flush_code_block(&mut out, &mut code_lang, &mut code_buf, code_blocks);
                in_code = false;
            } else {
                close_list(&mut out, &mut in_list);
                flush_quote(&mut out, &mut quote_buf);
                in_code = true;
                code_lang = rest.trim().to_string();
                code_buf.clear();
            }
            i += 1;
            continue;
        }
        if in_code {
            if !code_buf.is_empty() {
                code_buf.push('\n');
            }
            code_buf.push_str(raw_line);
            i += 1;
            continue;
        }

        let t = trimmed.trim();
        if t.is_empty() {
            close_list(&mut out, &mut in_list);
            flush_quote(&mut out, &mut quote_buf);
            i += 1;
            continue;
        }
        if let Some(q) = t.strip_prefix('>') {
            close_list(&mut out, &mut in_list);
            quote_buf.push(q.trim_start().to_string());
            i += 1;
            continue;
        }
        flush_quote(&mut out, &mut quote_buf);

        if t.contains('|') && lines.get(i + 1).is_some_and(|next| is_table_separator(next.trim())) {
            close_list(&mut out, &mut in_list);
            let headers = split_table_cells(t);
            out.push_str(r#"<div class="chat-tbl"><table><thead><tr>"#);
            for cell in &headers {
                out.push_str("<th>");
                out.push_str(&render_inline_md(cell));
                out.push_str("</th>");
            }
            out.push_str("</tr></thead><tbody>");
            i += 2;
            while i < lines.len() {
                let row_t = lines[i].trim();
                if row_t.is_empty() || !row_t.contains('|') || row_t.starts_with("```") {
                    break;
                }
                let cells = split_table_cells(row_t);
                out.push_str("<tr>");
                for col in 0..headers.len() {
                    out.push_str("<td>");
                    if let Some(val) = cells.get(col) {
                        out.push_str(&render_inline_md(val));
                    }
                    out.push_str("</td>");
                }
                out.push_str("</tr>");
                i += 1;
            }
            out.push_str("</tbody></table></div>");
            continue;
        }

        if is_hr_line(t) {
            close_list(&mut out, &mut in_list);
            out.push_str("<hr />");
            i += 1;
            continue;
        }
        if let Some(h) = t.strip_prefix("### ") {
            close_list(&mut out, &mut in_list);
            out.push_str("<h4>");
            out.push_str(&render_inline_md(h));
            out.push_str("</h4>");
        } else if let Some(h) = t.strip_prefix("## ") {
            close_list(&mut out, &mut in_list);
            out.push_str("<h3>");
            out.push_str(&render_inline_md(h));
            out.push_str("</h3>");
        } else if let Some(h) = t.strip_prefix("# ") {
            close_list(&mut out, &mut in_list);
            out.push_str("<h3>");
            out.push_str(&render_inline_md(h));
            out.push_str("</h3>");
        } else if let Some(li) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            if in_list != Some(ListKind::Unordered) {
                close_list(&mut out, &mut in_list);
                out.push_str("<ul>");
                in_list = Some(ListKind::Unordered);
            }
            out.push_str("<li>");
            out.push_str(&render_inline_md(li));
            out.push_str("</li>");
        } else if let Some(li) = strip_ordered_prefix(t) {
            if in_list != Some(ListKind::Ordered) {
                close_list(&mut out, &mut in_list);
                out.push_str("<ol>");
                in_list = Some(ListKind::Ordered);
            }
            out.push_str("<li>");
            out.push_str(&render_inline_md(li));
            out.push_str("</li>");
        } else {
            close_list(&mut out, &mut in_list);
            out.push_str("<p>");
            out.push_str(&render_inline_md(t));
            out.push_str("</p>");
        }
        i += 1;
    }

    close_list(&mut out, &mut in_list);
    flush_quote(&mut out, &mut quote_buf);
    if in_code && !code_buf.is_empty() {
        flush_code_block(&mut out, &mut code_lang, &mut code_buf, code_blocks);
    }
    out
}

pub(super) fn strip_ordered_prefix(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && i < 4 && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && bytes.get(i) == Some(&b'.') && bytes.get(i + 1) == Some(&b' ') {
        Some(&line[i + 2..])
    } else {
        None
    }
}

pub(super) fn render_inline_md(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(start) = rest.find('`') {
            out.push_str(&render_links_and_emphasis(&rest[..start]));
            let after = &rest[start + 1..];
            if let Some(end) = after.find('`') {
                out.push_str("<code>");
                out.push_str(&html_escape(&after[..end]));
                out.push_str("</code>");
                rest = &after[end + 1..];
            } else {
                out.push_str(&render_links_and_emphasis(&rest[start..]));
                break;
            }
        } else {
            out.push_str(&render_links_and_emphasis(rest));
            break;
        }
    }
    out
}

pub(super) fn render_links_and_emphasis(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(l_start) = rest.find('[')
            && let Some(l_end_rel) = rest[l_start + 1..].find("](")
        {
            let label = &rest[l_start + 1..l_start + 1 + l_end_rel];
            let url_start = l_start + 1 + l_end_rel + 2;
            if let Some(url_end_rel) = rest[url_start..].find(')') {
                let raw_url = rest[url_start..url_start + url_end_rel].trim();
                if (raw_url.starts_with("https://") || raw_url.starts_with("http://"))
                    && !label.is_empty()
                    && !raw_url.contains('"')
                {
                    out.push_str(&render_bold_and_italic(&rest[..l_start]));
                    let esc_url = html_escape(raw_url);
                    out.push_str(r#"<a href=""#);
                    out.push_str(&esc_url);
                    out.push_str(r#"" title=""#);
                    out.push_str(&esc_url);
                    out.push_str(r#"">"#);
                    out.push_str(&render_bold_and_italic(label));
                    out.push_str("</a>");
                    rest = &rest[url_start + url_end_rel + 1..];
                    continue;
                }
            }
            out.push_str(&render_bold_and_italic(&rest[..=l_start]));
            rest = &rest[l_start + 1..];
            continue;
        }
        out.push_str(&render_bold_and_italic(rest));
        break;
    }
    out
}

pub(super) fn render_bold_and_italic(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(start) = rest.find("**") {
            out.push_str(&render_italic_only(&rest[..start]));
            let after = &rest[start + 2..];
            if let Some(end) = after.find("**") {
                out.push_str("<strong>");
                out.push_str(&render_italic_only(&after[..end]));
                out.push_str("</strong>");
                rest = &after[end + 2..];
            } else {
                out.push_str(&html_escape(&rest[start..]));
                break;
            }
        } else {
            out.push_str(&render_italic_only(rest));
            break;
        }
    }
    out
}

pub(super) fn render_italic_only(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(start) = rest.find('*') {
            let after = &rest[start + 1..];
            if !after.starts_with(char::is_whitespace)
                && let Some(end) = after.find('*')
                && end > 0
                && !after[..end].ends_with(char::is_whitespace)
            {
                out.push_str(&html_escape(&rest[..start]));
                out.push_str("<em>");
                out.push_str(&html_escape(&after[..end]));
                out.push_str("</em>");
                rest = &after[end + 1..];
                continue;
            }
            out.push_str(&html_escape(&rest[..=start]));
            rest = after;
        } else {
            out.push_str(&html_escape(rest));
            break;
        }
    }
    out
}
