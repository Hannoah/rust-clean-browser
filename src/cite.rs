//! "Copy citation" for the current page: IEEE, APA 7, presentation-slide, MLA 9.
//! Author / site name / publish date come from `<meta>` tags when the page HTML is
//! available (Clean mode); otherwise only title + host + access date are used.

use crate::cleaner::extract_title;

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
    "November", "December",
];

#[derive(Default)]
struct Info {
    title: String,
    url: String,
    site: String,
    author: Option<String>,
    /// Publish date (year, month, day), each optional-precision via 0.
    published: Option<(i32, u32, u32)>,
    accessed: (i32, u32, u32),
}

/// Build the citation text for `style` (`ieee` | `apa` | `slide` | `mla`).
pub fn citation(style: &str, title: &str, url: &str, html: Option<&str>) -> Option<String> {
    let mut info = Info { title: title.trim().to_string(), url: url.to_string(), accessed: today(), ..Default::default() };
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?.split(['/', '?', '#']).next()?;
    info.site = host.strip_prefix("www.").unwrap_or(host).to_string();
    if let Some(html) = html {
        if info.title.is_empty() {
            info.title = extract_title(html).unwrap_or_default();
        }
        info.site = meta(html, &["og:site_name", "application-name"]).unwrap_or(info.site);
        info.author = meta(html, &["author", "article:author", "og:article:author"]).filter(|a| !a.starts_with("http"));
        info.published = meta(html, &["article:published_time", "og:article:published_time", "date", "pubdate", "dc.date"])
            .and_then(|d| parse_date(&d));
    }
    if info.title.is_empty() {
        info.title = info.site.clone();
    }
    Some(match style {
        "ieee" => info.ieee(),
        "apa" => info.apa(),
        "slide" => info.slide(),
        "mla" => info.mla(),
        _ => return None,
    })
}

impl Info {
    fn ieee(&self) -> String {
        let a = self.author.as_ref().map_or(String::new(), |a| format!("{a}, "));
        let y = self.published.map_or(String::new(), |(y, ..)| format!(", {y}"));
        let (ay, am, ad) = self.accessed;
        format!(
            "{a}\"{}\", {}{y}. [Online]. Available: {} (accessed {} {ad}, {ay}).",
            self.title, self.site, self.url, &MONTHS[am as usize - 1][..3],
        )
    }

    fn apa(&self) -> String {
        let date = self.published.map_or("n.d.".to_string(), |(y, m, d)| match (m, d) {
            (0, _) => y.to_string(),
            (m, 0) => format!("{y}, {}", MONTHS[m as usize - 1]),
            (m, d) => format!("{y}, {} {d}", MONTHS[m as usize - 1]),
        });
        let site = if self.author.as_deref() == Some(&self.site) { String::new() } else { format!(" {}.", self.site) };
        match &self.author {
            Some(a) => format!("{a}. ({date}). {}.{site} {}", self.title, self.url),
            None => format!("{}. ({date}).{site} {}", self.title, self.url),
        }
    }

    fn slide(&self) -> String {
        let (y, m, d) = self.accessed;
        let who = self.author.as_ref().map_or(self.site.clone(), |a| format!("{a}, {}", self.site));
        format!("출처: {who}, \"{}\", {} ({y}.{m:02}.{d:02} 접속)", self.title, self.url)
    }

    fn mla(&self) -> String {
        let a = self.author.as_ref().map_or(String::new(), |a| format!("{a}. "));
        let p = self.published.map_or(String::new(), |(y, m, d)| {
            let mon = if m > 0 { format!("{} ", &MONTHS[m as usize - 1][..3]) } else { String::new() };
            let day = if d > 0 { format!("{d} ") } else { String::new() };
            format!(", {day}{mon}{y}")
        });
        let (y, m, d) = self.accessed;
        format!("{a}\"{}.\" {}{p}, {}. Accessed {d} {} {y}.", self.title, self.site, self.url, &MONTHS[m as usize - 1][..3])
    }
}

