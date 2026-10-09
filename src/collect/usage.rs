//! Claude Code token-usage + cost from session transcripts (~/.claude/projects/**/*.jsonl).
//!
//! CRITICAL: assistant messages are logged multiple times while streaming, and
//! the *early* copies carry partial usage (e.g. `output_tokens` 5, 5, 5, 337).
//! Records are deduped by `message.id` keeping the **per-field maximum** — the
//! finalized usage — or cost is wildly off (doubled with no dedupe, ~44% low on
//! output when keeping the first copy). Dedupe is **global across files**:
//! resumed/forked sessions copy earlier messages into a new transcript.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::collect::pricing::PriceTable;

mod rate;
mod tail;

pub use rate::{rate_stats, RateStats};

/// Model name Claude Code writes for client-synthesized messages (no API call).
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// One deduplicated assistant message's token usage.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageRecord {
    pub day: String,        // YYYY-MM-DD (local time)
    pub ts_ms: Option<i64>, // event time (epoch ms) for sub-day trend bucketing
    pub model: String,
    pub input: u64,
    pub output: u64,
    /// All cache-write tokens (5-minute + 1-hour).
    pub cache_write: u64,
    /// The 1-hour ephemeral share of `cache_write` (priced at 2x input, not 1.25x).
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl UsageRecord {
    /// Fold another copy of the same message in: token fields take the max (the
    /// finalized streaming copy), time/day take the earliest.
    fn merge(&mut self, o: &UsageRecord) {
        self.input = self.input.max(o.input);
        self.output = self.output.max(o.output);
        self.cache_write = self.cache_write.max(o.cache_write);
        self.cache_write_1h = self.cache_write_1h.max(o.cache_write_1h);
        self.cache_read = self.cache_read.max(o.cache_read);
        if o.ts_ms.is_some() && (self.ts_ms.is_none() || o.ts_ms < self.ts_ms) {
            self.ts_ms = o.ts_ms;
            self.day = o.day.clone();
        }
    }
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
    pub cache_write: u64,
    pub cache_read: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub by_day: Vec<DayUsage>,                        // ascending by day
    pub by_model: Vec<ModelUsage>,                    // descending by cost
    pub by_model_day: HashMap<String, Vec<DayUsage>>, // model -> ascending days
    pub cost_trend: crate::trend::Trend, // adaptive spend curve (hourly today, daily over weeks)
    pub rate: RateStats,                 // rolling-window rate-limit proximity
    pub total_cost_usd: f64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub fresh_input: u64,
    /// Earliest local day with usage — the start of the period `total_cost_usd`
    /// covers (transcripts are pruned, so this is not "all time").
    pub since_day: Option<String>,
    /// Models with token usage but no known price (counted as $0 — flag them).
    pub unpriced: Vec<String>,
}

impl UsageTotals {
    /// Whole days covered from `since_day` through `today` (inclusive), if known.
    pub fn period_days(&self, today: &str) -> Option<i64> {
        let parse = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
        let from = parse(self.since_day.as_deref()?)?;
        let to = parse(today)?;
        Some((to - from).num_days().max(0) + 1)
    }
}

/// Local calendar day (`YYYY-MM-DD`) of an epoch-ms timestamp.
pub fn local_day(ts_ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ts_ms).map(|dt| {
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    })
}

/// Today's local calendar day (`YYYY-MM-DD`) — the key `by_day` uses. Any
/// "today" lookup into usage data should use this, not a UTC date.
pub fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Epoch ms of an RFC 3339 transcript `timestamp`.
pub(crate) fn parse_ts(v: &Value) -> Option<i64> {
    v.get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| dt.timestamp_millis())
}

/// True for subagent transcripts (`<session>/subagents/agent-*.jsonl`).
pub(crate) fn is_subagent(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "subagents")
}

/// Accumulated parse state for one transcript file.
#[derive(Debug, Default)]
pub struct UsageFile {
    /// message.id -> merged usage.
    msgs: HashMap<String, UsageRecord>,
    /// user-line uuid -> timestamp, for human prompts (see `collect::activity`).
    prompts: HashMap<String, i64>,
}

