//! Embedded pdf.js annotator ("PDF 필기 모드"): highlight, freehand draw, text notes, stamps and
//! save-with-annotations, which WebView2's built-in PDF viewer lacks.
//!
//! The pruned pdf.js viewer (`vendor/pdfjs`, deflated by `build.rs`) is served to the WebView from
//! the custom origin `http://rcbpdf.localhost/`. The PDF itself is fetched by this process (local
//! file, or HTTP(S) with the browser's cookie jar) and served from `/pdf/<token>/<name>.pdf`, so the
//! viewer stays same-origin and no CORS is involved. Tokens are random and only handed out by the
//! app, so web pages can't use the protocol to read local files.

use crate::net::PersistentCookieJar;
use std::borrow::Cow;
use std::io::Read as _;
use std::sync::{Arc, Mutex, OnceLock};
use wry::RequestAsyncResponder;
use wry::http::{Request, Response};

include!(concat!(env!("OUT_DIR"), "/pdfjs_assets.rs")); // FILES: &[(&str, &[u8])] (deflated)

#[cfg(target_os = "windows")]
const ORIGIN: &str = "http://rcbpdf.localhost";
#[cfg(not(target_os = "windows"))]
const ORIGIN: &str = "rcbpdf://localhost";
const MAX_PDF_BYTES: u64 = 256 * 1024 * 1024;

/// `true` when the URL path names a PDF (`.pdf`, any case; query/fragment ignored).
pub fn is_pdf_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.path().to_ascii_lowercase().ends_with(".pdf"))
}

/// Maps random tokens to the PDF source URLs the app chose to open in the annotator.
#[derive(Default)]
pub struct PdfSources(Mutex<Vec<(String, String)>>);

/// Newest tokens kept; older PDF tabs lose their annotator link (reloading the PDF re-issues one).
const MAX_SOURCES: usize = 64;

impl PdfSources {
    /// Viewer URL that shows `src` (a `file://` or `http(s)://` PDF URL).
    pub fn editor_url(&self, src: &str) -> String {
        let token = crate::single::random_token();
        let mut tokens = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let token = match tokens.iter().find(|(_, s)| s == src) {
            Some((t, _)) => t.clone(),
            None => {
                if tokens.len() >= MAX_SOURCES {
                    tokens.remove(0);
                }
                tokens.push((token.clone(), src.to_string()));
                token
            }
        };
        drop(tokens);
        let path = format!("/pdf/{token}/{}", enc(&file_name(src)));
        format!("{ORIGIN}/web/viewer.html?file={}", enc(&path))
    }

    fn get(&self, token: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(t, _)| t == token)
            .map(|(_, s)| s.clone())
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Last path segment of the source, forced to end in `.pdf` (it names the saved file).
fn file_name(src: &str) -> String {
    let seg = url::Url::parse(src)
        .ok()
        .and_then(|u| u.path_segments().and_then(|mut s| s.next_back().map(pct_decode)))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "document".to_string());
    // Windows forbids these in file names.
    let seg: String = seg
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    if seg.to_ascii_lowercase().ends_with(".pdf") {
        seg
    } else {
        format!("{seg}.pdf")
    }
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "mjs" | "js" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "wasm" => "application/wasm",
        "ftl" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn reply(status: u16, content_type: &str, cache_control: &str, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", cache_control)
        .header("X-Content-Type-Options", "nosniff")
        .header("X-Frame-Options", "DENY")
        .header("Content-Security-Policy", "frame-ancestors 'none'")
        .header("Referrer-Policy", "no-referrer")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

fn pdf_http_client(cookies: Arc<PersistentCookieJar>) -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .cookie_provider(cookies)
            .timeout(std::time::Duration::from_secs(60))
            .user_agent("Mozilla/5.0")
            .build()
            .map_err(|e| e.to_string())
    }) {
        Ok(c) => Ok(c),
        Err(e) => Err(e.clone()),
    }
}

