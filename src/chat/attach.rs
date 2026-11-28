//! Files attached to a chat message: images go to the model as PNG, text/PDF/code files are
//! inlined into the message text as `<attached_file>` blocks (so later turns still see them).

use std::path::Path;

/// Pending in the composer until sent.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub kind: AttachKind,
}

#[derive(Debug, Clone)]
pub enum AttachKind {
    /// PNG, base64.
    Image(String),
    Text(String),
}

pub const MAX_IMAGES: usize = 4;
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 60_000;
/// Longest image edge sent to a model (bigger ones are downscaled).
const MAX_EDGE: u32 = 1568;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn load_file(path: &Path) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("{name}: 20MB보다 큰 파일은 첨부할 수 없습니다"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    load_bytes(&name, bytes)
}

/// A file's contents by name (a JS-mode page hands over dropped files as bytes, not paths).
pub fn load_bytes(name: &str, bytes: Vec<u8>) -> Result<Attachment, String> {
    let name = clean_name(name);
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{name}: 20MB보다 큰 파일은 첨부할 수 없습니다"));
    }
    let ext = Path::new(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let kind = if IMAGE_EXTS.contains(&ext.as_str()) {
        let img = image::load_from_memory(&bytes).map_err(|e| format!("{name}: 이미지를 읽을 수 없습니다 ({e})"))?;
        AttachKind::Image(to_png_b64(img)?)
    } else if ext == "pdf" {
        let text = crate::pdfjs::extract_pdf_text(&bytes, MAX_TEXT_CHARS).map_err(|e| format!("{name}: {e}"))?;
        if text.trim().is_empty() {
            return Err(format!("{name}: 글자가 없는 PDF입니다(스캔본은 이미지로 첨부하세요)"));
        }
        AttachKind::Text(text)
    } else {
        // Anything that decodes as text (code, csv, json, md, logs…); binaries are refused.
        let text =
            String::from_utf8(bytes).map_err(|_| format!("{name}: 지원하지 않는 형식입니다(텍스트·PDF·이미지만)"))?;
        if text.contains('\0') {
            return Err(format!("{name}: 지원하지 않는 형식입니다(텍스트·PDF·이미지만)"));
        }
        AttachKind::Text(cap(&text))
    };
    Ok(Attachment { name, kind })
}

/// A pasted clipboard image (RGBA).
pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Attachment, String> {
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("클립보드 이미지 형식 오류")?;
    Ok(Attachment {
        name: "붙여넣은 이미지.png".into(),
        kind: AttachKind::Image(to_png_b64(image::DynamicImage::ImageRgba8(img))?),
    })
}

fn to_png_b64(img: image::DynamicImage) -> Result<String, String> {
    let img = if img.width().max(img.height()) > MAX_EDGE {
        img.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("PNG 변환 실패: {e}"))?;
    Ok(crate::screenshot::base64(&png))
}

fn cap(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((i, _)) => format!("{}\n…(이후 생략)", &text[..i]),
        None => text.to_string(),
    }
}

/// Names end up inside `name="…"` markers: keep them to one plain line.
fn clean_name(name: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| !matches!(c, '"' | '<' | '>' | '\r' | '\n'))
        .take(80)
        .collect();
    if n.trim().is_empty() { "파일".into() } else { n }
}

const MARK: &str = "\n\n<attached_";

/// The message text sent and stored: the typed text plus one block per attachment.
pub fn embed(text: &str, atts: &[Attachment]) -> String {
    let mut out = text.to_string();
    for a in atts {
        match &a.kind {
            AttachKind::Image(_) => out.push_str(&format!("{MARK}image name=\"{}\"/>", a.name)),
            AttachKind::Text(t) => out.push_str(&format!("{MARK}file name=\"{}\">\n{t}\n</attached_file>", a.name)),
        }
    }
    out
}

/// For the bubble: the typed text and the attached file names.
pub fn split_display(text: &str) -> (&str, Vec<&str>) {
    let Some(at) = text.find(MARK) else {
        return (text, Vec::new());
    };
    let names = text[at..]
        .match_indices(MARK)
        .filter_map(|(i, _)| {
            let rest = &text[at + i + MARK.len()..];
            let rest = rest
                .strip_prefix("file name=\"")
                .or_else(|| rest.strip_prefix("image name=\""))?;
            Some(&rest[..rest.find('"')?])
        })
        .collect();
    (&text[..at], names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_round_trips_for_display() {
        let atts = vec![
            Attachment {
                name: clean_name("a\"<b>.txt"),
                kind: AttachKind::Text("hello\n\n<attached_x".into()),
            },
            Attachment {
                name: "pic.png".into(),
                kind: AttachKind::Image("AAAA".into()),
            },
        ];
        let msg = embed("요약해줘", &atts);
        assert!(msg.contains("<attached_file name=\"ab.txt\">\nhello"));
        let (body, names) = split_display(&msg);
        assert_eq!(body, "요약해줘");
        assert_eq!(names, vec!["ab.txt", "pic.png"]);
        assert_eq!(split_display("plain"), ("plain", vec![]));
    }

    #[test]
    fn pasted_rgba_becomes_downscaled_png() {
        let att = from_rgba(2000, 10, vec![255; 2000 * 10 * 4]).unwrap();
        let AttachKind::Image(b64) = att.kind else { panic!() };
        let png = crate::chat::decode_base64(&b64).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), MAX_EDGE);
    }
}
