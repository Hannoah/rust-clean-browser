use super::{THEME_CSS, enter_attrs, format_relative_time, truncate_chars};
use crate::cleaner::html_escape;
use crate::icons::{Icon, svg};
use std::fmt::Write;

// ── Docked AI Assistant Sidebar ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarPicker {
    #[default]
    None,
    Provider,
    Model,
}

pub struct SidebarRenderState<'a> {
    pub provider: crate::chat::ChatProvider,
    pub avail: crate::chat::CliAvailability,
    /// Provider entries to offer (pre-filtered to installed CLIs + connected APIs).
    pub providers: &'a [crate::chat::ChatProvider],
    pub messages: &'a [crate::chat::ChatMessage],
    pub is_loading: bool,
    pub include_page: bool,
    pub agent_control: bool,
    /// Request in flight: `(status line, steps so far)`.
    pub live: Option<(String, &'a [crate::chat::ActivityStep])>,
    /// Replies whose tool-card list is expanded.
    pub activity_open: &'a std::collections::HashSet<usize>,
    pub sidebar_wide: bool,
    pub show_settings: bool,
    pub picker_open: SidebarPicker,
    pub active_title: &'a str,
    pub active_url: &'a str,
    pub active_model_id: &'a str,
    pub active_model_label: &'a str,
    pub control_model_id: &'a str,
    /// Stored `--effort` override (`""` = auto). Only the Gemini/Claude CLI agents use it.
    pub effort: &'a str,
    pub custom_presets: &'a [crate::chat::AgentPreset],
    pub model_options: &'a [crate::chat::ModelOption],
    /// Model list fetch in flight (agy CLI or API endpoint).
    pub agy_models_loading: bool,
    /// Why the API endpoint's model list could not be fetched (presets shown instead).
    pub models_error: Option<&'a str>,
    /// Model IDs the user added by hand for this provider (removable with ×).
    pub custom_models: &'a [String],
    /// API providers only: `(estimated tokens the next request carries, context window)`.
    pub context: Option<(u32, u32)>,
    /// Conversation compaction (summary request) in flight.
    pub compacting: bool,
    /// Enough uncompacted turns to be worth summarizing.
    pub can_compact: bool,
    pub can_retry: bool,
    pub api_endpoint: &'a str,
    pub has_api_key: bool,
    pub draft: &'a str,
    /// Masked saved-key hint (`"••••••Ab12"`), never the secret itself.
    pub api_key_hint: &'a str,
    /// Last connection test for the active API provider: `(ok, summary)`.
    pub api_conn: Option<(bool, &'a str)>,
    /// Message text size in px.
    pub font_size: u32,
    /// `(message index, chars shown)` while a reply is typing out.
    pub reveal: Option<(usize, usize)>,
    pub show_history: bool,
    pub history: &'a [crate::chat::ChatSession],
    pub session_id: Option<u64>,
    /// `(action description, host)` of an agent navigation awaiting the user's OK.
    pub agent_confirm: Option<(&'a str, &'a str)>,
    pub motion: SidebarMotion,
}

/// Time-based entrance animations for the sidebar (see `enter_attrs`), in ms since appearing.
#[derive(Debug, Clone, Copy, Default)]
pub struct SidebarMotion {
    /// `(message index, ms)` for the newest message.
    pub msg: Option<(usize, u64)>,
    /// Settings card / history list / picker sheet that was just opened.
    pub panel_ms: Option<u64>,
    /// The sidebar itself, just opened.
    pub shell_ms: Option<u64>,
}

const ENTER_WINDOW_MS: u64 = 160;

/// Pixel height of the chat textarea showing `lines` lines (line-height 20px + padding).
pub fn chat_input_height(lines: usize) -> u32 {
    8 + 20 * lines.clamp(1, 6) as u32
}

// Text selection: THEME_CSS sets `* { user-select: none }`, and Blitz starts a drag-selection only
// when the pressed element computes `auto` AND its parent isn't `none`. So message text (and its
// wrappers, for the parent check) opts back in; buttons inside replies stay `none`.
const SIDEBAR_CSS: &str = r#"
html, body { width: 100%; height: 100%; overflow: hidden; background: var(--surface); user-select: none; }
.msg-u-wrap, .msg-u, .msg-a, .msg-body, .msg-body *, .msg-a.err > div { user-select: auto; }
.msg-body .chat-code-hdr, .msg-body .chat-code-hdr *, .msg-a-hdr, .msg-a-hdr *, .msg-a-ftr, .msg-a-ftr *,
.msg-u-ctx, .msg-u-ctx *, .err-acts, .err-acts *, [data-action], [data-action] * { user-select: none; }
[data-action] { cursor: pointer; }
.sb-shell {
  display: flex; flex-direction: column; width: 100%; height: 100%;
  border-left: 1px solid var(--border); background: var(--surface);
}
.sb-hdr {
  display: flex; align-items: center; justify-content: space-between;
  height: 46px; padding: 0 8px 0 14px;
  border-bottom: 1px solid var(--divider);
  background: var(--surface); flex-shrink: 0;
  /* Own stacking layer: Blitz hit-tests don't clip to scroll containers, so without this the
     scrolled-away messages/settings content above the list swallowed header clicks. */
  position: relative; z-index: 2;
}
.sb-title { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 700; color: var(--text); letter-spacing: -0.015em; min-width: 0; }
.sb-title-ico {
  width: 24px; height: 24px; border-radius: 8px;
  background: var(--ink); color: var(--ink-fg);
  display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
}
.sb-title-txt { white-space: nowrap; overflow: hidden; }
.sb-mode-pill {
  display: inline-flex; align-items: center; gap: 4px;
  height: 20px; padding: 0 7px; border-radius: 999px;
  background: var(--accent-bg); color: var(--accent-text);
  font-size: 10px; font-weight: 600; flex-shrink: 0;
}
.sb-acts { display: flex; align-items: center; gap: 1px; flex-shrink: 0; }
.sb-hbtn {
  display: inline-flex; align-items: center; justify-content: center;
  width: 30px; height: 30px; border-radius: 9px;
  color: var(--text-2);
}
.sb-hbtn:hover { background: var(--hover); color: var(--text); }
.sb-hbtn:active { background: var(--press); }
.sb-hbtn.on { background: var(--surface-3); color: var(--text); }

/* ── Header tabs ── */
.sb-tabs { display: flex; align-items: center; gap: 2px; padding: 2px; border-radius: 10px; background: var(--surface-3); }
.sb-tab {
  display: inline-flex; align-items: center; height: 26px; padding: 0 11px; border-radius: 8px;
  font-size: 12px; font-weight: 600; color: var(--text-2); white-space: nowrap;
}
.sb-tab:hover { color: var(--text); }
.sb-tab.on { background: var(--surface); color: var(--text); box-shadow: var(--shadow-xs); }

/* ── Settings tab (fills the sidebar body) ── */
.sb-settings {
  flex: 1; min-height: 0; overflow-y: auto;
  padding: 2px 16px 20px; background: var(--surface);
  display: flex; flex-direction: column;
}
.key-wrap { position: relative; }
.key-mask {
  position: absolute; left: 11px; right: 10px; top: 0; height: 32px; line-height: 32px;
  font-size: 12px; color: var(--text); white-space: nowrap; overflow: hidden; pointer-events: none;
}
.key-mask .key-ph { color: var(--text-3); }
.mdl-list {
  display: flex; flex-direction: column; gap: 2px; max-height: 280px; overflow-y: auto;
  border: 1px solid var(--border); border-radius: 10px; padding: 3px;
}
.mdl-del { padding: 0 6px; color: var(--text-3); font-size: 14px; }
.mdl-del:hover { color: var(--danger); }
.set-sec { padding: 10px 0; border-bottom: 1px solid var(--divider); display: flex; flex-direction: column; gap: 6px; }
.set-sec:last-child { border-bottom: none; padding-bottom: 2px; }
.set-lbl { font-size: 10.5px; font-weight: 700; color: var(--text-2); letter-spacing: 0.01em; }
.set-lbl-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.set-chips { display: flex; flex-wrap: wrap; gap: 5px; }
.set-chip {
  display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 10px;
  border-radius: 999px; border: 1px solid var(--border); background: var(--surface-2);
  font-size: 11px; font-weight: 500; color: var(--text-2); white-space: nowrap;
}
.set-chip:hover { background: var(--hover); color: var(--text); border-color: var(--border-strong); }
.set-chip.active { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); font-weight: 600; }
.set-chip.dim { opacity: 0.55; }
.set-inp {
  width: 100%; height: 32px; padding: 0 10px; border-radius: 9px;
  border: 1px solid var(--border); background: var(--surface-2);
  color: var(--text); font-size: 12px; outline: none;
}
.set-inp:focus { background: var(--surface); border-color: var(--accent); box-shadow: 0 0 0 2.5px var(--accent-ring); }
.set-inp.secret { color: transparent; caret-color: var(--text); }
.set-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.set-note { font-size: 10.5px; color: var(--text-3); line-height: 1.4; flex: 1; }
.set-btns { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.font-row { display: flex; align-items: center; gap: 6px; }
.font-step {
  display: inline-flex; align-items: center; justify-content: center;
  min-width: 30px; height: 28px; padding: 0 8px; border-radius: 8px;
  border: 1px solid var(--border); background: var(--surface-2);
  font-size: 13px; font-weight: 600; color: var(--text);
}
.font-step:hover { background: var(--hover); border-color: var(--border-strong); }
.font-val { min-width: 44px; text-align: center; font-size: 12px; font-weight: 600; color: var(--text); }
.font-prev { flex: 1; text-align: right; color: var(--text-2); white-space: nowrap; overflow: hidden; }
.sbtn {
  display: inline-flex; align-items: center; justify-content: center;
  height: 28px; padding: 0 12px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--surface);
  font-size: 11px; font-weight: 600; color: var(--text);
}
.sbtn:hover { background: var(--hover); }
.sbtn.primary { background: var(--ink); border-color: var(--ink); color: var(--ink-fg); }
.sbtn.primary:hover { opacity: 0.88; }
.sbtn.danger { color: var(--danger); }

