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
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wry::RequestAsyncResponder;
use wry::http::{Request, Response};

include!(concat!(env!("OUT_DIR"), "/pdfjs_assets.rs")); // FILES: &[(&str, &[u8])] (deflated)

const ORIGIN: &str = "http://rcbpdf.localhost";
/// `blob:` downloads created by the annotator's Save button.
pub const BLOB_PREFIX: &str = "blob:http://rcbpdf.localhost/";
const MAX_PDF_BYTES: u64 = 256 * 1024 * 1024;

/// `true` when the URL path names a PDF (`.pdf`, any case; query/fragment ignored).
pub fn is_pdf_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.path().to_ascii_lowercase().ends_with(".pdf"))
}

/// Maps random tokens to the PDF source URLs the app chose to open in the annotator.
#[derive(Default)]
pub struct PdfSources(Mutex<HashMap<String, String>>);

impl PdfSources {
    /// Viewer URL that shows `src` (a `file://` or `http(s)://` PDF URL).
    pub fn editor_url(&self, src: &str) -> String {
        let token = crate::single::random_token();
        // ponytail: one short string per toggle kept for the session; prune if it ever matters.
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(token.clone(), src.to_string());
        let path = format!("/pdf/{token}/{}", enc(&file_name(src)));
        format!("{ORIGIN}/web/viewer.html?file={}", enc(&path))
    }

    fn get(&self, token: &str) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(token).cloned()
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
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
    let seg: String = seg.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    if seg.to_ascii_lowercase().ends_with(".pdf") { seg } else { format!("{seg}.pdf") }
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

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

async fn load(src: &str, cookies: Arc<PersistentCookieJar>) -> Result<Vec<u8>, String> {
    let url = url::Url::parse(src).map_err(|e| e.to_string())?;
    match url.scheme() {
        "file" => {
            let path = url.to_file_path().map_err(|()| "잘못된 파일 경로".to_string())?;
            tokio::task::spawn_blocking(move || {
                let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                if len > MAX_PDF_BYTES {
                    return Err("PDF가 너무 큼 (256MB 초과)".to_string());
                }
                std::fs::read(&path).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "http" | "https" => {
            let client = reqwest::Client::builder()
                .cookie_provider(cookies)
                .timeout(std::time::Duration::from_secs(60))
                .user_agent("Mozilla/5.0")
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
            if resp.content_length().is_some_and(|n| n > MAX_PDF_BYTES) {
                return Err("PDF가 너무 큼 (256MB 초과)".to_string());
            }
            resp.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
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
    let path = req.uri().path().to_string();
    if let Some(rest) = path.strip_prefix("/pdf/") {
        let token = rest.split('/').next().unwrap_or("");
        let Some(src) = sources.get(token) else {
            return responder.respond(reply(404, "text/plain", b"unknown PDF token".to_vec()));
        };
        let name = file_name(&src);
        let cookies = Arc::clone(cookies);
        rt.spawn(async move {
            responder.respond(match load(&src, cookies).await {
                Ok(bytes) => Response::builder()
                    .status(200)
                    .header("Content-Type", "application/pdf")
                    .header("Content-Disposition", format!("inline; filename*=UTF-8''{}", enc(&name)))
                    .header("Accept-Ranges", "none")
                    .header("Cache-Control", "no-store")
                    .body(Cow::Owned(bytes))
                    .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..]))),
                Err(e) => reply(502, "text/plain; charset=utf-8", e.into_bytes()),
            });
        });
        return;
    }
    let key = path.trim_start_matches('/');
    let found = FILES.iter().find(|(n, _)| *n == key).and_then(|(_, z)| miniz_oxide::inflate::decompress_to_vec(z).ok());
    responder.respond(match found {
        Some(body) => reply(200, mime(key), body),
        None => reply(404, "text/plain", b"not found".to_vec()),
    });
}

/// Runs in every WebView page; only acts on the annotator origin. Turns PDF JavaScript off
/// (a PDF is untrusted input) and opens documents fit-to-width.
pub const INIT_SCRIPT: &str = r#"(function () {
  if (location.hostname !== 'rcbpdf.localhost') return;
  try {
    var k = 'pdfjs.preferences', p = JSON.parse(localStorage.getItem(k) || '{}');
    var want = { enableScripting: false, defaultZoomValue: 'page-width' }, changed = false;
    for (var a in want) if (p[a] !== want[a]) { p[a] = want[a]; changed = true; }
    if (changed) localStorage.setItem(k, JSON.stringify(p));
  } catch (e) {}
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
        assert!(u.starts_with("http://rcbpdf.localhost/web/viewer.html?file=%2Fpdf%2F"));
        let token = u.split("%2Fpdf%2F").nth(1).and_then(|t| t.split("%2F").next()).unwrap();
        assert_eq!(s.get(token).as_deref(), Some("file:///C:/a%20b.pdf"));
        assert_eq!(s.get("nope"), None);
    }

    #[test]
    fn assets_are_embedded_and_inflate() {
        let (_, z) = FILES.iter().find(|(n, _)| *n == "web/viewer.html").expect("viewer.html embedded");
        let html = miniz_oxide::inflate::decompress_to_vec(z).unwrap();
        assert!(String::from_utf8_lossy(&html).contains("viewer.mjs"));
        assert!(FILES.iter().any(|(n, _)| *n == "build/pdf.worker.mjs"));
        assert!(FILES.iter().any(|(n, _)| *n == "web/locale/ko/viewer.ftl"));
    }
}
