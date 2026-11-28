use super::context::{format_user_turn, neutralize_untrusted};
use super::{AgentContext, ChatMessage, ChatRole, MAX_HISTORY_TURNS, MAX_TURN_CHARS, PageContext, SYSTEM_PROMPT};

const UNTRUSTED_STATE_TAG: &str = "untrusted_agent_state";

pub(super) const AGENT_CONTROL_RULES: &str = "\
Browser control agent. Use [INTERACTIVE ELEMENTS] IDs (`\"target\":ID`) and [OPEN TABS] indices.\n\
Output rule (STRICT):\n\
- If the request needs no browser actions (already answered by [PAGE TEXT]/[INTERACTIVE ELEMENTS]), answer concisely in Korean with NO JSON.\n\
- For intermediate steps (`done` omitted/false): output ONLY one minified ```browser-action JSON block with ZERO text before or after.\n\
- For the final step, add `\"done\":true,\"summary\":\"한국어 요약\"` (or `{\"action\":\"done\",\"summary\":\"한국어 요약\"}`).\n\
Batch up to 6 actions: `{\"actions\":[...],\"done\":true,\"summary\":\"...\"}` or single `{\"action\":\"click\",\"target\":ID}`.\n\
Actions: click{target} | click_text{text} | hover{target} | type{target,text,clear,submit} | select{target,value} | press{key} | scroll{direction,amount,target?} | navigate{url} | back | forward | reload | wait{ms} | wait_for{text,timeout_ms} | open_tab{url} | switch_tab{tab} | close_tab{tab} | extract{kind:\"tables|links|text|all\",copy} | done{summary}.\n\
Optional top-level `\"note\":\"...\"` persists facts across tabs. Never repeat Previous actions. Stop on login/captcha with `done`.";

pub(super) fn format_agent_state_block(agent: &AgentContext) -> String {
    if agent.notes.is_empty() && agent.action_log.is_empty() {
        return String::new();
    }
    let mut out = format!("<{UNTRUSTED_STATE_TAG}>\n");
    if !agent.notes.is_empty() {
        out.push_str("[AGENT NOTES]\n");
        for note in agent.notes.iter().take(10) {
            let clean = neutralize_untrusted(note).replace(['\r', '\n'], " ");
            let capped: String = clean.chars().take(200).collect();
            out.push_str("- ");
            out.push_str(&capped);
            out.push('\n');
        }
    }
    if !agent.action_log.is_empty() {
        if !agent.notes.is_empty() {
            out.push('\n');
        }
        out.push_str("Previous actions (do NOT repeat these):\n");
        for entry in &agent.action_log {
            let clean = neutralize_untrusted(entry).replace(['\r', '\n'], " ");
            let capped: String = clean.chars().take(200).collect();
            out.push_str("- ");
            out.push_str(&capped);
            out.push('\n');
        }
    }
    out.push_str(&format!("</{UNTRUSTED_STATE_TAG}>\n\n"));
    out
}

pub(super) fn format_agent_instructions(agent: &AgentContext) -> String {
    format!(
        "BROWSER CONTROL MODE (round {} of {}).\n{AGENT_CONTROL_RULES}",
        agent.step + 1,
        agent.max_steps,
    )
}

pub(super) fn claude_system_prompt(agent_mode: bool) -> String {
    if agent_mode {
        format!("{SYSTEM_PROMPT}\n\n{AGENT_CONTROL_RULES}")
    } else {
        SYSTEM_PROMPT.to_string()
    }
}

pub(super) fn append_recent_history(out: &mut String, history: &[ChatMessage], agent_mode: bool) {
    let (max_turns, max_turn_chars) = if agent_mode {
        (2, MAX_TURN_CHARS.min(600))
    } else {
        (MAX_HISTORY_TURNS, MAX_TURN_CHARS)
    };
    let recent: Vec<&ChatMessage> = history
        .iter()
        .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
        .rev()
        .take(max_turns)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    if !recent.is_empty() {
        out.push_str("<conversation_history>\n");
        for msg in recent {
            let role = if msg.role == ChatRole::User {
                "User"
            } else {
                "Assistant"
            };
            let body: String = neutralize_untrusted(&msg.text).chars().take(max_turn_chars).collect();
            out.push_str(role);
            out.push_str(": ");
            out.push_str(&body);
            out.push_str("\n\n");
        }
        out.push_str("</conversation_history>\n\n");
    }
}

pub(super) fn build_claude_turn_prompt(
    history: &[ChatMessage],
    user_text: &str,
    page: Option<&PageContext>,
    agent: Option<&AgentContext>,
    include_history: bool,
) -> String {
    let mut out = String::with_capacity(4096);
    if include_history {
        append_recent_history(&mut out, history, agent.is_some());
    }
    if let Some(ctx) = agent {
        out.push_str(&format!(
            "Round {} of {}. Use ONLY element IDs from the latest snapshot below; ignore older snapshots.\n\n",
            ctx.step + 1,
            ctx.max_steps,
        ));
        out.push_str(&format_agent_state_block(ctx));
    }
    out.push_str(&format_user_turn(user_text, page));
    out
}

pub(super) fn build_cli_prompt(
    history: &[ChatMessage],
    user_text: &str,
    page: Option<&PageContext>,
    agent: Option<&AgentContext>,
    include_history: bool,
) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(SYSTEM_PROMPT);
    if let Some(ctx) = agent {
        out.push_str("\n\n");
        out.push_str(&format_agent_instructions(ctx));
    }
    out.push_str("\n\n");
    if include_history {
        append_recent_history(&mut out, history, agent.is_some());
    }
    if let Some(ctx) = agent {
        out.push_str(&format_agent_state_block(ctx));
    }
    out.push_str(&format_user_turn(user_text, page));
    out
}