fn append_bounded(out: &mut Vec<u8>, chunk: &[u8], limit: u64) -> Result<(), String> {
    if (out.len() as u64).saturating_add(chunk.len() as u64) > limit {
        return Err("PDF가 너무 큼 (256MB 초과)".to_string());
    }
    out.extend_from_slice(chunk);
    Ok(())
}

/// Fetches the raw PDF bytes from a `file://` path or `http(s)://` URL (up to `MAX_PDF_BYTES`).
pub async fn load_pdf_bytes(src: &str, cookies: Arc<PersistentCookieJar>) -> Result<Vec<u8>, String> {
    let url = url::Url::parse(src).map_err(|e| e.to_string())?;
    match url.scheme() {
        "file" => {
            let path = url.to_file_path().map_err(|()| "잘못된 파일 경로".to_string())?;
            tokio::task::spawn_blocking(move || {
                let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                let mut out = Vec::new();
                file.take(MAX_PDF_BYTES + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| e.to_string())?;
                if out.len() as u64 > MAX_PDF_BYTES {
                    return Err("PDF가 너무 큼 (256MB 초과)".to_string());
                }
                Ok(out)
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "http" | "https" => {
            let client = pdf_http_client(cookies)?;
            let mut resp = client
                .get(url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?;
            if resp.content_length().is_some_and(|n| n > MAX_PDF_BYTES) {
                return Err("PDF가 너무 큼 (256MB 초과)".to_string());
            }
            // Stream in chunks so chunked responses without Content-Length also honor MAX_PDF_BYTES.
            let initial_cap = resp.content_length().unwrap_or(0).min(16 * 1024 * 1024) as usize;
            let mut out = Vec::with_capacity(initial_cap);
            while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
                append_bounded(&mut out, &chunk, MAX_PDF_BYTES)?;
            }
            Ok(out)
        }
        other => Err(format!("지원하지 않는 주소: {other}")),
    }
}

/// Handler for the `rcbpdf` custom protocol.
pub fn handle(
    sources: &Arc<PdfSources>,
    cookies: &Arc<PersistentCookieJar>,
    rt: &tokio::runtime::Handle,
    req: Request<Vec<u8>>,
    responder: RequestAsyncResponder,
) {
    if req
        .headers()
        .get("Sec-Fetch-Site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.eq_ignore_ascii_case("cross-site"))
    {
        return responder.respond(reply(403, "text/plain", "no-store", b"forbidden".to_vec()));
    }
    let path = req.uri().path().to_string();
    if let Some(rest) = path.strip_prefix("/pdf/") {
        let token = rest.split('/').next().unwrap_or("");
        let Some(src) = sources.get(token) else {
            return responder.respond(reply(404, "text/plain", "no-store", b"unknown PDF token".to_vec()));
        };
        let name = file_name(&src);
        let cookies = Arc::clone(cookies);
        rt.spawn(async move {
            responder.respond(match load_pdf_bytes(&src, cookies).await {
                Ok(bytes) => Response::builder()
                    .status(200)
                    .header("Content-Type", "application/pdf")
                    .header(
                        "Content-Disposition",
                        format!("inline; filename*=UTF-8''{}", enc(&name)),
                    )
                    .header("Accept-Ranges", "none")
                    .header("Cache-Control", "no-store")
                    .header("X-Content-Type-Options", "nosniff")
                    .header("X-Frame-Options", "DENY")
                    .header("Content-Security-Policy", "frame-ancestors 'none'")
                    .header("Referrer-Policy", "no-referrer")
                    .body(Cow::Owned(bytes))
                    .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..]))),
                Err(e) => reply(502, "text/plain; charset=utf-8", "no-store", e.into_bytes()),
            });
        });
        return;
    }
    let key = path.trim_start_matches('/');
    let found = FILES
        .iter()
        .find(|(n, _)| *n == key)
        .and_then(|(_, z)| miniz_oxide::inflate::decompress_to_vec(z).ok());
    responder.respond(match found {
        Some(body) => reply(200, mime(key), "private, max-age=3600", body),
        None => reply(404, "text/plain", "no-store", b"not found".to_vec()),
    });
}