/* ── Conversation ── */
.sb-msgs {
  flex: 1; min-height: 0; overflow-y: auto; padding: 16px 16px 18px;
  background: var(--surface);
  display: flex; flex-direction: column; gap: 20px;
}
.sb-empty {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  text-align: center; padding: 18px 4px 8px; gap: 9px; margin: auto 0;
}
.sb-emblem {
  width: 44px; height: 44px; border-radius: 14px;
  display: flex; align-items: center; justify-content: center;
  background: var(--ink); color: var(--ink-fg);
  box-shadow: var(--shadow-sm); margin-bottom: 2px;
}
.sb-empty-t { font-size: 17px; font-weight: 700; color: var(--text); letter-spacing: -0.025em; }
.sb-empty-s { font-size: 12px; color: var(--text-2); line-height: 1.5; max-width: 268px; }
.sb-page-pill {
  display: inline-flex; align-items: center; gap: 6px; max-width: 100%;
  padding: 4px 10px; border-radius: 999px;
  border: 1px solid var(--border); background: var(--surface-2);
  font-size: 11px; color: var(--text-2); white-space: nowrap; overflow: hidden;
}
.sb-page-dot { width: 6px; height: 6px; border-radius: 3px; background: var(--green); flex-shrink: 0; }
.sb-quick-grid {
  display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px;
  width: 100%; margin-top: 10px;
}
.qcard {
  display: flex; flex-direction: column; align-items: flex-start; gap: 3px;
  padding: 11px 12px; border-radius: 13px; text-align: left;
  border: 1px solid var(--border); background: var(--surface);
  box-shadow: var(--shadow-xs);
}
.qcard:hover { background: var(--surface-2); border-color: var(--border-strong); }
.qcard:active { background: var(--press); }
.qcard-top { display: flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; color: var(--text); }
.qcard-ico { color: var(--text-2); display: inline-flex; flex-shrink: 0; }
.qcard:hover .qcard-ico { color: var(--accent); }
.qcard-sub { font-size: 11px; color: var(--text-3); line-height: 1.35; }

.msg-u-wrap { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; }
.msg-u-ctx {
  display: inline-flex; align-items: center; gap: 5px;
  font-size: 10.5px; color: var(--text-3); padding-right: 4px; max-width: 86%; white-space: nowrap; overflow: hidden;
}
.msg-u {
  max-width: 86%; padding: 9px 13px;
  border-radius: 16px 16px 5px 16px;
  background: var(--accent-bg); color: var(--text);
  font-size: var(--chat-fs); line-height: 1.55; white-space: pre-wrap;
}
.msg-a { align-self: stretch; padding: 2px 0; color: var(--text); }
.msg-a-hdr { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
.msg-a-ico {
  width: 22px; height: 22px; border-radius: 7px;
  background: var(--ink); color: var(--ink-fg);
  display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
}
.msg-a-tag { font-size: 11.5px; font-weight: 600; color: var(--text-2); letter-spacing: -0.01em; }
.msg-body {
  padding-left: 30px;
  font-size: var(--chat-fs); line-height: 1.68; color: var(--text);
}
.msg-body p { margin: 0 0 10px; }
.msg-body p:last-child { margin-bottom: 0; }
.msg-body h3 { font-size: 1.08em; font-weight: 700; letter-spacing: -0.015em; margin: 16px 0 6px; }
.msg-body h4 { font-size: 1em; font-weight: 700; margin: 12px 0 4px; }
.msg-body ul, .msg-body ol { margin: 4px 0 10px; padding-left: 20px; }
.msg-body li { margin-bottom: 4px; }
.msg-body blockquote {
  margin: 10px 0; padding: 8px 12px; border-left: 3px solid var(--accent);
  background: var(--surface-2); border-radius: 0 9px 9px 0; color: var(--text-2);
}
.msg-body blockquote p { margin: 0 0 4px; }
.msg-body blockquote p:last-child { margin-bottom: 0; }
.msg-body hr { border: none; border-top: 1px solid var(--divider); margin: 14px 0; }
.msg-body a { color: var(--accent); text-decoration: underline; text-underline-offset: 2px; }
.msg-body code {
  padding: 1.5px 5px; border-radius: 5px; background: var(--surface-3);
  font-family: "Cascadia Code", Consolas, monospace; font-size: 0.92em;
}
.type-caret {
  display: inline-block; width: 7px; height: 1.05em; margin-left: 2px;
  border-radius: 2px; background: var(--accent); vertical-align: text-bottom;
}
.chat-tbl {
  margin: 10px 0; border-radius: 10px; border: 1px solid var(--border);
  overflow-x: auto; background: var(--surface);
}
.chat-tbl table { width: 100%; border-collapse: collapse; font-size: 0.92em; }
.chat-tbl th, .chat-tbl td {
  padding: 7px 10px; border-bottom: 1px solid var(--divider);
  border-right: 1px solid var(--divider); text-align: left;
}
.chat-tbl th:last-child, .chat-tbl td:last-child { border-right: none; }
.chat-tbl tr:last-child td { border-bottom: none; }
.chat-tbl th { font-weight: 600; color: var(--text-2); background: var(--surface-2); }
.chat-code {
  margin: 10px 0; border-radius: 11px; border: 1px solid var(--border);
  background: var(--surface-2); overflow: hidden;
}
.chat-code-hdr {
  display: flex; align-items: center; justify-content: space-between;
  padding: 4px 6px 4px 12px; border-bottom: 1px solid var(--divider);
  background: var(--surface-3); font-size: 11px; font-weight: 600; color: var(--text-2);
}
.chat-code pre {
  margin: 0; padding: 11px 13px; overflow-x: auto;
  font-family: "Cascadia Code", Consolas, monospace; font-size: 0.92em; line-height: 1.55;
}
.chat-code pre code { padding: 0; background: transparent; font-size: 1em; }
.msg-a-ftr { display: flex; align-items: center; gap: 2px; padding-left: 26px; margin-top: 6px; }
.chat-copy {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 3px 8px; border-radius: 999px; font-size: 11px; font-weight: 500; color: var(--text-3);
}
.chat-copy:hover { background: var(--hover); color: var(--text); }

.msg-a.err {
  padding: 11px 13px; border-radius: 12px;
  background: var(--danger-bg); color: var(--danger);
  font-size: 12.5px; line-height: 1.55;
}
.msg-a.err .msg-a-hdr { margin-bottom: 4px; }
.msg-a.err .msg-a-tag { color: var(--danger); }
.err-acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 9px; }
.err-retry {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 4px 10px; border-radius: 999px;
  background: var(--surface); border: 1px solid var(--border);
  font-size: 11px; font-weight: 600; color: var(--text);
}
.err-retry:hover { border-color: var(--border-strong); background: var(--surface-2); }