/// One assistant line's usage, or `None` if it carries none / is synthetic.
fn record_from(v: &Value) -> Option<(String, UsageRecord)> {
    let msg = v.get("message")?;
    let usage = msg.get("usage")?;
    let id = msg
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let model = msg
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if model == SYNTHETIC_MODEL {
        return None;
    }
    let n = |o: &Value, k: &str| o.get(k).and_then(Value::as_u64).unwrap_or(0);
    let split = usage.get("cache_creation");
    let cw_1h = split.map_or(0, |c| n(c, "ephemeral_1h_input_tokens"));
    let cw_5m = split.map_or(0, |c| n(c, "ephemeral_5m_input_tokens"));
    let ts_ms = parse_ts(v);
    let rec = UsageRecord {
        day: ts_ms
            .and_then(local_day)
            .unwrap_or_else(|| "unknown".into()),
        ts_ms,
        model: model.to_string(),
        input: n(usage, "input_tokens"),
        output: n(usage, "output_tokens"),
        cache_write: n(usage, "cache_creation_input_tokens").max(cw_1h + cw_5m),
        cache_write_1h: cw_1h,
        cache_read: n(usage, "cache_read_input_tokens"),
    };
    Some((id.to_string(), rec))
}

/// Fold newly appended transcript lines into one file's state.
fn parse_lines(state: &mut UsageFile, text: &str, main_session: bool) {
    for line in text.lines() {
        // Cheap pre-filter: only assistant and user lines matter here.
        if !line.contains("\"assistant\"") && !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some((id, rec)) = record_from(&v) {
                    match state.msgs.get_mut(&id) {
                        Some(r) => r.merge(&rec),
                        None => {
                            state.msgs.insert(id, rec);
                        }
                    }
                }
            }
            Some("user") if crate::collect::activity::is_human_prompt(&v, main_session) => {
                if let (Some(uuid), Some(ts)) =
                    (v.get("uuid").and_then(Value::as_str), parse_ts(&v))
                {
                    state.prompts.insert(uuid.to_string(), ts);
                }
            }
            _ => {}
        }
    }
}

/// Parse one session's JSONL into deduped per-message UsageRecords.
pub fn parse_session(jsonl: &str) -> Vec<UsageRecord> {
    let mut st = UsageFile::default();
    parse_lines(&mut st, jsonl, true);
    st.msgs.into_values().collect()
}

/// Merge per-file states into globally deduped records + prompt timestamps.
fn merge_files<'a>(files: impl Iterator<Item = &'a UsageFile>) -> (Vec<UsageRecord>, Vec<i64>) {
    let mut msgs: HashMap<&str, UsageRecord> = HashMap::new();
    let mut prompts: HashMap<&str, i64> = HashMap::new();
    for f in files {
        for (id, rec) in &f.msgs {
            match msgs.get_mut(id.as_str()) {
                Some(r) => r.merge(rec),
                None => {
                    msgs.insert(id, rec.clone());
                }
            }
        }
        for (uuid, ts) in &f.prompts {
            let e = prompts.entry(uuid).or_insert(*ts);
            *e = (*e).min(*ts);
        }
    }
    let mut ts: Vec<i64> = prompts.into_values().collect();
    ts.sort_unstable();
    (msgs.into_values().collect(), ts)
}

fn record_cost(r: &UsageRecord, p: &crate::collect::pricing::ModelPrice) -> f64 {
    let cw_1h = r.cache_write_1h.min(r.cache_write);
    r.input as f64 * p.input
        + r.output as f64 * p.output
        + (r.cache_write - cw_1h) as f64 * p.cache_write
        + cw_1h as f64 * p.cache_write_1h
        + r.cache_read as f64 * p.cache_read
}

fn day_list(days: HashMap<String, (f64, u64)>) -> Vec<DayUsage> {
    let mut v: Vec<DayUsage> = days
        .into_iter()
        .map(|(day, (cost_usd, tokens))| DayUsage {
            day,
            cost_usd,
            tokens,
        })
        .collect();
    v.sort_by(|a, b| a.day.cmp(&b.day));
    v
}

