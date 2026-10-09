//! Tool-use frequency from session transcripts (`~/.claude/projects/**/*.jsonl`).
//! Counts `tool_use` blocks per tool name over a trailing window — "where the
//! agent spends its actions" — replacing the old generic process list.

use std::collections::HashMap;

use serde_json::Value;

use crate::collect::usage::{parse_ts, transcript_files, TailCache};

/// One tool's usage count.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolStat {
    pub name: String,
    pub count: u64,
}

/// tool_use id -> (tool name, timestamp ms; 0 when unknown).
type ToolEvents = HashMap<String, (String, i64)>;

/// Fold `tool_use` blocks from newly appended lines into `out`.
///
/// Every line is scanned: an assistant turn is logged once per content block
/// while streaming, so a turn's `tool_use` blocks usually arrive on *later*
/// lines than its first copy — deduping by `message.id` and keeping the first
/// line (as before) missed ~70% of calls. Calls are deduped by the block's own
/// `tool_use.id` instead, which also collapses repeated copies of a line.
fn parse_into(out: &mut ToolEvents, jsonl: &str) {
    for line in jsonl.lines() {
        if !line.contains("\"tool_use\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let ts = parse_ts(&v).unwrap_or(0);
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let (Some(id), Some(name)) = (
                block.get("id").and_then(Value::as_str),
                block.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            out.entry(id.to_string())
                .or_insert_with(|| (name.to_string(), ts));
        }
    }
}

/// Parse deduped `(tool_name, timestamp_ms)` tool-use events from one session.
pub fn parse_tool_events(jsonl: &str) -> Vec<(String, i64)> {
    let mut m = ToolEvents::new();
    parse_into(&mut m, jsonl);
    m.into_values().collect()
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

/// Per-file incremental cache of parsed tool-use events.
#[derive(Debug, Default)]
pub struct ToolCache {
    files: TailCache<ToolEvents>,
}

impl ToolCache {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Scan all transcripts (reading only appended lines) and return tool counts
/// over the trailing `window_days`. Deduped by `tool_use.id` across files, so
/// a resumed session's copied history isn't counted twice.
pub fn scan_tools(cache: &mut ToolCache, now_ms: i64, window_days: i64) -> Vec<ToolStat> {
    cache
        .files
        .update(&transcript_files(), |_, st, txt| parse_into(st, txt));
    let mut all: ToolEvents = HashMap::new();
    for st in cache.files.states() {
        for (id, ev) in st {
            all.entry(id.clone()).or_insert_with(|| ev.clone());
        }
    }
    let events: Vec<(String, i64)> = all.into_values().collect();
    aggregate(&events, now_ms, window_days)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One streamed assistant line carrying `tools` as `(tool_use_id, name)`.
    fn line(msg_id: &str, ts: &str, tools: &[(&str, &str)]) -> String {
        let blocks: Vec<String> = tools
            .iter()
            .map(|(id, t)| format!(r#"{{"type":"tool_use","id":"{id}","name":"{t}"}}"#))
            .collect();
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{msg_id}","content":[{}]}}}}"#,
            blocks.join(",")
        )
    }

    fn now() -> i64 {
        chrono::DateTime::parse_from_rfc3339("2026-06-24T11:00:00Z")
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn counts_tool_uses_and_dedupes() {
        let jsonl = [
            line(
                "m1",
                "2026-06-24T10:00:00Z",
                &[("t1", "Bash"), ("t2", "Edit")],
            ),
            line(
                "m1",
                "2026-06-24T10:00:00Z",
                &[("t1", "Bash"), ("t2", "Edit")],
            ), // dupe
            line(
                "m2",
                "2026-06-24T10:01:00Z",
                &[("t3", "Bash"), ("t4", "Read")],
            ),
        ]
        .join("\n");
        let stats = aggregate(&parse_tool_events(&jsonl), now(), 7);
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
    fn counts_tool_uses_on_later_streamed_lines() {
        // Real streaming shape: the first copy of m1 is text only; each tool_use
        // block arrives on its own later line with the same message.id.
        let jsonl = [
            r#"{"type":"assistant","timestamp":"2026-06-24T10:00:00Z","message":{"id":"m1","content":[{"type":"text","text":"Let me look."}]}}"#.to_string(),
            line("m1", "2026-06-24T10:00:01Z", &[("t1", "Read")]),
            line("m1", "2026-06-24T10:00:02Z", &[("t2", "Bash")]),
        ]
        .join("\n");
        let stats = aggregate(&parse_tool_events(&jsonl), now(), 7);
        let total: u64 = stats.iter().map(|s| s.count).sum();
        assert_eq!(total, 2, "{stats:?}");
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