/// First `<meta name|property=KEY content=…>` hit, trying `keys` in order.
fn meta(html: &str, keys: &[&str]) -> Option<String> {
    let lower = html.to_ascii_lowercase(); // ASCII lowercase keeps byte offsets aligned with `html`
    let tags: Vec<(&str, &str)> = lower
        .match_indices("<meta")
        .filter_map(|(i, _)| {
            let end = i + lower[i..].find('>')?;
            Some((&lower[i..end], &html[i..end]))
        })
        .collect();
    keys.iter().find_map(|key| {
        tags.iter().find_map(|(lo, orig)| {
            let id = attr(lo, lo, "name").or_else(|| attr(lo, lo, "property")).or_else(|| attr(lo, lo, "itemprop"))?;
            (id == *key).then(|| attr(lo, orig, "content"))?.map(|v| unescape(v.trim())).filter(|v| !v.is_empty())
        })
    })
}

/// Value of ` key=…` in a tag; `lo` is the lowercased tag (for matching), `src` supplies the value.
fn attr(lo: &str, src: &str, key: &str) -> Option<String> {
    let at = lo.match_indices(key).find(|(i, _)| {
        lo.as_bytes()[i.saturating_sub(1)].is_ascii_whitespace() && lo[i + key.len()..].trim_start().starts_with('=')
    })?.0;
    let rest = src[at + key.len()..].trim_start().strip_prefix('=')?.trim_start();
    Some(match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?.to_string(),
        _ => rest.split(|c: char| c.is_whitespace() || c == '/').next()?.to_string(),
    })
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// `2024-03-05…` / `2024-03` / `2024` → (y, m, d) with 0 for unknown parts.
fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let mut p = s.trim().split(|c: char| !c.is_ascii_digit());
    let y: i32 = p.next()?.parse().ok().filter(|y| (1000..3000).contains(y))?;
    let m = p.next().and_then(|m| m.parse().ok()).filter(|m| (1..=12).contains(m)).unwrap_or(0);
    let d = if m > 0 { p.next().and_then(|d| d.parse().ok()).filter(|d| (1..=31).contains(d)).unwrap_or(0) } else { 0 };
    Some((y, m, d))
}

/// Today's date in the user's local time zone.
#[cfg(target_os = "windows")]
fn today() -> (i32, u32, u32) {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(out: *mut [u16; 8]);
    }
    let mut st = [0u16; 8]; // SYSTEMTIME: year, month, dayOfWeek, day, ...
    unsafe { GetLocalTime(&mut st) };
    (st[0] as i32, st[1] as u32, st[3] as u32)
}

#[cfg(target_os = "macos")]
fn today() -> (i32, u32, u32) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&libc::time(std::ptr::null_mut()), &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
}

// ponytail: UTC on other platforms (the browser ships for Windows/macOS only).
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn today() -> (i32, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let z = (secs / 86400) as i64 + 719_468; // Hinnant's civil-from-days
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((yoe + era * 400) as i32 + (m <= 2) as i32, m, (doy - (153 * mp + 2) / 5 + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<html><head><title>T</title>
        <meta property="og:site_name" content="Example &amp; Co">
        <meta name="author" content="Jane Doe">
        <meta property='article:published_time' content='2024-03-05T10:00:00Z'></head>"#;

    #[test]
    fn formats_with_metadata() {
        let c = |s| citation(s, "My Title", "https://www.example.com/a?b=1", Some(HTML)).unwrap();
        assert!(c("apa").starts_with("Jane Doe. (2024, March 5). My Title. Example & Co. https://www.example.com/a?b=1"));
        assert!(c("ieee").starts_with("Jane Doe, \"My Title\", Example & Co, 2024. [Online]. Available: https://"));
        assert!(c("mla").starts_with("Jane Doe. \"My Title.\" Example & Co, 5 Mar 2024, https://"));
        assert!(c("slide").starts_with("출처: Jane Doe, Example & Co, \"My Title\", https://"));
    }

    #[test]
    fn falls_back_to_host_without_html() {
        let apa = citation("apa", "My Title", "https://www.example.com/a", None).unwrap();
        assert_eq!(apa, "My Title. (n.d.). example.com. https://www.example.com/a");
        assert!(citation("nope", "t", "https://a.b", None).is_none());
        assert!(citation("apa", "t", "rust://home", None).is_none());
    }

    #[test]
    fn partial_dates() {
        assert_eq!(parse_date("2024"), Some((2024, 0, 0)));
        assert_eq!(parse_date("2024-03"), Some((2024, 3, 0)));
        assert_eq!(parse_date("garbage"), None);
    }
}
