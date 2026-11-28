use crate::bookmarks::app_data_dir;
use crate::net::seal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{AGY_FAST_CONTROL_MODEL, ChatProvider, CliAvailability, is_valid_effort, is_valid_model_id};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPreset {
    pub title: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    pub provider: ChatProvider,
    pub include_page: bool,
    pub agent_control: bool,
    pub api_endpoint: String,
    pub api_model: String,
    pub api_key: String,
    pub api_keys: BTreeMap<String, String>,
    pub api_endpoints: BTreeMap<String, String>,
    pub provider_models: BTreeMap<String, String>,
    pub control_models: BTreeMap<String, String>,
    /// Per-provider `--effort` override (`""` = auto). Only the Gemini (`agy`) and
    /// Claude CLI agents read it; other providers ignore it.
    #[serde(default)]
    pub effort_levels: BTreeMap<String, String>,
    pub custom_presets: Vec<AgentPreset>,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            provider: ChatProvider::Agy,
            include_page: true,
            agent_control: false,
            api_endpoint: String::new(),
            api_model: String::new(),
            api_key: String::new(),
            api_keys: BTreeMap::new(),
            api_endpoints: BTreeMap::new(),
            provider_models: BTreeMap::new(),
            control_models: BTreeMap::new(),
            effort_levels: BTreeMap::new(),
            custom_presets: Vec::new(),
        }
    }
}

impl ChatConfig {
    fn path() -> Option<PathBuf> {
        Some(app_data_dir()?.join("chat.json"))
    }

    pub fn load_with_avail(avail: CliAvailability) -> Self {
        match Self::path().and_then(|p| Self::load_from_path(&p)) {
            Some(mut cfg) => {
                cfg.migrate_legacy_api_fields();
                cfg
            }
            None => Self {
                provider: avail.preferred_default(),
                ..Self::default()
            },
        }
    }

    fn migrate_legacy_api_fields(&mut self) {
        // The legacy `gemini` CLI entry was removed; its saved configs now use the `agy` (Gemini) agent.
        if self.provider == ChatProvider::Gemini {
            self.provider = ChatProvider::Agy;
        }
        if !self.provider.is_cli() {
            let pid = self.provider.id().to_string();
            let key = self.api_key.trim().to_string();
            if !key.is_empty() && !self.api_keys.contains_key(&pid) {
                self.api_keys.insert(pid.clone(), key);
            }
            let ep = self.api_endpoint.trim().to_string();
            if !ep.is_empty() && !self.api_endpoints.contains_key(&pid) {
                self.api_endpoints.insert(pid, ep);
            }
        }
        self.api_key.clear();
        self.api_endpoint.clear();
    }

    pub(super) fn load_from_path(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let plain = seal::open(bytes)?;
        let mut cfg: Self = serde_json::from_slice(&plain).ok()?;
        cfg.migrate_legacy_api_fields();
        Some(cfg)
    }

    pub fn save(&self) -> bool {
        Self::path().is_some_and(|path| self.save_to_path(&path))
    }

    pub(super) fn save_to_path(&self, path: &Path) -> bool {
        let Ok(json) = serde_json::to_vec_pretty(self) else {
            return false;
        };
        let Ok(sealed) = seal::seal(json) else {
            return false;
        };
        crate::net::atomic_write_private(path, &sealed).is_ok()
    }

    pub fn effective_api_key(&self) -> &str {
        if let Some(k) = self.api_keys.get(self.provider.id()) {
            let t = k.trim();
            if !t.is_empty() {
                return t;
            }
        }
        if self.api_keys.is_empty() {
            let t = self.api_key.trim();
            if !t.is_empty() {
                return t;
            }
        }
        ""
    }

    /// Masked display of the saved key (`"••••••Ab12"`): proves a key is stored
    /// without ever rendering the secret itself. Empty when no key is saved.
    pub fn masked_api_key_hint(&self) -> String {
        let key = self.effective_api_key();
        if key.is_empty() {
            return String::new();
        }
        let tail: String = key.chars().rev().take(4).collect::<String>().chars().rev().collect();
        format!("••••••{tail}")
    }

    pub fn set_api_key(&mut self, key: String) {
        let trimmed = key.trim().to_string();
        if trimmed.is_empty() {
            self.api_keys.remove(self.provider.id());
        } else {
            self.api_keys.insert(self.provider.id().to_string(), trimmed);
        }
        self.api_key.clear();
    }

    pub fn effective_endpoint(&self) -> &str {
        if let Some(ep) = self.api_endpoints.get(self.provider.id()) {
            let t = ep.trim();
            if !t.is_empty() {
                return t;
            }
        }
        if self.api_endpoints.is_empty() {
            let t = self.api_endpoint.trim();
            if !t.is_empty() {
                return t;
            }
        }
        self.provider.default_endpoint()
    }

    pub fn set_api_endpoint(&mut self, ep: String) {
        let trimmed = ep.trim().to_string();
        if trimmed.is_empty() {
            self.api_endpoints.remove(self.provider.id());
        } else {
            self.api_endpoints.insert(self.provider.id().to_string(), trimmed);
        }
        self.api_endpoint.clear();
    }