/// Extracts readable UTF-8 text from raw PDF bytes using `pdf_extract`, formatted by page and
/// safely capped to `max_chars` characters. Returns `Ok("")` when a PDF has no selectable text layer.
pub fn extract_pdf_text(bytes: &[u8], max_chars: usize) -> Result<String, String> {
    if bytes.is_empty() || max_chars == 0 {
        return Ok(String::new());
    }
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || match pdf_extract::extract_text_from_mem_by_pages(bytes) {
            Ok(pages) if !pages.is_empty() => Ok(pages),
            Ok(_) | Err(_) => pdf_extract::extract_text_from_mem(bytes).map(|t| vec![t]),
        },
    ));
    let pages = match res {
        Ok(Ok(pages)) => pages,
        Ok(Err(e)) => return Err(format!("PDF 텍스트 추출 실패: {e}")),
        Err(_) => return Err("손상되었거나 지원하지 않는 PDF 형식입니다.".to_string()),
    };
    let total = pages.len();
    let mut out = String::new();
    let mut truncated = false;
    for (idx, raw) in pages.into_iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let chunk = if total > 1 {
            format!("[Page {}/{}]\n{trimmed}\n\n", idx + 1, total)
        } else {
            format!("{trimmed}\n")
        };
        let cur_chars = out.chars().count();
        let rem = max_chars.saturating_sub(cur_chars);
        if rem == 0 {
            truncated = true;
            break;
        }
        if chunk.chars().count() <= rem {
            out.push_str(&chunk);
        } else {
            out.extend(chunk.chars().take(rem));
            truncated = true;
            break;
        }
    }
    let out = out.trim_end().to_string();
    if truncated && total > 1 {
        Ok(format!("{out}\n\n…(총 {total}쪽 중 앞부분 발췌)"))
    } else {
        Ok(out)
    }
}

