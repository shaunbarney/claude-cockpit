//! Claude Code skills: discovered from disk (personal, project, and plugin
//! `SKILL.md` files) and ranked by how often they've actually been invoked
//! (parsed from `Skill` tool-use blocks in the session transcripts).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// One skill: its identity, where it came from, and how much it's been used.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub source: String, // "personal" | "project" | "plugin" | "used"
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

fn skill_from_file(path: &Path, source: &str) -> Option<Skill> {
    let md = std::fs::read_to_string(path).ok()?;
    let fallback = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (name, description) = parse_frontmatter(&md, &fallback);
    if name.is_empty() {
        return None;
    }
    Some(Skill {
        name,
        description,
        source: source.to_string(),
        path: Some(path.to_path_buf()),
        uses: 0,
        last_used: None,
    })
}

/// Collect `<dir>/*/SKILL.md` skills with the given source label.
fn scan_skill_dir(dir: &Path, source: &str, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path().join("SKILL.md");
        if p.is_file() {
            if let Some(s) = skill_from_file(&p, source) {
                out.push(s);
            }
        }
    }
}

/// Recursively find plugin `SKILL.md` files under `dir` (bounded depth).
fn scan_plugin_skills(dir: &Path, depth: usize, out: &mut Vec<Skill>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_plugin_skills(&p, depth - 1, out);
        } else if p.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            if let Some(s) = skill_from_file(&p, "plugin") {
                out.push(s);
            }
        }
    }
}

/// Discover skills on disk: personal (`~/.claude/skills`), project
/// (`<root>/.claude/skills`), then plugin (`~/.claude/plugins/**/skills`).
/// Deduped by name, earlier sources winning.
pub fn discover(root: &str) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    if let Some(home) = crate::util::claude_home() {
        scan_skill_dir(&home.join("skills"), "personal", &mut out);
        scan_plugin_skills(&home.join("plugins"), 10, &mut out);
    }
    scan_skill_dir(&Path::new(root).join(".claude/skills"), "project", &mut out);

    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.name.clone()));
    out
}

/// Parse deduped `(skill_name, ts_ms)` invocations from one session — `Skill`
/// tool-use blocks, read via `input.skill`.
///
/// An assistant turn is logged repeatedly while streaming, and the *first* copy
/// often predates the `tool_use` block — so we key by `message.id` and keep the
/// **last non-empty** occurrence (the finalized turn). Keying on the first copy
/// (as a naive dedupe would) misses the call entirely.
pub fn parse_skill_uses(jsonl: &str) -> Vec<(String, i64)> {
    let mut by_id: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|x| x.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        let id = msg.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let ts = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
        let Some(content) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        let mut skills = Vec::new();
        for block in content {
            if block.get("type").and_then(|x| x.as_str()) == Some("tool_use")
                && block.get("name").and_then(|x| x.as_str()) == Some("Skill")
            {
                if let Some(skill) = block
                    .get("input")
                    .and_then(|inp| inp.get("skill"))
                    .and_then(|x| x.as_str())
                {
                    skills.push((skill.to_string(), ts));
                }
            }
        }
        // Overwrite only when this copy has the block, so a trailing partial
        // can't erase the finalized one.
        if !skills.is_empty() {
            by_id.insert(id.to_string(), skills);
        }
    }
    by_id.into_values().flatten().collect()
}

/// Per-file cache: path -> (mtime, len, parsed skill-use events).
pub type SkillCache = HashMap<PathBuf, (SystemTime, u64, Vec<(String, i64)>)>;

/// Discover skills and merge in invocation counts from the transcripts.
/// Sorted by uses desc, then name. Skills invoked but not found on disk are
/// appended with source `"used"`.
pub fn scan_skills(cache: &mut SkillCache, root: &str) -> Vec<Skill> {
    let mut skills = discover(root);

    // Tally usage across transcripts (re-reading only changed files).
    let mut counts: HashMap<String, (u64, i64)> = HashMap::new();
    if let Some(home) = crate::util::claude_home() {
        let mut files = Vec::new();
        crate::collect::usage::collect_jsonl(&home.join("projects"), &mut files);
        for path in &files {
            let (mtime, len) = std::fs::metadata(path)
                .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
                .unwrap_or((SystemTime::UNIX_EPOCH, 0));
            let fresh = match cache.get(path) {
                Some((mt, l, _)) => *mt != mtime || *l != len,
                None => true,
            };
            if fresh {
                let txt = std::fs::read_to_string(path).unwrap_or_default();
                cache.insert(path.clone(), (mtime, len, parse_skill_uses(&txt)));
            }
            if let Some((_, _, ev)) = cache.get(path) {
                for (name, ts) in ev {
                    let e = counts.entry(name.clone()).or_insert((0, 0));
                    e.0 += 1;
                    e.1 = e.1.max(*ts);
                }
            }
        }
    }

    for s in &mut skills {
        if let Some((uses, last)) = counts.remove(&s.name) {
            s.uses = uses;
            s.last_used = if last > 0 { Some(last) } else { None };
        }
    }
    // Invoked but not discovered on disk (e.g. a skill from another project).
    for (name, (uses, last)) in counts {
        skills.push(Skill {
            name,
            description: String::new(),
            source: "used".into(),
            path: None,
            uses,
            last_used: if last > 0 { Some(last) } else { None },
        });
    }

    // Keep the user's own skills (personal/project) always; show plugin skills
    // only once actually used — the marketplaces ship dozens that would
    // otherwise bury everything in zero-use noise.
    skills.retain(|s| s.uses > 0 || s.source == "personal" || s.source == "project");
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
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","name":"Skill","input":{"skill":"deploy"}}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","name":"Skill","input":{"skill":"deploy"}}]}}"#, // dupe
            r#"{"type":"assistant","timestamp":"2026-06-24T10:01:00Z","message":{"id":"m2","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
        ]
        .join("\n");
        let ev = parse_skill_uses(&jsonl);
        // Deduped by message.id → one "deploy"; the Bash call is ignored.
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "deploy");
    }
}
