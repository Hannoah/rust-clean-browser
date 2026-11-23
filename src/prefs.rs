//! Persistent user interface preferences (`<app data>/prefs.json`):
//! theme mode override, bookmark bar visibility, and Reader Mode typography.

use crate::bookmarks::app_data_dir;
use crate::cleaner::ReaderOptions;
use serde::{Deserialize, Serialize};
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
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            show_bookmarks_bar: true,
            reader: ReaderOptions::default(),
            https_only: false,
        }
    }
}

impl UiPrefs {
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
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}