/// Runs in every WebView page; only acts on the annotator origin. Turns PDF JavaScript off
/// (a PDF is untrusted input), opens documents fit-to-width, enables selection floating
/// highlights, keeps bulky parameter popovers hidden while annotating, and mounts a slim
/// floating quick pill with note-app keyboard shortcuts.
pub const INIT_SCRIPT: &str = r#"(function () {
  if (location.hostname !== 'rcbpdf.localhost' && location.protocol !== 'rcbpdf:') return;
  try {
    var k = 'pdfjs.preferences', p = JSON.parse(localStorage.getItem(k) || '{}');
    var want = {
      enableScripting: false,
      defaultZoomValue: 'page-width',
      enableHighlightFloatingButton: true
    }, changed = false;
    for (var a in want) if (p[a] !== want[a]) { p[a] = want[a]; changed = true; }
    if (changed) localStorage.setItem(k, JSON.stringify(p));
  } catch (e) {}

  function initAnnotatorUx() {
    try {
      if (document.getElementById('rcb-pdf-pill')) return;
      var hlBtn = document.getElementById('editorHighlightButton');
      var inkBtn = document.getElementById('editorInkButton');
      var txtBtn = document.getElementById('editorFreeTextButton');
      var hlBar = document.getElementById('editorHighlightParamsToolbar');
      var inkBar = document.getElementById('editorInkParamsToolbar');
      var txtBar = document.getElementById('editorFreeTextParamsToolbar');
      var picker = document.getElementById('editorHighlightColorPicker');
      var inkColor = document.getElementById('editorInkColor');
      var inkThick = document.getElementById('editorInkThickness');
      var viewerContainer = document.getElementById('viewerContainer');
      if (!hlBtn || !inkBtn || !txtBtn) return;

      var tools = [
        { key: 'hl', btn: hlBtn, bar: hlBar, label: '형광펜' },
        { key: 'ink', btn: inkBtn, bar: inkBar, label: '그리기' },
        { key: 'txt', btn: txtBtn, bar: txtBar, label: '텍스트' }
      ];

      function hideBar(t) {
        if (t && t.bar) t.bar.classList.add('hidden');
        if (t && t.btn) t.btn.setAttribute('aria-expanded', 'false');
      }
      function hideAllBars() { tools.forEach(hideBar); }
      function activeTool() {
        for (var i = 0; i < tools.length; i++) {
          if (tools[i].btn.classList.contains('toggled')) return tools[i];
        }
        return null;
      }

      var style = document.createElement('style');
      style.textContent =
        '#rcb-pdf-pill{position:fixed;bottom:16px;left:50%;transform:translateX(-50%);z-index:9999;' +
        'display:none;align-items:center;gap:6px;padding:6px 10px;border-radius:999px;' +
        'background:rgba(255,255,255,0.88);backdrop-filter:blur(12px);color:#14161a;' +
        'border:1px solid rgba(0,0,0,0.12);box-shadow:0 8px 24px rgba(0,0,0,0.16);' +
        'font:600 12px/1 system-ui,-apple-system,sans-serif;user-select:none;}' +
        '@media(prefers-color-scheme:dark){#rcb-pdf-pill{background:rgba(28,30,36,0.9);color:#f1f2f5;' +
        'border-color:rgba(255,255,255,0.16);box-shadow:0 8px 24px rgba(0,0,0,0.45);}}' +
        '#rcb-pdf-pill .rcb-lbl{padding:0 4px;opacity:.75;font-size:11px;}' +
        '#rcb-pdf-pill .rcb-dot{width:18px;height:18px;border-radius:999px;border:2px solid transparent;' +
        'padding:0;cursor:pointer;box-shadow:inset 0 0 0 1px rgba(0,0,0,0.18);}' +
        '#rcb-pdf-pill .rcb-dot.on{border-color:#2563eb;transform:scale(1.12);}' +
        '#rcb-pdf-pill .rcb-btn{border:none;background:rgba(128,128,128,0.14);color:inherit;' +
        'height:24px;min-width:24px;padding:0 7px;border-radius:999px;cursor:pointer;font:inherit;}' +
        '#rcb-pdf-pill .rcb-btn.on{background:#2563eb;color:#fff;}' +
        '#rcb-pdf-pill .rcb-sep{width:1px;height:16px;background:rgba(128,128,128,0.28);margin:0 2px;}';
      document.head.appendChild(style);

      var pill = document.createElement('div');
      pill.id = 'rcb-pdf-pill';
      document.body.appendChild(pill);

      function setInputVal(el, val) {
        if (!el) return;
        el.value = String(val);
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }

      function renderPill() {
        var act = activeTool();
        if (!act) {
          pill.style.display = 'none';
          pill.textContent = '';
          return;
        }
        pill.style.display = 'flex';
        pill.textContent = '';

        var lbl = document.createElement('span');
        lbl.className = 'rcb-lbl';
        lbl.textContent = act.label;
        pill.appendChild(lbl);

        if (act.key === 'hl' && picker) {
          var swatches = picker.querySelectorAll('button');
          swatches.forEach(function (sw) {
            var b = document.createElement('button');
            b.type = 'button';
            b.className = 'rcb-dot' + (sw.getAttribute('aria-checked') === 'true' || sw.classList.contains('selected') ? ' on' : '');
            b.title = sw.title || sw.getAttribute('aria-label') || '';
            var bg = sw.style.backgroundColor || getComputedStyle(sw).backgroundColor;
            b.style.backgroundColor = bg || '#fff066';
            b.addEventListener('click', function () {
              sw.click();
              requestAnimationFrame(function () { hideBar(act); renderPill(); });
            });
            pill.appendChild(b);
          });
        } else if (act.key === 'ink') {
          var curColor = (inkColor && inkColor.value || '').toLowerCase();
          ['#111827', '#ef4444', '#2563eb', '#10b981', '#f59e0b'].forEach(function (hex) {
            var b = document.createElement('button');
            b.type = 'button';
            b.className = 'rcb-dot' + (curColor === hex ? ' on' : '');
            b.style.backgroundColor = hex;
            b.addEventListener('click', function () { setInputVal(inkColor, hex); renderPill(); });
            pill.appendChild(b);
          });
          var sepInk = document.createElement('span');
          sepInk.className = 'rcb-sep';
          pill.appendChild(sepInk);
          var curW = Number(inkThick && inkThick.value || 1);
          [2, 5, 10].forEach(function (w) {
            var b = document.createElement('button');
            b.type = 'button';
            b.className = 'rcb-btn' + (Math.round(curW) === w ? ' on' : '');
            b.textContent = String(w);
            b.title = '두께 ' + w;
            b.addEventListener('click', function () { setInputVal(inkThick, w); renderPill(); });
            pill.appendChild(b);
          });
        }

        var sep = document.createElement('span');
        sep.className = 'rcb-sep';
        pill.appendChild(sep);

        var gear = document.createElement('button');
        gear.type = 'button';
        gear.className = 'rcb-btn';
        gear.textContent = '⚙';
        gear.title = '상세 설정 열기/닫기';
        gear.addEventListener('click', function () {
          if (!act.bar) return;
          var willShow = act.bar.classList.contains('hidden');
          act.bar.classList.toggle('hidden', !willShow);
          act.btn.setAttribute('aria-expanded', willShow ? 'true' : 'false');
        });
        pill.appendChild(gear);

        var close = document.createElement('button');
        close.type = 'button';
        close.className = 'rcb-btn';
        close.textContent = '✕';
        close.title = '도구 끄기 (Esc)';
        close.addEventListener('click', function () { act.btn.click(); });
        pill.appendChild(close);
      }

      tools.forEach(function (t) {
        var wasOn = t.btn.classList.contains('toggled');
        new MutationObserver(function () {
          var isOn = t.btn.classList.contains('toggled');
          if (isOn !== wasOn) {
            wasOn = isOn;
            if (isOn) requestAnimationFrame(function () { hideBar(t); renderPill(); });
            else renderPill();
          }
        }).observe(t.btn, { attributes: true, attributeFilter: ['class'] });
      });

      if (viewerContainer) {
        viewerContainer.addEventListener('pointerdown', hideAllBars, true);
      }
      if (picker) {
        picker.addEventListener('click', function () {
          requestAnimationFrame(function () { hideBar(tools[0]); renderPill(); });
        });
      }

      document.addEventListener('keydown', function (e) {
        var tgt = e.target;
        var editing = tgt && (tgt.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(tgt.tagName));
        if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === 's' || e.key === 'S')) {
          var dl = document.getElementById('downloadButton');
          if (dl) { e.preventDefault(); dl.click(); }
          return;
        }
        if (editing || e.ctrlKey || e.metaKey || e.altKey) return;
        if (e.key === 'h' || e.key === 'H') { e.preventDefault(); hlBtn.click(); }
        else if (e.key === 'd' || e.key === 'D' || e.key === 'p' || e.key === 'P') { e.preventDefault(); inkBtn.click(); }
        else if (e.key === 't' || e.key === 'T') { e.preventDefault(); txtBtn.click(); }
        else if (/^[1-5]$/.test(e.key) && hlBtn.classList.contains('toggled') && picker) {
          var sw = picker.querySelectorAll('button')[Number(e.key) - 1];
          if (sw) { e.preventDefault(); sw.click(); requestAnimationFrame(function () { hideBar(tools[0]); renderPill(); }); }
        } else if (e.key === 'Escape') {
          var openBar = tools.some(function (t) { return t.bar && !t.bar.classList.contains('hidden'); });
          if (openBar) { hideAllBars(); return; }
          var act = activeTool();
          if (act) { act.btn.click(); }
        }
      });

      renderPill();
    } catch (e) {}
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initAnnotatorUx, { once: true });
  } else {
    initAnnotatorUx();
  }
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_urls() {
        assert_eq!(file_name("https://x.test/a/My%20Report.PDF?x=1"), "My Report.PDF");
        assert_eq!(file_name("https://x.test/download"), "download.pdf");
        assert_eq!(file_name("https://x.test/"), "document.pdf");
        assert_eq!(file_name("file:///C:/docs/%ED%95%9C%EA%B8%80.pdf"), "한글.pdf");
        assert!(is_pdf_url("https://e.test/A.PDF?dl=1#p=2") && !is_pdf_url("https://e.test/pdf.html"));
        let s = PdfSources::default();
        let u = s.editor_url("file:///C:/a%20b.pdf");
        assert!(u.starts_with(&format!("{ORIGIN}/web/viewer.html?file=%2Fpdf%2F")));
        let token = u.split("%2Fpdf%2F").nth(1).and_then(|t| t.split("%2F").next()).unwrap();
        assert_eq!(s.get(token).as_deref(), Some("file:///C:/a%20b.pdf"));
        assert_eq!(s.get("nope"), None);
        // Same source reuses its token; the table stays bounded.
        assert_eq!(s.editor_url("file:///C:/a%20b.pdf"), u);
        for i in 0..MAX_SOURCES + 5 {
            s.editor_url(&format!("file:///C:/{i}.pdf"));
        }
        assert!(s.0.lock().unwrap().len() <= MAX_SOURCES && s.get(token).is_none());

        let mut buf = Vec::new();
        append_bounded(&mut buf, b"1234", 6).unwrap();
        append_bounded(&mut buf, b"56", 6).unwrap();
        assert!(append_bounded(&mut buf, b"7", 6).is_err());
        assert_eq!(buf, b"123456");
    }

    #[test]
    fn assets_are_embedded_and_inflate() {
        let (_, z) = FILES
            .iter()
            .find(|(n, _)| *n == "web/viewer.html")
            .expect("viewer.html embedded");
        let html = miniz_oxide::inflate::decompress_to_vec(z).unwrap();
        assert!(String::from_utf8_lossy(&html).contains("viewer.mjs"));
        assert!(FILES.iter().any(|(n, _)| *n == "build/pdf.worker.mjs"));
        assert!(FILES.iter().any(|(n, _)| *n == "web/locale/ko/viewer.ftl"));
        assert!(!FILES.iter().any(|(n, _)| n.contains("quickjs-eval")));
        let (_, pdf_mjs_z) = FILES
            .iter()
            .find(|(n, _)| *n == "build/pdf.mjs")
            .expect("build/pdf.mjs embedded");
        let pdf_mjs = miniz_oxide::inflate::decompress_to_vec(pdf_mjs_z).unwrap();
        assert!(
            String::from_utf8_lossy(&pdf_mjs).contains("refH * aspect * 2.2"),
            "patched line-merging getSelectionBoxes must be embedded"
        );
        assert!(INIT_SCRIPT.contains("enableHighlightFloatingButton"));
        assert!(INIT_SCRIPT.contains("rcb-pdf-pill"));
        let resp = reply(200, "text/plain", "no-store", b"ok".to_vec());
        assert_eq!(resp.headers().get("X-Frame-Options").unwrap(), "DENY");
        assert_eq!(
            resp.headers().get("Content-Security-Policy").unwrap(),
            "frame-ancestors 'none'"
        );
    }

    fn minimal_test_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 10 100 Td ({text}) Tj ET");
        let objs = [
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n".to_string(),
            format!(
                "4 0 obj\n<< /Length {} >>\nstream\n{stream}\nendstream\nendobj\n",
                stream.len()
            ),
            "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n".to_string(),
        ];
        let mut pdf = Vec::from(&b"%PDF-1.4\n"[..]);
        let mut offsets = Vec::with_capacity(objs.len());
        for o in &objs {
            offsets.push(pdf.len());
            pdf.extend_from_slice(o.as_bytes());
        }
        let xref_pos = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn extracts_text_from_minimal_pdf_and_handles_invalid_bytes() {
        let pdf = minimal_test_pdf("Hello PDF World");
        let extracted = extract_pdf_text(&pdf, 1000).expect("valid PDF extracts text");
        assert!(extracted.contains("Hello PDF World"), "got: {extracted:?}");
        assert!(extract_pdf_text(b"not a pdf", 1000).is_err());
        assert_eq!(extract_pdf_text(b"", 1000).unwrap(), "");
    }
}
