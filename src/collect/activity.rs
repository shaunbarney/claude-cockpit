//! Prompt cadence, derived from session transcripts.
//!
//! A "prompt" is a **human turn in a main session**: a `type: "user"` line in a
//! top-level transcript (not a `subagents/` file, not `isSidechain`) whose
//! content is not a `tool_result`, not `isMeta`, and not harness noise
//! (local-command output, `!` shell input/output, task notifications, system
//! reminders). Slash commands that never reach the model (`/model`, `/effort`,
//! `/mcp`, `/clear`, …) are excluded; skill/custom commands and ones that do
//! call the model (`/compact`, `/goal`, `/plan`) count. This replaces
//! `~/.claude/history.jsonl`, which logs every local slash command and misses
//! SDK/agent sessions entirely.
//!
//! The usage scan (cost thread) extracts prompts while it tails the
//! transcripts and stores them here via [`set_prompt_history`]; the slow
//! refresh thread reads them back with [`read_history`] for the Rate detail.
use std::sync::Mutex;

use serde_json::Value;

/// Slash commands handled entirely by the client (no model call).
const LOCAL_COMMANDS: &[&str] = &[
    "add-dir",
    "agents",
    "allowed-tools",
    "artifacts",
    "btw",
    "bug",
    "chrome",
    "clear",
    "config",
    "context",
    "copy",
    "cost",
    "doctor",
    "effort",
    "exit",
    "export",
    "extra-usage",
    "fast",
    "feedback",
    "help",
    "hooks",
    "ide",
    "install-github-app",
    "keybindings",
    "login",
    "logout",
    "mcp",
    "memory",
    "migrate-installer",
    "model",
    "output-style",
    "permissions",
    "plugin",
    "plugins",
    "privacy-settings",
    "quit",
    "release-notes",
    "reload-plugins",
    "remote-control",
    "rename",
    "resume",
    "rewind",
    "sandbox",
    "skills",
    "status",
    "statusline",
    "stickers",
    "tasks",
    "terminal-setup",
    "theme",
    "todos",
    "upgrade",
    "usage",
    "vim",
];

/// Content prefixes the harness writes as `user` lines that are not typed by a human.
const NON_HUMAN_PREFIXES: &[&str] = &[
    "<local-command-stdout",
    "<local-command-stderr",
    "<local-command-caveat",
    "<task-notification",
    "<bash-input",
    "<bash-stdout",
    "<bash-stderr",
    "<system-reminder",
    "<user-memory-input",
];

/// True for a slash command that is handled locally (no model call).
pub fn is_local_command(name: &str) -> bool {
    LOCAL_COMMANDS.contains(&name.trim().trim_start_matches('/'))
}

/// The text of a user message, or `None` when it carries a tool result.
pub fn user_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            Some(
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(""),
            )
        }
        _ => None,
    }
}

/// The slash command named in `<command-name>/x</command-name>`, if any.
pub fn command_name(text: &str) -> Option<&str> {
    let start = text.find("<command-name>")? + "<command-name>".len();
    let end = start + text[start..].find("</command-name>")?;
    let name = text[start..end].trim().trim_start_matches('/');
    (!name.is_empty()).then_some(name)
}

/// Whether a parsed transcript line is a human prompt (see module docs).
/// `main_session` is false for subagent transcripts.
pub fn is_human_prompt(v: &Value, main_session: bool) -> bool {
    if !main_session
        || v.get("type").and_then(Value::as_str) != Some("user")
        || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        || v.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return false;
    }
    let Some(text) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(user_text)
    else {
        return false;
    };
    let t = text.trim_start();
    if t.is_empty() || NON_HUMAN_PREFIXES.iter().any(|p| t.starts_with(p)) {
        return false;
    }
    !command_name(t).is_some_and(is_local_command)
}

/// Count prompts per local day from millisecond timestamps, ascending by day.
pub fn daily_counts(timestamps_ms: &[i64]) -> Vec<(String, u32)> {
    use std::collections::BTreeMap;
    let mut m: BTreeMap<String, u32> = BTreeMap::new();
    for &ts in timestamps_ms {
        if let Some(day) = crate::collect::usage::local_day(ts) {
            *m.entry(day).or_default() += 1;
        }
    }
    m.into_iter().collect()
}

static PROMPTS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// Publish the latest transcript-derived prompt timestamps (ms).
pub fn set_prompt_history(ts: Vec<i64>) {
    if let Ok(mut p) = PROMPTS.lock() {
        *p = ts;
    }
}

/// Prompt timestamps (ms) from the most recent usage scan (empty until the
/// first scan completes).
pub fn read_history() -> Vec<i64> {
    PROMPTS.lock().map(|p| p.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(date: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&format!("{date}T12:00:00Z"))
            .unwrap()
            .timestamp_millis()
    }

    fn user(content: &str, extra: &str) -> Value {
        serde_json::from_str(&format!(
            r#"{{"type":"user"{extra},"message":{{"role":"user","content":{content}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn buckets_by_local_day() {
        let a = ms("2026-06-22");
        let b = ms("2026-06-23");
        let v = daily_counts(&[a, a, b]);
        let day = |t| crate::collect::usage::local_day(t).unwrap();
        assert_eq!(v, vec![(day(a), 2), (day(b), 1)]);
    }

    #[test]
    fn empty_is_empty() {
        assert!(daily_counts(&[]).is_empty());
    }

    #[test]
    fn human_prompt_classification() {
        assert!(is_human_prompt(&user(r#""fix the bug""#, ""), true));
        assert!(is_human_prompt(
            &user(r#"[{"type":"text","text":"look at this"}]"#, ""),
            true
        ));
        // Subagent transcripts and sidechains are not the user's prompts.
        assert!(!is_human_prompt(&user(r#""fix the bug""#, ""), false));
        assert!(!is_human_prompt(
            &user(r#""x""#, r#","isSidechain":true"#),
            true
        ));
        // Tool results, meta lines and harness noise are not human turns.
        assert!(!is_human_prompt(
            &user(r#"[{"type":"tool_result","content":"ok"}]"#, ""),
            true
        ));
        assert!(!is_human_prompt(
            &user(r#""caveat""#, r#","isMeta":true"#),
            true
        ));
        for noise in [
            "<local-command-stdout>Set model</local-command-stdout>",
            "<task-notification><task-id>1</task-id></task-notification>",
            "<bash-input>ls</bash-input>",
        ] {
            let c = serde_json::to_string(noise).unwrap();
            assert!(!is_human_prompt(&user(&c, ""), true), "{noise}");
        }
        // Local slash commands don't call the model; skills/compact do.
        let cmd = |n: &str| {
            serde_json::to_string(&format!(
                "<command-name>/{n}</command-name>\n<command-message>{n}</command-message>"
            ))
            .unwrap()
        };
        assert!(!is_human_prompt(&user(&cmd("model"), ""), true));
        assert!(!is_human_prompt(&user(&cmd("effort"), ""), true));
        assert!(is_human_prompt(&user(&cmd("compact"), ""), true));
        assert!(is_human_prompt(&user(&cmd("deploy"), ""), true));
    }

    #[test]
    fn extracts_command_names() {
        assert_eq!(
            command_name("<command-name>/goal</command-name>"),
            Some("goal")
        );
        assert_eq!(
            command_name(
                "<command-message>x</command-message><command-name>plug:skill</command-name>"
            ),
            Some("plug:skill")
        );
        assert_eq!(command_name("plain"), None);
    }
}