.sb-loading {
  display: flex; align-items: center; justify-content: space-between;
  padding: 4px 0 4px 2px; font-size: 12.5px; font-weight: 500; color: var(--text-2);
}
.sb-loading-l { display: flex; align-items: center; gap: 8px; }
.sb-agent-log {
  display: flex; flex-direction: column; gap: 4px;
  padding: 8px 11px; border-radius: 11px;
  background: var(--surface-2); border: 1px solid var(--border);
  font-size: 11px; color: var(--text-2);
}
.sb-agent-step { white-space: nowrap; overflow: hidden; line-height: 1.4; }
.sb-agent-step.err { color: var(--danger); }
.agent-confirm {
  display: flex; flex-direction: column; gap: 6px;
  padding: 12px 13px; border-radius: 13px;
  background: var(--amber-bg); border: 1px solid var(--amber);
  font-size: 12px; color: var(--text); line-height: 1.5;
}
.ac-t { font-weight: 700; color: var(--amber-text); }
.ac-d { font-size: 11px; color: var(--text-2); font-family: "Cascadia Code", Consolas, monospace; white-space: nowrap; overflow: hidden; }
.ac-btns { display: flex; justify-content: flex-end; gap: 6px; margin-top: 2px; }
@keyframes tab-spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
.fav-spin {
  display: inline-flex; align-items: center; justify-content: center;
  width: 20px; height: 20px; flex-shrink: 0;
  animation: tab-spin 0.8s linear infinite;
}