/// Aggregate deduped records into totals + per-day + per-model breakdowns.
/// `now_ms` bounds the spend trend so idle time shows as zeros.
pub fn totalize(records: &[UsageRecord], prices: &PriceTable, now_ms: i64) -> UsageTotals {
    let mut day: HashMap<String, (f64, u64)> = HashMap::new();
    let mut model: HashMap<String, ModelUsage> = HashMap::new();
    let mut model_day: HashMap<String, HashMap<String, (f64, u64)>> = HashMap::new();
    let mut unpriced = std::collections::BTreeSet::new();
    let mut cost_events: Vec<(i64, f64)> = Vec::new();
    let mut totals = UsageTotals::default();
    for r in records {
        let toks = r.input + r.output + r.cache_write + r.cache_read;
        let price = prices.resolve(&r.model);
        if price.is_none() && toks > 0 {
            unpriced.insert(r.model.clone());
        }
        let cost = price.map_or(0.0, |(p, _)| record_cost(r, &p));
        if let Some(ts) = r.ts_ms {
            cost_events.push((ts, cost));
        }
        totals.total_cost_usd += cost;
        totals.cache_read += r.cache_read;
        totals.cache_write += r.cache_write;
        totals.fresh_input += r.input;
        let d = day.entry(r.day.clone()).or_default();
        d.0 += cost;
        d.1 += toks;
        let m = model.entry(r.model.clone()).or_insert_with(|| ModelUsage {
            model: r.model.clone(),
            cost_usd: 0.0,
            input: 0,
            output: 0,
            cache_write: 0,
            cache_read: 0,
        });
        m.cost_usd += cost;
        m.input += r.input;
        m.output += r.output;
        m.cache_write += r.cache_write;
        m.cache_read += r.cache_read;
        let md = model_day
            .entry(r.model.clone())
            .or_default()
            .entry(r.day.clone())
            .or_default();
        md.0 += cost;
        md.1 += toks;
    }
    totals.since_day = day.keys().filter(|d| *d != "unknown").min().cloned();
    totals.by_day = day_list(day);
    totals.by_model = model.into_values().collect();
    totals.by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    totals.by_model_day = model_day
        .into_iter()
        .map(|(model, days)| (model, day_list(days)))
        .collect();
    totals.unpriced = unpriced.into_iter().collect();
    // Adaptive spend curve: hourly when all usage is from ~one day, daily over weeks.
    totals.cost_trend = crate::trend::bucketize(&cost_events, 30, now_ms);
    totals
}

/// Per-file incremental cache: each transcript is read from its last byte
/// offset, so a steady-state scan only parses newly appended lines.
#[derive(Debug, Default)]
pub struct UsageCache {
    files: tail::TailCache<UsageFile>,
}

impl UsageCache {
    pub fn new() -> Self {
        Self::default()
    }
}

pub(crate) use tail::TailCache;

/// Recursively collect `*.jsonl` files under `dir`.
pub(crate) fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_jsonl(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

/// All session transcripts under `~/.claude/projects` (empty if unavailable).
pub(crate) fn transcript_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = crate::util::claude_home() {
        collect_jsonl(&home.join("projects"), &mut files);
    }
    files
}

/// Scan all session transcripts under ~/.claude/projects (incrementally) and
/// return aggregated totals. Also publishes the transcript-derived prompt
/// history for the Rate detail (`collect::activity::read_history`).
pub fn scan_all(cache: &mut UsageCache) -> UsageTotals {
    let prices = crate::collect::pricing::load();
    let files = transcript_files();
    cache.files.update(&files, |path, st, txt| {
        parse_lines(st, txt, !is_subagent(path))
    });
    let (records, prompt_ts) = merge_files(cache.files.states());
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut totals = totalize(&records, &prices, now_ms);
    totals.rate = rate_stats(
        &records,
        &prompt_ts,
        now_ms,
        &crate::config::load().rate_limit,
    );
    crate::collect::activity::set_prompt_history(prompt_ts);
    totals
}

#[cfg(test)]
mod tests;
