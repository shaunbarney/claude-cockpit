//! Claude Code skills: discovered from disk (personal, project, and plugin
//! `SKILL.md` files, plus custom slash commands) and ranked by how often
//! they've actually been invoked — `Skill` tool-use blocks and typed
//! `/name` commands in the session transcripts.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::collect::usage::{parse_ts, transcript_files, TailCache};

mod discover;
pub use discover::discover;

/// One skill: its identity, where it came from, and how much it's been used.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub source: String, // "personal" | "project" | "command" | "plugin" | "used"
    pub path: Option<PathBuf>,
    pub uses: u64,
    pub last_used: Option<i64>,
}

/// Pull `name` and `description` out of a `SKILL.md` YAML frontmatter block.
/// Handles inline (`description: foo`) and folded (`description: >-` + indented
/// continuation lines) forms. Falls back to `fallback_name` and empty desc.
pub fn parse_frontmatter(md: &str, fallback_name: &str) -> (String, String) {
    let mut lines = md.lines();
    // Frontmatter must open with `---`.
    if lines.next().map(str::trim) != Some("---") {
        return (fallback_name.to_string(), String::new());
    }
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let pending = lines.collect::<Vec<_>>();
    let mut i = 0;
    while i < pending.len() {
        let line = pending[i];
        if line.trim() == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches(['"', '\'']).to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            let inline = v.trim().trim_start_matches(['>', '|', '-', '+']).trim();
            let mut parts: Vec<String> = Vec::new();
            if !inline.is_empty() {
                parts.push(inline.trim_matches(['"', '\'']).to_string());
            }
            // Gather indented continuation lines (folded/block scalar).
            while i + 1 < pending.len() {
                let next = pending[i + 1];
                if next.trim() == "---" {
                    break;
                }
                let indented = next.starts_with(' ') || next.starts_with('\t');
                if indented && !next.trim().is_empty() {
                    parts.push(next.trim().to_string());
                    i += 1;
                } else if next.trim().is_empty() {
                    i += 1; // skip blank lines inside the block
                } else {
                    break; // a new top-level key
                }
            }
            description = parts.join(" ");
        }
        i += 1;
    }
    (name, description)
}

/// Invocations seen in one transcript, keyed for dedupe.
#[derive(Debug, Default)]
pub struct SkillEvents {
    /// `Skill` tool_use id -> (skill name, ts ms).
    tool: HashMap<String, (String, i64)>,
    /// user-line uuid -> (typed `/command` name, ts ms).
    typed: HashMap<String, (String, i64)>,
}

