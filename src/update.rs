//! Automatic update check against GitHub Releases (`api.github.com`):
//! once a day the app asks for the latest release, compares it with the
//! built-in `CARGO_PKG_VERSION`, and offers a one-click installer download.
//! Only official `github.com` / `*.githubusercontent.com` asset URLs are ever
//! fetched, and the installer itself (Inno Setup, `CloseApplications=yes`)
//! performs the actual replace.

use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

pub const RELEASES_API: &str = "https://api.github.com/repos/Hannoah/rust-clean-browser/releases/latest";
/// Minimum gap between two automatic checks (manual checks always run).
pub const CHECK_INTERVAL_SECS: u64 = 24 * 3600;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SETUP_BYTES: u64 = 300 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Release tag as published (e.g. `"v0.5.5"`), shown in the toolbar pill.
    pub version: String,
    pub setup_url: String,
    pub setup_name: String,
}

/// `true` when enough time passed since `last_check_secs` (Unix seconds).
pub fn should_check(last_check_secs: u64, now_secs: u64) -> bool {
    now_secs.saturating_sub(last_check_secs) >= CHECK_INTERVAL_SECS
}

/// Parses `"v1.2.3"` / `"1.2"` / `"1"` into a comparable triple.
/// Missing trailing parts default to 0; present-but-invalid parts reject the tag.
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let t = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    let mut parts = t.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = match parts.next() {
        None => 0,
        Some(p) => p.parse().ok()?,
    };
    let patch: u64 = match parts.next() {
        None => 0,
        Some(p) => p.parse().ok()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// `true` when `latest_tag` is strictly newer than `current`.
pub fn is_newer(current: &str, latest_tag: &str) -> bool {
    match (parse_version(current), parse_version(latest_tag)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    }
}

/// Picks the Windows Setup asset out of a `releases/latest` JSON document.
pub fn find_setup_asset(doc: &Value) -> Option<(String, String)> {
    let assets = doc.get("assets")?.as_array()?;
    assets.iter().find_map(|a| {
        let name = a.get("name")?.as_str()?;
        let url = a.get("browser_download_url")?.as_str()?;
        if name.starts_with("RustCleanBrowser-Setup-") && name.ends_with("-x64.exe") {
            Some((name.to_string(), url.to_string()))
        } else {
            None
        }
    })
}

/// Only official GitHub release asset hosts are trusted for downloads.
pub fn is_official_asset_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    match parsed.host_str() {
        Some("github.com") => true,
        Some(h) => h.ends_with(".githubusercontent.com"),
        None => false,
    }
}

/// Fetches the latest release and returns it when it carries a newer Setup asset.
pub async fn fetch_latest(current_version: &str) -> Result<Option<UpdateInfo>, String> {
    let client = reqwest::Client::builder()
        .user_agent("rust-clean-browser-updater")
        .connect_timeout(Duration::from_secs(10))
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("업데이트 확인 실패: {e}"))?;
    let body = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("업데이트 확인 실패: {e}"))?
        .error_for_status()
        .map_err(|e| format!("업데이트 확인 실패: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("업데이트 확인 실패: {e}"))?;
    let doc: Value = serde_json::from_slice(&body).map_err(|e| format!("업데이트 확인 실패: {e}"))?;
    let tag = doc.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    if !is_newer(current_version, tag) {
        return Ok(None);
    }
    let Some((setup_name, setup_url)) = find_setup_asset(&doc) else {
        return Ok(None);
    };
    if !is_official_asset_url(&setup_url) {
        return Err("업데이트 주소가 공식 릴리스가 아닙니다.".to_string());
    }
    Ok(Some(UpdateInfo {
        version: tag.to_string(),
        setup_url,
        setup_name,
    }))
}

fn downloads_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// Downloads the Setup exe, capped at [`MAX_SETUP_BYTES`].
pub async fn download_setup(info: &UpdateInfo) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .user_agent("rust-clean-browser-updater")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("업데이트 다운로드 실패: {e}"))?;
    let resp = client
        .get(&info.setup_url)
        .send()
        .await
        .map_err(|e| format!("업데이트 다운로드 실패: {e}"))?
        .error_for_status()
        .map_err(|e| format!("업데이트 다운로드 실패: {e}"))?;
    if resp.content_length().is_some_and(|n| n > MAX_SETUP_BYTES) {
        return Err("업데이트 파일이 너무 큽니다.".to_string());
    }
    let bytes = resp.bytes().await.map_err(|e| format!("업데이트 다운로드 실패: {e}"))?;
    if bytes.len() as u64 > MAX_SETUP_BYTES {
        return Err("업데이트 파일이 너무 큽니다.".to_string());
    }
    let dest = downloads_dir().join(&info.setup_name);
    tokio::task::spawn_blocking(move || std::fs::write(&dest, &bytes).map(|_| dest))
        .await
        .map_err(|e| format!("업데이트 저장 실패: {e}"))?
        .map_err(|e| format!("업데이트 저장 실패: {e}"))
}

