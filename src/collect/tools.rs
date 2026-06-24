//! Tool-use frequency from session transcripts (`~/.claude/projects/**/*.jsonl`).
//! Counts `tool_use` blocks per tool name over a trailing window — "where the
//! agent spends its actions" — replacing the old generic process list.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use serde_json::Value;

/// One tool's usage count.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolStat {
    pub name: String,
    pub count: u64,
}

/// Parse deduped `(tool_name, timestamp_ms)` tool-use events from one session.
/// Dedupes by `message.id` (assistant messages are logged repeatedly while
/// streaming, which would otherwise multiply every tool call).
pub fn parse_tool_events(jsonl: &str) -> Vec<(String, i64)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
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
        if id.is_empty() || !seen.insert(id.to_string()) {
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
        for block in content {
            if block.get("type").and_then(|x| x.as_str()) == Some("tool_use") {
                if let Some(name) = block.get("name").and_then(|x| x.as_str()) {
                    out.push((name.to_string(), ts));
                }
            }
        }
    }
    out
}

/// Aggregate `(name, ts)` events into counts within `[now - window, now]`
/// (events with an unknown timestamp of 0 are always included), sorted by
/// count desc then name.
pub fn aggregate(events: &[(String, i64)], now_ms: i64, window_days: i64) -> Vec<ToolStat> {
    let cutoff = now_ms - window_days * 86_400_000;
    let mut counts: HashMap<&str, u64> = HashMap::new();
    for (name, ts) in events {
        if *ts == 0 || *ts >= cutoff {
            *counts.entry(name.as_str()).or_default() += 1;
        }
    }
    let mut v: Vec<ToolStat> = counts
        .into_iter()
        .map(|(name, count)| ToolStat {
            name: name.to_string(),
            count,
        })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    v
}

/// Per-file cache: path -> (mtime, len, parsed events). Re-parse only on change.
pub type ToolCache = HashMap<PathBuf, (SystemTime, u64, Vec<(String, i64)>)>;

/// Scan all transcripts (re-reading only changed files) and return tool counts
/// over the trailing `window_days`.
pub fn scan_tools(cache: &mut ToolCache, now_ms: i64, window_days: i64) -> Vec<ToolStat> {
    let Some(home) = crate::util::claude_home() else {
        return vec![];
    };
    let mut files = Vec::new();
    crate::collect::usage::collect_jsonl(&home.join("projects"), &mut files);
    let mut all: Vec<(String, i64)> = Vec::new();
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
            cache.insert(path.clone(), (mtime, len, parse_tool_events(&txt)));
        }
        if let Some((_, _, ev)) = cache.get(path) {
            all.extend_from_slice(ev);
        }
    }
    aggregate(&all, now_ms, window_days)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, ts: &str, tools: &[&str]) -> String {
        let blocks: Vec<String> = tools
            .iter()
            .map(|t| format!(r#"{{"type":"tool_use","name":"{t}"}}"#))
            .collect();
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","content":[{}]}}}}"#,
            blocks.join(",")
        )
    }

    #[test]
    fn counts_tool_uses_and_dedupes() {
        let jsonl = [
            line("m1", "2026-06-24T10:00:00Z", &["Bash", "Edit"]),
            line("m1", "2026-06-24T10:00:00Z", &["Bash", "Edit"]), // streaming dupe
            line("m2", "2026-06-24T10:01:00Z", &["Bash", "Read"]),
        ]
        .join("\n");
        let ev = parse_tool_events(&jsonl);
        // m1 counted once: Bash, Edit; m2: Bash, Read → Bash×2, Edit×1, Read×1.
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-24T11:00:00Z")
            .unwrap()
            .timestamp_millis();
        let stats = aggregate(&ev, now, 7);
        assert_eq!(
            stats[0],
            ToolStat {
                name: "Bash".into(),
                count: 2
            }
        );
        assert_eq!(stats.iter().find(|s| s.name == "Edit").unwrap().count, 1);
        assert_eq!(stats.iter().find(|s| s.name == "Read").unwrap().count, 1);
    }

    #[test]
    fn window_excludes_old_events() {
        let ev = vec![
            ("Bash".to_string(), 1_000_000_000_000),
            ("Edit".to_string(), 2_000_000_000_000),
        ];
        // now just after the Edit event, 7-day window → Bash (far older) excluded.
        let stats = aggregate(&ev, 2_000_000_000_000, 7);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].name, "Edit");
    }
}