/* ── Conversation history ── */
.hist-hdr { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.hist-t { font-size: 13px; font-weight: 700; color: var(--text); }
.hist-sub { font-size: 11px; color: var(--text-3); margin-top: 2px; }
.hist-list { display: flex; flex-direction: column; gap: 4px; }
.hist-row {
  display: flex; align-items: center; gap: 10px;
  padding: 9px 8px 9px 12px; border-radius: 11px;
  border: 1px solid transparent;
}
.hist-row:hover { background: var(--hover); }
.hist-row.active { background: var(--surface-2); border-color: var(--border); }
.hist-main { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
.hist-title { font-size: 12.5px; font-weight: 600; color: var(--text); white-space: nowrap; overflow: hidden; }
.hist-meta { font-size: 10.5px; color: var(--text-3); white-space: nowrap; }
.hist-del {
  display: inline-flex; align-items: center; justify-content: center;
  width: 26px; height: 26px; border-radius: 8px; color: var(--text-3); flex-shrink: 0;
}
.hist-del:hover { background: var(--danger-bg); color: var(--danger); }
.hist-empty { padding: 36px 8px; text-align: center; font-size: 12px; color: var(--text-3); }

/* ── Picker sheet (above the composer) ── */
.sb-sheet {
  display: flex; flex-direction: column;
  margin: 0 12px 6px; border-radius: 14px;
  border: 1px solid var(--border-strong); background: var(--surface);
  box-shadow: var(--shadow-md); flex-shrink: 0; overflow: hidden;
  position: relative; z-index: 2; /* above the message list for hit-testing, like .sb-hdr */
}
.sheet-hdr {
  display: flex; align-items: center; justify-content: space-between;
  padding: 9px 12px 6px; font-size: 11px; font-weight: 600; color: var(--text-2);
}
.sheet-act {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 2px 8px; border-radius: 999px; font-size: 10.5px; font-weight: 500; color: var(--accent);
}
.sheet-act:hover { background: var(--accent-bg); }
.sheet-list { max-height: 220px; overflow-y: auto; padding: 2px 8px 8px; display: flex; flex-direction: column; gap: 2px; }
.sheet-sec { padding: 6px 6px 2px; font-size: 10px; font-weight: 700; color: var(--text-3); letter-spacing: 0.03em; }
.sheet-row {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  min-height: 32px; padding: 5px 9px; border-radius: 9px;
  font-size: 12px; color: var(--text);
}
.sheet-row:hover { background: var(--hover); }
.sheet-row.active { background: var(--surface-3); color: var(--text); font-weight: 600; }
.sheet-row-l { display: flex; align-items: center; gap: 7px; min-width: 0; flex: 1; }
.sheet-row-txt { display: flex; flex-direction: column; min-width: 0; }
.sheet-name { white-space: nowrap; overflow: hidden; }
.sheet-sub { font-size: 10px; color: var(--text-3); font-family: "Cascadia Code", Consolas, monospace; white-space: nowrap; overflow: hidden; }
.sheet-chk { color: var(--accent); display: inline-flex; flex-shrink: 0; }
.sheet-tag { font-size: 10px; color: var(--text-3); flex-shrink: 0; }
.sheet-ftr {
  display: flex; align-items: center; justify-content: space-between;
  padding: 8px 12px; border-top: 1px solid var(--divider); background: var(--surface-2);
  font-size: 11px; font-weight: 500; color: var(--text-2);
}
.sheet-ftr:hover { background: var(--hover); color: var(--text); }

/* ── Composer ── */
.sb-comp { padding: 6px 12px 10px; background: var(--surface); flex-shrink: 0; position: relative; z-index: 2; }
.composer {
  display: flex; flex-direction: column; gap: 8px;
  padding: 10px 10px 9px 14px; border-radius: 18px;
  border: 1px solid var(--border-strong); background: var(--surface-2);
  box-shadow: var(--shadow-sm), inset 0 0 0 1px var(--divider);
}
.composer:focus-within {
  border-color: var(--accent); background: var(--surface);
  box-shadow: 0 0 0 3px var(--accent-ring), var(--shadow-sm);
}
#chat-input {
  width: 100%; padding: 4px 0; border: none; background: transparent; resize: none;
  color: var(--text); font-size: 13px; line-height: 20px; outline: none;
  font-family: inherit; overflow-y: auto; max-height: 120px;
}
.comp-bar { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.comp-left { display: flex; align-items: center; gap: 4px; min-width: 0; flex: 1; overflow: hidden; }
.cpill {
  display: inline-flex; align-items: center; gap: 5px;
  height: 26px; padding: 0 9px; border-radius: 999px;
  background: var(--surface-2);
  font-size: 11px; font-weight: 600; color: var(--text-2);
  white-space: nowrap; min-width: 0;
}
.cpill:hover { background: var(--hover); color: var(--text); }
.cpill.on { background: var(--surface-3); color: var(--text); }
.cpill.mod { flex: 0 1 auto; }
.cpill.ctx.on { background: var(--green-bg); color: var(--green-text); }
.cpill.ctrl.on { background: var(--accent-bg); color: var(--accent-text); }
.cpill-txt { white-space: nowrap; overflow: hidden; }
.cpill-chev { color: var(--text-3); display: inline-flex; flex-shrink: 0; }
.cpill.on .cpill-chev { color: var(--text); }
.pdot { width: 6px; height: 6px; border-radius: 3px; flex-shrink: 0; }
.pdot.ok { background: var(--green); }
.pdot.off { background: var(--text-3); opacity: 0.45; }
.cpill-dot { width: 5px; height: 5px; border-radius: 3px; background: var(--text-3); opacity: 0.45; flex-shrink: 0; }
.cpill.ctx.on .cpill-dot { background: var(--green); opacity: 1; }
.cpill.ctrl.on .cpill-dot { background: var(--accent); opacity: 1; }
.send-btn {
  display: flex; align-items: center; justify-content: center;
  width: 30px; height: 30px; border-radius: 999px;
  background: var(--ink); color: var(--ink-fg); flex-shrink: 0;
}
.send-btn:hover { opacity: 0.85; }
.send-btn.stop { background: var(--danger); color: #ffffff; }
.comp-hint { padding: 5px 6px 0; font-size: 10px; color: var(--text-3); text-align: center; }
.ctx-row { display: flex; align-items: center; justify-content: space-between; gap: 8px; text-align: left; }
.ctx-meter { display: inline-flex; align-items: center; gap: 5px; white-space: nowrap; }
.ctx-meter.warn { color: #d97706; }
.ctx-meter.full { color: var(--danger); }
.ctx-compact { padding: 2px 8px; border-radius: 999px; font-weight: 600; color: var(--text-2); background: var(--surface-2); white-space: nowrap; }
.ctx-compact:hover { background: var(--hover); color: var(--text); }
.ctx-compact.busy { opacity: 0.6; }
.sum-card {
  margin: 4px 0 2px; padding: 10px 12px; border-radius: 12px;
  border: 1px dashed var(--border-strong); background: var(--surface-2);
}
.sum-hdr { display: flex; align-items: center; gap: 6px; font-size: 11px; font-weight: 700; color: var(--text-2); margin-bottom: 6px; }
.sum-hdr span:last-child { font-weight: 500; color: var(--text-3); }

/* ── Motion ── */
.sb-shell.rb-in { animation: rb-slide-l 120ms var(--ease-out) both; }
.sb-settings.rb-in, .sb-sheet.rb-in { animation: rb-drop 100ms var(--ease-out) both; }
.sb-hbtn, .set-chip, .sbtn, .cpill, .send-btn, .qcard, .chat-copy, .font-step, .hist-row,
.hist-del, .err-retry, .sheet-row, .sheet-ftr {
  transition: background 60ms ease, color 60ms ease, border-color 60ms ease, opacity 60ms ease,
    transform 80ms var(--ease-out);
}
.sb-hbtn:active, .set-chip:active, .sbtn:active, .cpill:active, .chat-copy:active, .font-step:active,
.hist-del:active, .err-retry:active, .send-btn:active, .qcard:active, .hist-row:active, .sheet-row:active {
  opacity: 0.78;
}
.qcard:hover { transform: translateY(-1px); }

/* ── Agent activity: live status line + tool cards ── */
.act-live { padding-left: 30px; font-size: 12.5px; color: var(--text-3); margin-bottom: 8px; }
.act-live-txt { animation: rb-pulse 1.4s ease-in-out infinite; }
.act-sum {
  display: inline-flex; align-items: center; gap: 4px; margin: 0 0 8px 30px;
  font-size: 12px; font-weight: 500; color: var(--text-3);
}
.act-sum:hover { color: var(--text); }
.act-sum .chev { display: inline-flex; transition: transform 80ms var(--ease-out); }
.act-sum.open .chev { transform: rotate(90deg); }
.act-list { display: flex; flex-direction: column; gap: 5px; margin: 0 0 10px 30px; }
.act-card {
  display: flex; align-items: center; gap: 8px; padding: 6px 10px;
  border: 1px solid var(--border); border-radius: 10px; background: var(--surface); font-size: 12px;
}
.act-name { font-family: "Cascadia Code", Consolas, monospace; font-weight: 500; color: var(--text); flex-shrink: 0; }
.act-detail { flex: 1; min-width: 0; color: var(--text-3); white-space: nowrap; overflow: hidden; }
.act-st { flex-shrink: 0; color: var(--text-3); }
.act-st.err { color: var(--danger); }
.act-st.run { color: var(--accent-text); }
.ftr-time { margin-left: auto; padding-right: 4px; font-size: 11px; color: var(--text-3); }

/* ── Answer skeleton (replaces the spinner while the model thinks) ── */
.sk-msg { display: flex; flex-direction: column; gap: 8px; padding-left: 30px; padding-top: 2px; }
.sk-line { height: 10px; border-radius: 6px; background: var(--surface-3); animation: rb-pulse 1.1s ease-in-out infinite; }
.sk-line.w1 { width: 92%; }
.sk-line.w2 { width: 76%; animation-delay: 140ms; }
.sk-line.w3 { width: 52%; animation-delay: 280ms; }
.msg-a-hdr .sk-stop { margin-left: auto; }
"#;

/// Inserts the typing caret inside the last text block so it trails the words, not a new line.
fn with_type_caret(mut body_html: String) -> String {
    const CARET: &str = r#"<span class="type-caret"></span>"#;
    for close in [
        "</p>",
        "</li>",
        "</h3>",
        "</h4>",
        "</code></pre></div>",
        "</td></tr></tbody></table></div>",
    ] {
        if body_html.ends_with(close) {
            let at = body_html.len() - close.len();
            body_html.insert_str(at, CARET);
            return body_html;
        }
    }
    body_html.push_str(CARET);
    body_html
}

fn render_settings_card(h: &mut String, state: &SidebarRenderState<'_>) {
    let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
    let _ = write!(
        h,
        r#"<div id="chat-settings" class="sb-settings{enter_cls}"{enter_style}>"#
    );
    h.push_str(r#"<div class="set-sec"><div class="set-lbl">AI 프로바이더</div><div class="set-chips">"#);
    for &p in state.providers {
        let is_act = p == state.provider;
        let cli_missing = p.is_cli() && !state.avail.is_available(p);
        let cls = match (is_act, cli_missing) {
            (true, true) => "set-chip active dim",
            (true, false) => "set-chip active",
            (false, true) => "set-chip dim",
            (false, false) => "set-chip",
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-provider:{pid}">{plbl}</div>"#,
            pid = p.id(),
            plbl = html_escape(p.short_label()),
        );
    }
    h.push_str("</div></div>");

    // Connection first (API: endpoint + key; CLI: login + effort).
    if state.provider.is_cli() {
        let _ = write!(
            h,
            r#"<div class="set-sec"><div class="set-lbl">연결</div><div class="set-row"><span class="set-note">{hint}</span><div class="set-btns"><div class="sbtn" data-action="chat-open-login">터미널 로그인</div></div></div></div>"#,
            hint = html_escape(state.provider.login_hint()),
        );
        // Only the Gemini (`agy`) and Claude CLI agents accept an `--effort` flag.
        if matches!(
            state.provider,
            crate::chat::ChatProvider::Agy | crate::chat::ChatProvider::Claude | crate::chat::ChatProvider::Muse
        ) {
            h.push_str(r#"<div class="set-sec"><div class="set-lbl">Effort (추론 강도 · 자동은 제어 모드에서 낮음)</div><div class="set-chips">"#);
            for (id, lbl) in [("", "자동"), ("low", "낮음"), ("medium", "중간"), ("high", "높음")] {
                let cls = if state.effort == id {
                    "set-chip active"
                } else {
                    "set-chip"
                };
                let _ = write!(h, r#"<div class="{cls}" data-action="chat-effort:{id}">{lbl}</div>"#);
            }
            h.push_str("</div></div>");
        }
    } else {
        let (key_lbl, key_ph): (String, &str) = if state.has_api_key {
            (
                format!("API Key · 저장됨 {}", state.api_key_hint),
                "바꾸려면 새 키 입력",
            )
        } else {
            ("API Key (로컬 Ollama는 비워두기)".to_string(), "sk-...")
        };
        let clear_key_btn = if state.has_api_key {
            r#"<div class="sbtn danger" data-action="chat-clear-key">키 삭제</div>"#
        } else {
            ""
        };
        let conn_row = match state.api_conn {
            Some((ok, msg)) => format!(
                r#"<div class="set-row"><span class="set-note"><span class="pdot {}"></span> {}</span></div>"#,
                if ok { "ok" } else { "off" },
                html_escape(msg),
            ),
            None => String::new(),
        };
        // Blitz draws `type=password` text in the clear: the input text is transparent and
        // `#chat-api-key-dots` (kept in sync by the app) shows one dot per character.
        let _ = write!(
            h,
            r#"<div class="set-sec">
  <div class="set-lbl">Endpoint URL (Ollama: http://localhost:11434/v1/chat/completions)</div><input id="chat-api-endpoint" class="set-inp" type="text" value="{ep}" />
  <div class="set-lbl">{key_lbl}</div><div class="key-wrap"><input id="chat-api-key" class="set-inp secret" type="password" value="" placeholder="" /><div class="key-mask"><span id="chat-api-key-ph" class="key-ph">{key_ph}</span><span id="chat-api-key-dots"></span></div></div>
  <div class="set-row">
    <span class="set-note">저장하면 연결을 확인하고 쓸 수 있는 모델을 불러옵니다.</span>
    <div class="set-btns">{clear_key_btn}<div class="sbtn" data-action="chat-test-connection">연결 테스트</div><div class="sbtn primary" data-action="chat-save-settings">저장</div></div>
  </div>
{conn_row}</div>"#,
            ep = html_escape(state.api_endpoint),
            key_lbl = html_escape(&key_lbl),
            key_ph = html_escape(key_ph),
        );
    }

    // Model: one list of what this provider can run (fetched from the endpoint / agy when
    // possible); click selects. IDs missing from the list are added below and selected at once.
    let refresh = if state.agy_models_loading {
        r#"<span class="sheet-tag">불러오는 중…</span>"#.to_string()
    } else if state.provider == crate::chat::ChatProvider::Agy || !state.provider.is_cli() {
        format!(
            r#"<div class="sheet-act" data-action="chat-refresh-models">{}<span>새로고침</span></div>"#,
            svg(Icon::Reload, 10)
        )
    } else {
        String::new()
    };
    let _ = write!(
        h,
        r#"<div class="set-sec"><div class="set-lbl-row"><span class="set-lbl">모델</span>{refresh}</div>"#,
    );
    if let Some(err) = state.models_error {
        let _ = write!(
            h,
            r#"<div class="set-note">모델 목록을 불러오지 못해 기본 목록을 표시합니다 · {}</div>"#,
            html_escape(&truncate_chars(err, 90)),
        );
    }
    let chk = svg(Icon::Check, 13);
    h.push_str(r#"<div class="mdl-list">"#);
    for opt in state.model_options {
        let is_act = opt.id == state.active_model_id;
        let row_cls = if is_act { "sheet-row active" } else { "sheet-row" };
        let sub = if !opt.id.is_empty() && opt.id != opt.label {
            format!(r#"<span class="sheet-sub">{}</span>"#, html_escape(&opt.id))
        } else {
            String::new()
        };
        let del = if state.custom_models.iter().any(|m| m == &opt.id) {
            format!(
                r#"<span class="mdl-del" data-action="chat-custom-model-del:{}" title="목록에서 삭제">×</span>"#,
                html_escape(&opt.id)
            )
        } else {
            String::new()
        };
        let right = if is_act {
            format!(r#"<span class="sheet-chk">{chk}</span>"#)
        } else {
            String::new()
        };
        let _ = write!(
            h,
            r#"<div class="{row_cls}" data-action="chat-model:{id}"><div class="sheet-row-l"><div class="sheet-row-txt"><span class="sheet-name">{lbl}</span>{sub}</div></div>{del}{right}</div>"#,
            id = html_escape(&opt.id),
            lbl = html_escape(&opt.label),
        );
    }
    h.push_str("</div>");
    h.push_str(r#"<div style="display:flex;gap:5px;align-items:center"><input id="chat-custom-model" class="set-inp" style="flex:1" type="text" value="" placeholder="목록에 없는 모델 ID 직접 입력" /><div class="sbtn" data-action="chat-custom-model-add">추가</div></div></div>"#);

    let _ = write!(
        h,
        r#"<div class="set-sec"><div class="set-lbl">글자 크기</div><div class="font-row"><div class="font-step" data-action="chat-font:dec" title="작게">−</div><span class="font-val">{fs}px</span><div class="font-step" data-action="chat-font:inc" title="크게">+</div><div class="font-step" data-action="chat-font:reset" title="기본 크기">기본</div><span class="font-prev" style="font-size:{fs}px">가나다 Abc</span></div></div>"#,
        fs = state.font_size,
    );

    h.push_str(r#"<div class="set-sec"><div class="set-lbl">브라우저 제어용 빠른 모델</div><div class="set-chips">"#);
    let ctrl_same_cls = if state.control_model_id.is_empty() {
        "set-chip active"
    } else {
        "set-chip"
    };
    let ctrl_auto_lbl = if state.provider == crate::chat::ChatProvider::Agy {
        "자동 (Flash Low · 빠름)"
    } else {
        "대화 모델과 동일"
    };
    let _ = write!(
        h,
        r#"<div class="{ctrl_same_cls}" data-action="chat-ctrl-model:">{ctrl_auto_lbl}</div>"#
    );
    for opt in state.model_options.iter().filter(|o| !o.id.is_empty()) {
        let cls = if opt.id == state.control_model_id {
            "set-chip active"
        } else {
            "set-chip"
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-ctrl-model:{id}">{lbl}</div>"#,
            id = html_escape(&opt.id),
            lbl = html_escape(&opt.label),
        );
    }
    h.push_str("</div></div>");

    h.push_str(r#"<div class="set-sec"><div class="set-lbl">제어 프리셋 (원클릭 매크로)</div>"#);
    if !state.custom_presets.is_empty() {
        h.push_str(r#"<div class="set-chips">"#);
        for (i, preset) in state.custom_presets.iter().enumerate() {
            let _ = write!(
                h,
                r#"<div class="set-chip"><span>{title}</span><span style="margin-left:4px;opacity:0.7" data-action="chat-preset-del:{i}" title="삭제">×</span></div>"#,
                title = html_escape(&preset.title),
            );
        }
        h.push_str("</div>");
    }
    h.push_str(
        r#"<div style="display:flex;gap:5px;align-items:center"><input id="chat-preset-title" class="set-inp" style="flex:0 0 110px" type="text" value="" placeholder="이름" /><input id="chat-preset-prompt" class="set-inp" style="flex:1" type="text" value="" placeholder="실행할 지시 내용" /><div class="sbtn" data-action="chat-preset-add">추가</div></div><div class="set-note">이름과 지시 내용을 입력하고 추가하면 빈 대화 화면에 실행 카드로 나타납니다.</div></div>"#,
    );
    h.push_str("</div>");
}

fn fmt_secs(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.1}초", ms as f64 / 1000.0)
    } else if ms < 60_000 {
        format!("{}초", ms / 1000)
    } else {
        format!("{}분 {}초", ms / 60_000, (ms / 1000) % 60)
    }
}

/// Thin bordered cards, one per tool call / browser action.
fn activity_cards(steps: &[crate::chat::ActivityStep]) -> String {
    let mut out = String::from(r#"<div class="act-list">"#);
    for s in steps {
        let (cls, st) = if s.running {
            ("act-st run", "실행 중".to_string())
        } else if s.ok {
            (
                "act-st",
                if s.ms > 0 {
                    format!("완료 · {}", fmt_secs(s.ms))
                } else {
                    "완료".to_string()
                },
            )
        } else {
            ("act-st err", "오류".to_string())
        };
        let _ = write!(
            out,
            r#"<div class="act-card"><span class="act-name">{}</span><span class="act-detail">{}</span><span class="{cls}">{st}</span></div>"#,
            html_escape(&s.name),
            html_escape(&s.detail),
        );
    }
    out.push_str("</div>");
    out
}

fn fmt_tokens(n: u32) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f32 / 1_000_000.0),
        1_000.. => format!("{:.1}k", n as f32 / 1_000.0),
        _ => n.to_string(),
    }
}

/// Under the composer: the API context meter + 대화 압축, or the key hint for CLI agents
/// (they keep their own context).
fn context_row(state: &SidebarRenderState<'_>) -> String {
    let Some((used, max)) = state.context else {
        return r#"<div class="comp-hint">Enter 전송 · Shift+Enter 줄바꿈</div>"#.to_string();
    };
    let pct = (used as u64 * 100 / max.max(1) as u64).min(100) as u32;
    let lvl = match pct {
        90.. => " full",
        70.. => " warn",
        _ => "",
    };
    let dash = 37.7 * pct as f32 / 100.0;
    let ring = format!(
        r#"<svg width="12" height="12" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="2.5"/><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" stroke-dasharray="{dash:.1} 37.7" transform="rotate(-90 8 8)"/></svg>"#
    );
    let btn = if state.compacting {
        r#"<span class="ctx-compact busy">압축 중…</span>"#
    } else if state.can_compact && !state.is_loading {
        r#"<span class="ctx-compact" data-action="chat-compact" title="지금까지의 대화를 요약해 컨텍스트를 비웁니다">대화 압축</span>"#
    } else {
        ""
    };
    format!(
        r#"<div class="comp-hint ctx-row"><span class="ctx-meter{lvl}" title="다음 요청에 실릴 대화 추정치 (페이지 본문 제외) · 한글 기준 대략치">{ring}<span>컨텍스트 {} / {} · {pct}%</span></span>{btn}</div>"#,
        fmt_tokens(used),
        fmt_tokens(max),
    )
}

fn render_history_panel(h: &mut String, state: &SidebarRenderState<'_>) {
    let del_all = if state.history.is_empty() {
        String::new()
    } else {
        r#"<div class="sbtn danger" data-action="chat-del-all">전체 삭제</div>"#.to_string()
    };
    let _ = write!(
        h,
        r#"<div class="hist-hdr"><div><div class="hist-t">대화 기록</div><div class="hist-sub">이 PC에만 저장됩니다 · {n}개</div></div>{del_all}</div>"#,
        n = state.history.len(),
    );
    if state.history.is_empty() {
        h.push_str(r#"<div class="hist-empty">저장된 대화가 없습니다.<br/>대화를 시작하면 자동으로 저장됩니다.</div>"#);
        return;
    }
    let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
    let _ = write!(h, r#"<div class="hist-list{enter_cls}"{enter_style}>"#);
    let trash = svg(Icon::Trash, 13);
    for s in state.history {
        let cls = if Some(s.id) == state.session_id {
            "hist-row active"
        } else {
            "hist-row"
        };
        let _ = write!(
            h,
            r#"<div class="{cls}" data-action="chat-load:{id}"><div class="hist-main"><span class="hist-title">{title}</span><span class="hist-meta">{when} · 메시지 {n}개</span></div><span class="hist-del" data-action="chat-del:{id}" title="이 대화 삭제">{trash}</span></div>"#,
            id = s.id,
            title = html_escape(&s.title),
            when = format_relative_time(s.updated),
            n = s.messages.len(),
        );
    }
    h.push_str("</div>");
}

fn render_quick_actions(state: &SidebarRenderState<'_>, is_internal: bool) -> String {
    let cards: &[(&str, Icon, &str, &str)] = if state.agent_control {
        &[
            (
                "chat-quick:extract-tables",
                Icon::Sparkles,
                "표 데이터 추출",
                "테이블 전체를 표로 정리",
            ),
            (
                "chat-quick:compare-tabs",
                Icon::BookOpen,
                "열린 탭 비교",
                "탭 전환하며 핵심 비교",
            ),
            (
                "chat-quick:extract-links",
                Icon::ExternalLink,
                "주요 링크 수집",
                "페이지 링크 분류 정리",
            ),
            ("chat-quick:summary", Icon::Zap, "페이지 요약", "핵심 내용 한눈에"),
        ]
    } else if is_internal {
        &[]
    } else {
        &[
            ("chat-quick:summary", Icon::Sparkles, "페이지 요약", "핵심 내용 한눈에"),
            ("chat-quick:bullets", Icon::BookOpen, "핵심 포인트", "3~5줄 불릿 정리"),
            ("chat-quick:explain-ko", Icon::Zap, "쉽게 설명", "한국어 쉬운 해설"),
            (
                "chat-quick:translate",
                Icon::ExternalLink,
                "한국어 번역",
                "자연스러운 번역",
            ),
        ]
    };
    if cards.is_empty() && state.custom_presets.is_empty() {
        return String::new();
    }
    let mut qa = String::from(r#"<div class="sb-quick-grid">"#);
    for (action, icon, title, sub) in cards {
        let _ = write!(
            qa,
            r#"<div class="qcard" data-action="{action}"><div class="qcard-top"><span class="qcard-ico">{ico}</span><span>{title}</span></div><div class="qcard-sub">{sub}</div></div>"#,
            ico = svg(*icon, 13),
        );
    }
    for (i, preset) in state.custom_presets.iter().enumerate() {
        let _ = write!(
            qa,
            r#"<div class="qcard" data-action="chat-preset-run:{i}"><div class="qcard-top"><span class="qcard-ico">{ico}</span><span>{title}</span></div><div class="qcard-sub">{sub}</div></div>"#,
            ico = svg(Icon::Zap, 13),
            title = html_escape(&truncate_chars(&preset.title, 18)),
            sub = html_escape(&truncate_chars(&preset.prompt, 26)),
        );
    }
    qa.push_str("</div>");
    qa
}

pub fn render_sidebar_html(state: &SidebarRenderState<'_>) -> (String, Vec<String>) {
    let mut code_blocks = Vec::new();
    let mut h = String::with_capacity(16 * 1024);
    h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    h.push_str(THEME_CSS);
    h.push_str(SIDEBAR_CSS);
    let _ = write!(h, ":root {{ --chat-fs: {}px; }}", state.font_size);
    let on = |b: bool| if b { "sb-hbtn on" } else { "sb-hbtn" };
    let tab = |b: bool| if b { "sb-tab on" } else { "sb-tab" };
    let mode_badge = if state.agent_control {
        r#"<span class="sb-mode-pill"><span class="pdot ok"></span><span>제어</span></span>"#
    } else {
        ""
    };
    let _ = write!(
        h,
        r#"</style></head><body><div class="sb-shell{shell_cls}"{shell_style}>
<div class="sb-hdr">
  <div class="sb-title"><span class="sb-title-ico">{spark}</span><div class="sb-tabs"><div class="{chat_tab}" data-action="chat-tab:chat">대화</div><div class="{hist_tab}" data-action="chat-tab:history">기록</div><div class="{set_tab}" data-action="chat-tab:settings">설정</div></div>{mode_badge}</div>
  <div class="sb-acts">
    <div class="sb-hbtn" data-action="chat-clear" title="새 대화">{plus}</div>
    <div class="{width_cls}" data-action="chat-toggle-width" title="사이드바 폭 전환 (경계 드래그로도 조절)">{width_ico}</div>
    <div class="sb-hbtn" data-action="toggle-sidebar" title="닫기 (Ctrl+Shift+I)">{close}</div>
  </div>
</div>"#,
        spark = svg(Icon::Sparkles, 13),
        plus = svg(Icon::Plus, 14),
        width_ico = svg(Icon::MoveHorizontal, 13),
        close = svg(Icon::Close, 12),
        chat_tab = tab(!state.show_history && !state.show_settings),
        hist_tab = tab(state.show_history),
        set_tab = tab(state.show_settings),
        width_cls = on(state.sidebar_wide),
        shell_cls = enter_attrs(state.motion.shell_ms, ENTER_WINDOW_MS).0,
        shell_style = enter_attrs(state.motion.shell_ms, ENTER_WINDOW_MS).1,
    );

    if state.show_settings {
        render_settings_card(&mut h, state);
    }

    // The message list stays in the DOM (hidden) on the settings tab so its scroll survives.
    let _ = write!(
        h,
        r#"<div id="chat-messages" class="sb-msgs"{}>"#,
        if state.show_settings {
            r#" style="display:none""#
        } else {
            ""
        }
    );
    let is_internal = state.active_url.starts_with("rust://");
    if state.show_history {
        render_history_panel(&mut h, state);
    } else if state.messages.is_empty() && !state.is_loading {
        let page_pill = if is_internal {
            String::new()
        } else {
            format!(
                r#"<div class="sb-page-pill"><span class="sb-page-dot"></span><span>{}</span></div>"#,
                html_escape(&truncate_chars(state.active_title, 34))
            )
        };
        let _ = write!(
            h,
            r#"<div class="sb-empty">
  <div class="sb-emblem">{emblem}</div>
  <div class="sb-empty-t">무엇을 도와드릴까요?</div>
  <div class="sb-empty-s">웹페이지를 읽고 요약·번역·코드 분석까지 바로 답해드립니다.</div>
  {page_pill}
  {quick}
</div>"#,
            emblem = svg(Icon::Sparkles, 20),
            quick = render_quick_actions(state, is_internal),
        );
    } else {
        let copy_svg = svg(Icon::Copy, 11);
        let regen_svg = svg(Icon::Reload, 11);
        let spark_mini = svg(Icon::Sparkles, 11);
        let msg_count = state.messages.len();
        let last_assistant = state
            .messages
            .iter()
            .rposition(|m| m.role == crate::chat::ChatRole::Assistant);
        for (idx, msg) in state.messages.iter().enumerate() {
            let enter_ms = state.motion.msg.filter(|(i, _)| *i == idx).map(|(_, ms)| ms);
            let (enter_cls, enter_style) = enter_attrs(enter_ms, ENTER_WINDOW_MS);
            match msg.role {
                crate::chat::ChatRole::User => {
                    let ctx_badge = msg
                        .page_title
                        .as_deref()
                        .map(|t| {
                            format!(
                                r#"<div class="msg-u-ctx"><span class="sb-page-dot"></span>페이지 포함 · {}</div>"#,
                                html_escape(&truncate_chars(t, 28))
                            )
                        })
                        .unwrap_or_default();
                    let _ = write!(
                        h,
                        r#"<div class="msg-u-wrap{enter_cls}"{enter_style}>{ctx_badge}<div class="msg-u">{}</div></div>"#,
                        html_escape(&msg.text),
                    );
                }
                crate::chat::ChatRole::Assistant => {
                    let typing = state.reveal.filter(|(i, _)| *i == idx).map(|(_, n)| n);
                    let body_html = match typing {
                        Some(n) => {
                            let partial: String = msg.text.chars().take(n).collect();
                            with_type_caret(crate::chat::render_markdown_to_html(&partial, &mut code_blocks))
                        }
                        None => crate::chat::render_markdown_to_html(&msg.text, &mut code_blocks),
                    };
                    let footer = if typing.is_some() {
                        String::new()
                    } else {
                        let regen = if Some(idx) == last_assistant && !state.is_loading {
                            format!(
                                r#"<span class="chat-copy" data-action="chat-regen" title="이 답변 다시 생성">{regen_svg}<span>다시 생성</span></span>"#
                            )
                        } else {
                            String::new()
                        };
                        let time = msg
                            .elapsed_ms
                            .map(|ms| format!(r#"<span class="ftr-time">{}</span>"#, fmt_secs(ms)))
                            .unwrap_or_default();
                        format!(
                            r#"<div class="msg-a-ftr"><span class="chat-copy" data-action="chat-copy-msg:{idx}" title="답변 전체 복사">{copy_svg}<span>복사</span></span>{regen}{time}</div>"#
                        )
                    };
                    let activity = if msg.activity.is_empty() {
                        String::new()
                    } else {
                        let open = state.activity_open.contains(&idx);
                        let errs = msg.activity.iter().filter(|s| !s.ok).count();
                        let label = if errs > 0 {
                            format!("도구 {}개 사용 · 오류 {errs}", msg.activity.len())
                        } else {
                            format!("도구 {}개 사용", msg.activity.len())
                        };
                        format!(
                            r#"<div class="act-sum{}" data-action="chat-activity:{idx}"><span>{label}</span><span class="chev">{}</span></div>{}"#,
                            if open { " open" } else { "" },
                            svg(Icon::ChevronRight, 12),
                            if open {
                                activity_cards(&msg.activity)
                            } else {
                                String::new()
                            },
                        )
                    };
                    let tag = msg.provider_label.as_deref().unwrap_or("AI");
                    // A typing reply is its own entrance; don't stack a slide on top of it.
                    let (enter_cls, enter_style) = if typing.is_some() {
                        ("", String::new())
                    } else {
                        (enter_cls, enter_style)
                    };
                    let _ = write!(
                        h,
                        r#"<div class="msg-a{enter_cls}"{enter_style}><div class="msg-a-hdr"><span class="msg-a-ico">{spark_mini}</span><span class="msg-a-tag">{tag}</span></div>{activity}<div class="msg-body">{body_html}</div>{footer}</div>"#,
                        tag = html_escape(tag),
                    );
                }
                crate::chat::ChatRole::Summary => {
                    let _ = write!(
                        h,
                        r#"<div class="sum-card{enter_cls}"{enter_style}><div class="sum-hdr">{ico}<span>이전 대화 요약</span><span>· 이후 요청엔 이 요약과 새 대화만 전달</span></div><div class="msg-body">{body}</div></div>"#,
                        ico = svg(Icon::BookOpen, 12),
                        body = crate::chat::render_markdown_to_html(&msg.text, &mut code_blocks),
                    );
                }
                crate::chat::ChatRole::Error => {
                    let tag = msg.provider_label.as_deref().unwrap_or("안내");
                    let show_retry = state.can_retry && idx + 1 == msg_count;
                    let show_login = state.provider.is_cli();
                    let mut acts_html = String::new();
                    if show_retry || show_login {
                        acts_html.push_str(r#"<div class="err-acts">"#);
                        if show_retry {
                            let _ = write!(
                                acts_html,
                                r#"<div class="err-retry" data-action="chat-retry">{}<span>다시 시도</span></div>"#,
                                svg(Icon::Reload, 11)
                            );
                        }
                        if show_login {
                            acts_html.push_str(
                                r#"<div class="err-retry" data-action="chat-open-login"><span>로그인 열기</span></div>"#,
                            );
                        }
                        acts_html.push_str("</div>");
                    }
                    let _ = write!(
                        h,
                        r#"<div class="msg-a err{enter_cls}"{enter_style}><div class="msg-a-hdr"><span class="msg-a-tag">{tag}</span></div><div>{}</div>{acts_html}</div>"#,
                        html_escape(&msg.text),
                        tag = html_escape(tag),
                    );
                }
            }
        }
    }

    if state.is_loading && !state.show_history {
        let (live_lbl, steps) = state.live.clone().unwrap_or_else(|| ("생각 중…".to_string(), &[]));
        // Skeleton lines only while nothing is known yet; then the steps take their place.
        let skeleton = if steps.is_empty() && !live_lbl.starts_with("응답 작성") {
            r#"<div class="sk-msg"><div class="sk-line w1"></div><div class="sk-line w2"></div><div class="sk-line w3"></div></div>"#
        } else {
            ""
        };
        let cards = if steps.is_empty() {
            String::new()
        } else {
            activity_cards(steps)
        };
        let _ = write!(
            h,
            r#"<div class="msg-a rb-in"><div class="msg-a-hdr"><span class="msg-a-ico">{spark}</span><span class="msg-a-tag">{prov}</span><span class="chat-copy sk-stop" data-action="chat-stop">중지</span></div><div class="act-live"><span id="act-live-txt" class="act-live-txt">{lbl}</span></div>{cards}{skeleton}</div>"#,
            spark = svg(Icon::Sparkles, 11),
            prov = html_escape(state.provider.short_label()),
            lbl = html_escape(&live_lbl),
        );
        if let Some((desc, host)) = state.agent_confirm {
            let _ = write!(
                h,
                r#"<div class="agent-confirm"><div class="ac-t">새 사이트로 이동 요청</div><div>AI가 <b>{host}</b>(으)로 이동하려고 합니다. 웹페이지에 숨겨진 지시일 수 있으니 요청한 작업이 맞는지 확인하세요.</div><div class="ac-d">{desc}</div><div class="ac-btns"><div class="sbtn" data-action="agent-confirm:deny">중단</div><div class="sbtn primary" data-action="agent-confirm:allow">허용</div></div></div>"#,
                host = html_escape(host),
                desc = html_escape(desc),
            );
        }
    }
    h.push_str("</div>");

    match state.picker_open {
        SidebarPicker::Provider => {
            let chk = svg(Icon::Check, 13);
            let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
            let _ = write!(
                h,
                r#"<div class="sb-sheet{enter_cls}"{enter_style}><div class="sheet-hdr"><span>에이전트 / API 선택</span></div><div class="sheet-list"><div class="sheet-sec">로컬 로그인 에이전트</div>"#
            );
            for &prov in state.providers {
                if prov == crate::chat::ChatProvider::OpenAiApi {
                    h.push_str(r#"<div class="sheet-sec">직접 API / 로컬 LLM</div>"#);
                }
                let is_act = prov == state.provider;
                let row_cls = if is_act { "sheet-row active" } else { "sheet-row" };
                let dot = if prov.is_cli() {
                    let dc = if state.avail.is_available(prov) {
                        "pdot ok"
                    } else {
                        "pdot off"
                    };
                    format!(r#"<span class="{dc}"></span>"#)
                } else {
                    String::new()
                };
                let right = if is_act {
                    format!(r#"<span class="sheet-chk">{chk}</span>"#)
                } else if prov.is_cli() && !state.avail.is_available(prov) {
                    r#"<span class="sheet-tag">미설치</span>"#.to_string()
                } else {
                    String::new()
                };
                let _ = write!(
                    h,
                    r#"<div class="{row_cls}" data-action="chat-provider:{id}"><div class="sheet-row-l">{dot}<span class="sheet-name">{lbl}</span></div>{right}</div>"#,
                    id = prov.id(),
                    lbl = html_escape(prov.label()),
                );
            }
            h.push_str("</div></div>");
        }
        SidebarPicker::Model => {
            let chk = svg(Icon::Check, 13);
            let refresh_btn = if state.provider == crate::chat::ChatProvider::Agy {
                if state.agy_models_loading {
                    r#"<span class="sheet-tag">불러오는 중…</span>"#.to_string()
                } else {
                    format!(
                        r#"<div class="sheet-act" data-action="chat-refresh-models">{}<span>새로고침</span></div>"#,
                        svg(Icon::Reload, 11)
                    )
                }
            } else {
                String::new()
            };
            let (enter_cls, enter_style) = enter_attrs(state.motion.panel_ms, ENTER_WINDOW_MS);
            let _ = write!(
                h,
                r#"<div class="sb-sheet{enter_cls}"{enter_style}><div class="sheet-hdr"><span>{} 모델 선택</span>{refresh_btn}</div><div class="sheet-list">"#,
                html_escape(state.provider.short_label()),
            );
            for opt in state.model_options {
                let is_act = opt.id == state.active_model_id;
                let row_cls = if is_act { "sheet-row active" } else { "sheet-row" };
                let sub = if !opt.id.is_empty() && opt.id != opt.label {
                    format!(r#"<span class="sheet-sub">{}</span>"#, html_escape(&opt.id))
                } else {
                    String::new()
                };
                let right = if is_act {
                    format!(r#"<span class="sheet-chk">{chk}</span>"#)
                } else {
                    String::new()
                };
                let _ = write!(
                    h,
                    r#"<div class="{row_cls}" data-action="chat-model:{id}"><div class="sheet-row-l"><div class="sheet-row-txt"><span class="sheet-name">{lbl}</span>{sub}</div></div>{right}</div>"#,
                    id = html_escape(&opt.id),
                    lbl = html_escape(&opt.label),
                );
            }
            let _ = write!(
                h,
                r#"</div><div class="sheet-ftr" data-action="chat-tab:settings"><span>모델 추가 · 설정</span>{}</div></div>"#,
                svg(Icon::ChevronRight, 12),
            );
        }
        SidebarPicker::None => {}
    }

    let prov_pill_cls = if state.picker_open == SidebarPicker::Provider {
        "cpill on"
    } else {
        "cpill"
    };
    let mod_pill_cls = if state.picker_open == SidebarPicker::Model {
        "cpill mod on"
    } else {
        "cpill mod"
    };
    let prov_dot = if state.provider.is_cli() {
        let cls = if state.avail.is_available(state.provider) {
            "pdot ok"
        } else {
            "pdot off"
        };
        format!(r#"<span class="{cls}"></span>"#)
    } else {
        String::new()
    };
    // Internal pages (new tab, history, …) have no page text to send: the pill
    // renders visibly disabled and is not clickable, so it can never look stuck
    // and a stray click can never silently flip the saved preference.
    let (ctx_cls, ctx_action, ctx_tip) = if is_internal {
        (
            "cpill ctx",
            String::new(),
            "내부 페이지에서는 페이지 내용을 함께 보낼 수 없습니다".to_string(),
        )
    } else if state.include_page {
        (
            "cpill ctx on",
            r#" data-action="chat-toggle-page""#.to_string(),
            "현재 페이지 본문을 함께 보냄 (끄려면 클릭)".to_string(),
        )
    } else {
        (
            "cpill ctx",
            r#" data-action="chat-toggle-page""#.to_string(),
            "현재 페이지 본문 포함 켜기".to_string(),
        )
    };
    let ctrl_cls = if state.agent_control {
        "cpill ctrl on"
    } else {
        "cpill ctrl"
    };
    let chev = svg(Icon::ChevronDown, 10);
    let send_btn = if state.is_loading {
        format!(
            r#"<div class="send-btn stop" data-action="chat-stop" title="생성 중지">{}</div>"#,
            svg(Icon::Stop, 12)
        )
    } else {
        format!(
            r#"<div class="send-btn" data-action="chat-send" title="전송 (Enter)">{}</div>"#,
            svg(Icon::Send, 13)
        )
    };
    let input_h = chat_input_height(state.draft.split('\n').count());
    let _ = write!(
        h,
        r#"<div class="sb-comp"{comp_style}>
  <div class="composer">
    <textarea id="chat-input" rows="1" style="height:{input_h}px" placeholder="무엇이든 물어보세요…">{draft}</textarea>
    <div class="comp-bar">
      <div class="comp-left">
        <div class="{prov_pill_cls}" data-action="chat-picker:provider" title="에이전트 / API 전환">{prov_dot}<span class="cpill-txt">{prov_lbl}</span><span class="cpill-chev">{chev}</span></div>
        <div class="{mod_pill_cls}" data-action="chat-picker:model" title="사용할 모델 변경"><span class="cpill-txt">{mod_lbl}</span><span class="cpill-chev">{chev}</span></div>
        <div class="{ctx_cls}"{ctx_action} title="{ctx_tip}"><span class="cpill-dot"></span><span class="cpill-txt">페이지</span></div>
        <div class="{ctrl_cls}" data-action="chat-toggle-control" title="AI 에이전트 브라우저 화면 제어 (클릭·입력·이동)"><span class="cpill-dot"></span><span class="cpill-txt">제어</span></div>
      </div>
      {send_btn}
    </div>
  </div>
  {hint_row}
</div>
</div></body></html>"#,
        draft = html_escape(state.draft),
        hint_row = context_row(state),
        // Hidden (not removed) off the chat tab so the typed draft survives tab switches.
        comp_style = if state.show_settings || state.show_history {
            r#" style="display:none""#
        } else {
            ""
        },
        prov_lbl = html_escape(state.provider.short_label()),
        mod_lbl = html_escape(&truncate_chars(state.active_model_label, 15)),
    );

    (h, code_blocks)
}
