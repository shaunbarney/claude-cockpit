//! Claude Code token-usage + cost from session transcripts (~/.claude/projects/**/*.jsonl).
//! CRITICAL: assistant messages are logged multiple times while streaming — dedupe by
//! message.id or cost roughly doubles.
use std::collections::HashSet;

use serde_json::Value;

use crate::collect::pricing::PriceTable;

/// One deduplicated assistant message's token usage.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub day: String, // YYYY-MM-DD
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayUsage {
    pub day: String,
    pub cost_usd: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub model: String,
    pub cost_usd: f64,
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub by_day: Vec<DayUsage>,     // ascending by day
    pub by_model: Vec<ModelUsage>, // descending by cost
    pub total_cost_usd: f64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub fresh_input: u64,
}

/// Parse one session's JSONL into deduped per-message UsageRecords.
/// `day_fallback` (YYYY-MM-DD) is used when a line lacks a `timestamp`.
pub fn parse_session(jsonl: &str, day_fallback: &str) -> Vec<UsageRecord> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(|x| x.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message") else { continue };
        let Some(usage) = msg.get("usage") else { continue };
        let id = msg.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue; // dedupe by message.id
        }
        let day = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .and_then(|t| t.split('T').next())
            .unwrap_or(day_fallback)
            .to_string();
        let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        out.push(UsageRecord {
            day,
            model: msg
                .get("model")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cache_read: n("cache_read_input_tokens"),
        });
    }
    out
}

fn record_cost(r: &UsageRecord, t: &PriceTable) -> f64 {
    let p = t.get(&r.model);
    r.input as f64 * p.input
        + r.output as f64 * p.output
        + r.cache_write as f64 * p.cache_write
        + r.cache_read as f64 * p.cache_read
}

/// Aggregate deduped records into totals + per-day + per-model breakdowns.
pub fn totalize(records: &[UsageRecord], prices: &PriceTable) -> UsageTotals {
    use std::collections::HashMap;
    let mut day: HashMap<String, (f64, u64)> = HashMap::new();
    let mut model: HashMap<String, (f64, u64, u64)> = HashMap::new();
    let mut totals = UsageTotals::default();
    for r in records {
        let cost = record_cost(r, prices);
        totals.total_cost_usd += cost;
        totals.cache_read += r.cache_read;
        totals.cache_write += r.cache_write;
        totals.fresh_input += r.input;
        let toks = r.input + r.output + r.cache_write + r.cache_read;
        let d = day.entry(r.day.clone()).or_default();
        d.0 += cost;
        d.1 += toks;
        let m = model.entry(r.model.clone()).or_default();
        m.0 += cost;
        m.1 += r.input;
        m.2 += r.output;
    }
    totals.by_day = day
        .into_iter()
        .map(|(day, (cost_usd, tokens))| DayUsage { day, cost_usd, tokens })
        .collect();
    totals.by_day.sort_by(|a, b| a.day.cmp(&b.day));
    totals.by_model = model
        .into_iter()
        .map(|(model, (cost_usd, input, output))| ModelUsage { model, cost_usd, input, output })
        .collect();
    totals.by_model.sort_by(|a, b| {
        b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal)
    });
    totals
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Per-file cache: path -> (mtime, len, parsed records). Re-parse only on change.
pub type UsageCache = HashMap<PathBuf, (SystemTime, u64, Vec<UsageRecord>)>;

/// Recursively collect `*.jsonl` files under `dir`.
fn collect_jsonl(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_jsonl(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

/// Scan all session transcripts under ~/.claude/projects, re-parsing only changed
/// files (keyed by mtime+len), and return aggregated totals.
pub fn scan_all(cache: &mut UsageCache) -> UsageTotals {
    let prices = crate::collect::pricing::load();
    let Some(home) = crate::util::claude_home() else { return UsageTotals::default() };
    let mut files = Vec::new();
    collect_jsonl(&home.join("projects"), &mut files);
    let mut all: Vec<UsageRecord> = Vec::new();
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
            let recs = parse_session(&txt, "unknown");
            cache.insert(path.clone(), (mtime, len, recs));
        }
        if let Some((_, _, recs)) = cache.get(path) {
            all.extend_from_slice(recs);
        }
    }
    totalize(&all, &prices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::pricing::{ModelPrice, PriceTable};
    use std::collections::HashMap;

    fn asst(id: &str, model: &str, input: u64, output: u64, cw: u64, cr: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_creation_input_tokens":{cw},"cache_read_input_tokens":{cr}}}}}}}"#
        )
    }

    #[test]
    fn dedups_by_message_id_then_sums() {
        let lines = vec![
            asst("m1", "claude-opus-4-8", 100, 10, 0, 0),
            asst("m1", "claude-opus-4-8", 100, 10, 0, 0), // streaming dupe
            asst("m2", "claude-opus-4-8", 200, 20, 0, 0),
        ]
        .join("\n");
        let recs = parse_session(&lines, "2026-06-22");
        let in_tokens: u64 = recs.iter().map(|r| r.input).sum();
        assert_eq!(recs.len(), 2);
        assert_eq!(in_tokens, 300);
    }

    #[test]
    fn costs_with_table() {
        let recs = vec![UsageRecord {
            day: "d".into(),
            model: "claude-x".into(),
            input: 1000,
            output: 100,
            cache_write: 0,
            cache_read: 0,
        }];
        let mut map = HashMap::new();
        map.insert(
            "claude-x".to_string(),
            ModelPrice { input: 1e-6, output: 5e-6, cache_write: 0.0, cache_read: 0.0 },
        );
        let totals = totalize(&recs, &PriceTable(map));
        assert!((totals.total_cost_usd - (1000.0 * 1e-6 + 100.0 * 5e-6)).abs() < 1e-12);
        assert_eq!(totals.fresh_input, 1000);
    }
}