    pub fn selected_model(&self, provider: ChatProvider) -> String {
        if let Some(m) = self.provider_models.get(provider.id())
            && !m.trim().is_empty()
        {
            return m.trim().to_string();
        }
        if !provider.is_cli() {
            if !self.api_model.trim().is_empty() {
                return self.api_model.trim().to_string();
            }
            return provider.default_model().to_string();
        }
        String::new()
    }

    pub fn set_selected_model(&mut self, provider: ChatProvider, model: String) {
        let trimmed = model.trim().to_string();
        if trimmed.is_empty() {
            self.provider_models.remove(provider.id());
        } else if is_valid_model_id(&trimmed) {
            self.provider_models.insert(provider.id().to_string(), trimmed.clone());
        } else {
            return;
        }
        if !provider.is_cli() {
            self.api_model = trimmed;
        }
    }

    pub fn selected_control_model(&self, provider: ChatProvider) -> String {
        self.control_models
            .get(provider.id())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    pub fn effective_control_model(&self, provider: ChatProvider) -> String {
        let ctrl = self.selected_control_model(provider);
        if !ctrl.is_empty() {
            ctrl
        } else if provider == ChatProvider::Agy {
            AGY_FAST_CONTROL_MODEL.to_string()
        } else {
            self.selected_model(provider)
        }
    }

    pub fn set_control_model(&mut self, provider: ChatProvider, model: String) {
        let trimmed = model.trim().to_string();
        if trimmed.is_empty() {
            self.control_models.remove(provider.id());
        } else if is_valid_model_id(&trimmed) {
            self.control_models.insert(provider.id().to_string(), trimmed);
        }
    }

    /// Stored `--effort` override for `provider` (`""` = auto).
    pub fn selected_effort(&self, provider: ChatProvider) -> String {
        self.effort_levels
            .get(provider.id())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    pub fn set_effort(&mut self, provider: ChatProvider, level: String) {
        let trimmed = level.trim().to_string();
        if trimmed.is_empty() {
            self.effort_levels.remove(provider.id());
        } else if is_valid_effort(&trimmed) {
            self.effort_levels.insert(provider.id().to_string(), trimmed);
        }
    }

    /// Effective `--effort` flag: the stored override, or auto (`low` in agent-control
    /// mode for fast control turns, no flag otherwise). `""` means "pass no flag".
    pub fn effective_effort(&self, provider: ChatProvider, agent_mode: bool) -> String {
        let selected = self.selected_effort(provider);
        if !selected.is_empty() {
            selected
        } else if agent_mode {
            "low".to_string()
        } else {
            String::new()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_label: Option<String>,
}

/// One saved conversation in the local chat history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: u64,
    pub title: String,
    /// Unix seconds of the last change.
    pub updated: u64,
    pub messages: Vec<ChatMessage>,
}

pub(super) const MAX_SAVED_SESSIONS: usize = 100;

/// Conversations saved on this PC (`chat_history.json`, DPAPI-sealed on Windows like `chat.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatHistory {
    pub sessions: Vec<ChatSession>,
}

impl ChatHistory {
    fn path() -> Option<PathBuf> {
        Some(app_data_dir()?.join("chat_history.json"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| Self::load_from_path(&p)).unwrap_or_default()
    }

    pub(super) fn load_from_path(path: &Path) -> Option<Self> {
        let plain = seal::open(std::fs::read(path).ok()?)?;
        serde_json::from_slice(&plain).ok()
    }

    pub fn save(&self) -> bool {
        Self::path().is_some_and(|path| self.save_to_path(&path))
    }

    pub(super) fn save_to_path(&self, path: &Path) -> bool {
        let Ok(json) = serde_json::to_vec(self) else {
            return false;
        };
        let Ok(sealed) = seal::seal(json) else {
            return false;
        };
        crate::net::atomic_write_private(path, &sealed).is_ok()
    }

    /// Inserts or replaces session `id` with `messages`, moving it to the top.
    pub fn upsert(&mut self, id: u64, messages: &[ChatMessage]) {
        let title = messages
            .iter()
            .find(|m| m.role == ChatRole::User)
            .map(|m| m.text.split_whitespace().collect::<Vec<_>>().join(" "))
            .map(|t| t.chars().take(48).collect::<String>())
            .unwrap_or_else(|| "새 대화".to_string());
        self.sessions.retain(|s| s.id != id);
        self.sessions.insert(
            0,
            ChatSession {
                id,
                title,
                updated: now_secs(),
                messages: messages.to_vec(),
            },
        );
        self.sessions.truncate(MAX_SAVED_SESSIONS);
    }

    pub fn remove(&mut self, id: u64) {
        self.sessions.retain(|s| s.id != id);
    }

    pub fn get(&self, id: u64) -> Option<&ChatSession> {
        self.sessions.iter().find(|s| s.id == id)
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