/// Seconds since the Unix epoch (UTC); used to throttle automatic checks.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hands the downloaded installer to the OS; Inno Setup closes the browser itself.
pub fn launch_installer(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("설치 프로그램 실행 실패: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tags_compare_numerically() {
        assert_eq!(parse_version("v0.5.4"), Some((0, 5, 4)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("3"), Some((3, 0, 0)));
        assert_eq!(parse_version("V2.0.1"), Some((2, 0, 1)));
        assert!(parse_version("v").is_none());
        assert!(parse_version("nightly").is_none());
        assert!(parse_version("1.2.3.4").is_none());
        assert!(parse_version("1.x").is_none());

        assert!(is_newer("0.5.4", "v0.5.5"));
        assert!(is_newer("0.5.4", "v0.6.0"));
        assert!(is_newer("0.5.4", "v1.0.0"));
        assert!(!is_newer("0.5.5", "v0.5.5"));
        assert!(!is_newer("0.5.10", "v0.5.9"));
        assert!(!is_newer("0.5.4", "latest"));
        assert!(!is_newer("0.5.4", "v0.5.4-rc1"));
    }

    #[test]
    fn picks_only_the_windows_setup_asset() {
        let doc: Value = serde_json::json!({
            "tag_name": "v0.5.5",
            "assets": [
                {"name": "notes.txt", "browser_download_url": "https://example.com/n"},
                {"name": "RustCleanBrowser-0.5.5-windows-x64-portable.zip", "browser_download_url": "https://example.com/p"},
                {"name": "RustCleanBrowser-Setup-0.5.5-x64.exe", "browser_download_url": "https://github.com/Hannoah/rust-clean-browser/releases/download/v0.5.5/RustCleanBrowser-Setup-0.5.5-x64.exe"},
                {"name": "RustCleanBrowser-0.5.5-macos-arm64.dmg", "browser_download_url": "https://example.com/d"}
            ]
        });
        assert_eq!(
            find_setup_asset(&doc),
            Some((
                "RustCleanBrowser-Setup-0.5.5-x64.exe".to_string(),
                "https://github.com/Hannoah/rust-clean-browser/releases/download/v0.5.5/RustCleanBrowser-Setup-0.5.5-x64.exe".to_string()
            ))
        );
        let empty: Value = serde_json::json!({"tag_name": "v0.5.5", "assets": []});
        assert_eq!(find_setup_asset(&empty), None);
    }

    #[test]
    fn only_official_hosts_are_trusted() {
        assert!(is_official_asset_url(
            "https://github.com/Hannoah/rust-clean-browser/releases/download/v0.5.5/a.exe"
        ));
        assert!(is_official_asset_url("https://objects.githubusercontent.com/x/y.exe"));
        assert!(!is_official_asset_url("https://example.com/a.exe"));
        assert!(!is_official_asset_url("http://github.com/a.exe"));
        assert!(!is_official_asset_url("not a url"));
    }

    #[test]
    fn daily_throttle_math() {
        assert!(should_check(0, CHECK_INTERVAL_SECS));
        assert!(should_check(100, 100 + CHECK_INTERVAL_SECS));
        assert!(!should_check(100, 100 + CHECK_INTERVAL_SECS - 1));
        assert!(!should_check(200, 100));
    }
}