/// `Skill` tool_use blocks on an assistant line, as `(dedupe key, skill)`.
/// Every streamed line is scanned (a turn's tool_use blocks arrive on later
/// copies than its first), and calls are keyed by `tool_use.id` so parallel
/// Skill calls in one turn are each counted once.
fn skill_tool_uses(v: &Value) -> Vec<(String, String)> {
    let msg = v.get("message");
    let msg_id = msg
        .and_then(|m| m.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let blocks = msg.and_then(|m| m.get("content")).and_then(Value::as_array);
    let mut out = Vec::new();
    for (i, b) in blocks.into_iter().flatten().enumerate() {
        if b.get("type").and_then(Value::as_str) != Some("tool_use")
            || b.get("name").and_then(Value::as_str) != Some("Skill")
        {
            continue;
        }
        let Some(skill) = b
            .get("input")
            .and_then(|inp| inp.get("skill"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let key = b
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{msg_id}#{i}"));
        out.push((key, skill.trim_start_matches('/').to_string()));
    }
    out
}

/// Fold newly appended transcript lines into one file's invocation state.
fn parse_into(st: &mut SkillEvents, jsonl: &str) {
    for line in jsonl.lines() {
        let is_tool = line.contains("\"Skill\"");
        let is_cmd = line.contains("<command-name>");
        if !is_tool && !is_cmd {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ts = parse_ts(&v).unwrap_or(0);
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") if is_tool => {
                for (key, skill) in skill_tool_uses(&v) {
                    st.tool.entry(key).or_insert((skill, ts));
                }
            }
            Some("user") if is_cmd => {
                let text = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(crate::collect::activity::user_text);
                let name = text
                    .as_deref()
                    .and_then(crate::collect::activity::command_name)
                    .filter(|n| !crate::collect::activity::is_local_command(n))
                    .map(str::to_string);
                if let (Some(uuid), Some(name)) = (v.get("uuid").and_then(Value::as_str), name) {
                    st.typed.entry(uuid.to_string()).or_insert((name, ts));
                }
            }
            _ => {}
        }
    }
}

/// Parse deduped `(skill_name, ts_ms)` `Skill`-tool invocations from one session.
pub fn parse_skill_uses(jsonl: &str) -> Vec<(String, i64)> {
    let mut st = SkillEvents::default();
    parse_into(&mut st, jsonl);
    st.tool.into_values().collect()
}

/// Per-file incremental cache of parsed skill invocations.
#[derive(Debug, Default)]
pub struct SkillCache {
    files: TailCache<SkillEvents>,
}

impl SkillCache {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Map an invoked name onto a discovered skill's key. Exact match first; a
/// namespaced `ns:name` with no exact match falls back to a non-plugin skill
/// called `name` (e.g. synced skills invoked as `anthropic-skills:pdf`).
fn resolve<'a>(name: &'a str, skills: &'a [Skill]) -> Option<&'a str> {
    if let Some(s) = skills.iter().find(|s| s.name == name) {
        return Some(&s.name);
    }
    let (_, bare) = name.split_once(':')?;
    skills
        .iter()
        .find(|s| s.source != "plugin" && s.name == bare)
        .map(|s| s.name.as_str())
}

/// Tally `(uses, last_ts)` per skill key from deduped tool + typed events.
/// Typed `/x` commands only count when `x` is a known skill/command (on disk
/// or seen via the Skill tool), so built-ins like `/goal` don't appear.
fn tally(
    tool: &HashMap<String, (String, i64)>,
    typed: &HashMap<String, (String, i64)>,
    skills: &[Skill],
) -> HashMap<String, (u64, i64)> {
    let mut counts: HashMap<String, (u64, i64)> = HashMap::new();
    let mut bump = |key: String, ts: i64| {
        let e = counts.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 = e.1.max(ts);
    };
    let tool_names: std::collections::HashSet<&str> =
        tool.values().map(|(n, _)| n.as_str()).collect();
    for (name, ts) in tool.values() {
        bump(resolve(name, skills).unwrap_or(name).to_string(), *ts);
    }
    for (name, ts) in typed.values() {
        if let Some(key) = resolve(name, skills) {
            bump(key.to_string(), *ts);
        } else if tool_names.contains(name.as_str()) {
            bump(name.clone(), *ts);
        }
    }
    counts
}

/// Discover skills and merge in invocation counts from the transcripts.
/// Sorted by uses desc, then name. Skills invoked but not found on disk
/// (built-in/bundled, or from another project) are appended with source `"used"`.
pub fn scan_skills(cache: &mut SkillCache, root: &str) -> Vec<Skill> {
    let mut skills = discover(root);

    cache
        .files
        .update(&transcript_files(), |_, st, txt| parse_into(st, txt));
    // Dedupe globally: resumed sessions copy earlier lines into new files.
    let mut tool: HashMap<String, (String, i64)> = HashMap::new();
    let mut typed: HashMap<String, (String, i64)> = HashMap::new();
    for st in cache.files.states() {
        for (k, v) in &st.tool {
            tool.entry(k.clone()).or_insert_with(|| v.clone());
        }
        for (k, v) in &st.typed {
            typed.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut counts = tally(&tool, &typed, &skills);

    for s in &mut skills {
        if let Some((uses, last)) = counts.remove(&s.name) {
            s.uses = uses;
            s.last_used = (last > 0).then_some(last);
        }
    }
    for (name, (uses, last)) in counts {
        skills.push(Skill {
            name,
            description: String::new(),
            source: "used".into(),
            path: None,
            uses,
            last_used: (last > 0).then_some(last),
        });
    }

    // Keep the user's own skills/commands always; show plugin skills only once
    // actually used — marketplaces ship dozens that would bury everything.
    skills
        .retain(|s| s.uses > 0 || matches!(s.source.as_str(), "personal" | "project" | "command"));
    skills.sort_by(|a, b| b.uses.cmp(&a.uses).then_with(|| a.name.cmp(&b.name)));
    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_and_folded_frontmatter() {
        let inline =
            "---\nname: deploy\ndescription: Run the CI gate then push.\nother: x\n---\nbody";
        assert_eq!(
            parse_frontmatter(inline, "fallback"),
            ("deploy".into(), "Run the CI gate then push.".into())
        );

        let folded = "---\nname: understand\ndescription: >-\n  Build a deep model\n  of the codebase.\nallowed-tools: x\n---\n";
        let (n, d) = parse_frontmatter(folded, "fallback");
        assert_eq!(n, "understand");
        assert_eq!(d, "Build a deep model of the codebase.");
    }

    #[test]
    fn no_frontmatter_uses_fallback() {
        assert_eq!(
            parse_frontmatter("# just markdown\n", "myskill"),
            ("myskill".into(), String::new())
        );
    }

    #[test]
    fn parses_skill_invocations() {
        let jsonl = [
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"text","text":"loading"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"deploy"}}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"deploy"}}]}}"#, // dupe
            r#"{"type":"assistant","timestamp":"2026-06-24T10:01:00Z","message":{"id":"m2","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"ls"}}]}}"#,
        ]
        .join("\n");
        let ev = parse_skill_uses(&jsonl);
        // Deduped by tool_use.id → one "deploy"; the Bash call is ignored.
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "deploy");
    }

    #[test]
    fn parallel_skill_calls_each_count() {
        let line = r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"a","name":"Skill","input":{"skill":"x"}},{"type":"tool_use","id":"b","name":"Skill","input":{"skill":"x"}}]}}"#;
        assert_eq!(parse_skill_uses(line).len(), 2);
    }

    fn skill(name: &str, source: &str) -> Skill {
        Skill {
            name: name.into(),
            description: String::new(),
            source: source.into(),
            path: None,
            uses: 0,
            last_used: None,
        }
    }

    #[test]
    fn tallies_namespaced_and_typed_invocations() {
        let jsonl = [
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"toolkit:review"}}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m2","content":[{"type":"tool_use","id":"t2","name":"Skill","input":{"skill":"anthropic-skills:pdf"}}]}}"#,
            r#"{"type":"user","uuid":"u1","timestamp":"2026-06-24T11:00:00Z","message":{"role":"user","content":"<command-name>/deploy</command-name>"}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2026-06-24T11:00:00Z","message":{"role":"user","content":"<command-name>/goal</command-name>"}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2026-06-24T11:00:00Z","message":{"role":"user","content":"<command-name>/model</command-name>"}}"#,
        ]
        .join("\n");
        let mut st = SkillEvents::default();
        parse_into(&mut st, &jsonl);
        let skills = vec![
            skill("deploy", "personal"),
            skill("pdf", "personal"),
            skill("toolkit:review", "plugin"),
        ];
        let c = tally(&st.tool, &st.typed, &skills);
        assert_eq!(c.get("toolkit:review").map(|x| x.0), Some(1));
        assert_eq!(c.get("pdf").map(|x| x.0), Some(1)); // ns stripped onto personal
        assert_eq!(c.get("deploy").map(|x| x.0), Some(1)); // typed slash command
        assert!(!c.contains_key("goal") && !c.contains_key("model"));
    }
}
