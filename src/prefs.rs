//! Persistent user interface preferences (`<app data>/prefs.json`):
//! theme mode override, bookmark bar visibility, and Reader Mode typography.

use crate::bookmarks::app_data_dir;
use crate::cleaner::ReaderOptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "시스템",
            Self::Light => "라이트",
            Self::Dark => "다크",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub theme: ThemePreference,
    pub show_bookmarks_bar: bool,
    pub reader: ReaderOptions,
    /// Upgrade `http://` navigations to `https://` automatically.
    pub https_only: bool,
    /// Width (logical px) of the docked AI assistant sidebar.
    pub sidebar_width: u32,
    /// Hosts remembered to always load in JS mode (WebView2).
    pub js_hosts: BTreeSet<String>,
    /// AI assistant message font size (px).
    pub chat_font_size: u32,
}

pub const DEFAULT_CHAT_FONT: u32 = 13;

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            show_bookmarks_bar: true,
            reader: ReaderOptions::default(),
            https_only: false,
            sidebar_width: 380,
            js_hosts: BTreeSet::new(),
            chat_font_size: DEFAULT_CHAT_FONT,
        }
    }
}

impl UiPrefs {
    fn host_key(url: &str) -> Option<String> {
        let parsed = url::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        parsed.host_str().map(|h| h.to_ascii_lowercase())
    }

    pub fn host_prefers_js(&self, url: &str) -> bool {
        Self::host_key(url).is_some_and(|h| self.js_hosts.contains(&h))
    }

    pub fn remember_js_host(&mut self, url: &str) {
        if let Some(host) = Self::host_key(url)
            && self.js_hosts.len() < 200
            && self.js_hosts.insert(host)
        {
            self.save();
        }
    }

    pub fn forget_js_host(&mut self, url: &str) {
        if let Some(host) = Self::host_key(url)
            && self.js_hosts.remove(&host)
        {
            self.save();
        }
    }

    fn path() -> Option<PathBuf> {
        Some(app_data_dir()?.join("prefs.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = crate::net::atomic_write_private(&path, json.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_hosts_remember_forget_and_ignore_non_http() {
        let mut prefs = UiPrefs::default();
        assert!(!prefs.host_prefers_js("https://Example.COM/a"));
        assert_eq!(
            UiPrefs::host_key("https://Example.COM/a").as_deref(),
            Some("example.com")
        );
        assert_eq!(UiPrefs::host_key("file:///C:/a.html"), None);
        assert_eq!(UiPrefs::host_key("rust://home"), None);
        prefs.js_hosts.insert("example.com".into());
        assert!(prefs.host_prefers_js("https://EXAMPLE.COM/b"));
    }
}
