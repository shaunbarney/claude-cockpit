//! Claude Code token-usage + cost from session transcripts (~/.claude/projects/**/*.jsonl).
//! CRITICAL: assistant messages are logged multiple times while streaming — dedupe by
//! message.id or cost roughly doubles.
use std::collections::HashSet;

use serde_json::Value;

use crate::collect::pricing::PriceTable;

/// One deduplicated assistant message's token usage.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub day: String,        // YYYY-MM-DD
    pub ts_ms: Option<i64>, // event time (epoch ms) for sub-day trend bucketing
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
    pub cache_write: u64,
    pub cache_read: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub by_day: Vec<DayUsage>,     // ascending by day
    pub by_model: Vec<ModelUsage>, // descending by cost
    pub by_model_day: std::collections::HashMap<String, Vec<DayUsage>>, // model -> ascending days
    pub cost_trend: crate::trend::Trend, // adaptive spend curve (hourly today, daily over weeks)
    pub rate: RateStats,           // rolling-window rate-limit proximity
    pub total_cost_usd: f64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub fresh_input: u64,
}

/// Rate-limit proximity for the Rate widget.
///
/// Built to match how the limits are actually defined (per Anthropic docs):
/// the Claude Code subscription limit is **prompts in a rolling 5-hour
/// window** (so `prompts_5h` vs a plan cap), while the API limit that bites
/// first is **OTPM** — output tokens per minute. Token volume is informational
/// only. Each cap notes whether it was configured or auto-scaled to your own
/// busiest observed window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateStats {
    // Subscription 5-hour rolling window (limit unit = prompts).
    pub prompts_5h: u64,
    pub cap_prompts_5h: u64,
    pub auto_prompts_5h: bool, // cap auto-scaled (no plan/prompts_5h in config)
    pub plan_label: String,    // "Pro" / "Max 5x" / "Max 20x" / "" if unknown
    pub resets_in_secs: Option<u64>, // until the oldest in-window prompt ages out
    // API per-minute output-token burn (OTPM — the tightest API limit).
    pub output_1m: u64,
    pub cap_out_1m: u64,
    pub auto_out_1m: bool,
    // Informational token volume in the rolling 5h window (includes cache).
    pub tokens_5h: u64,
    pub burn_trend: crate::trend::Trend, // output tokens per adaptive bucket (sparkline)
}

const WINDOW_5H_MS: i64 = 5 * 3_600_000;
const WINDOW_1M_MS: i64 = 60_000;

/// Sum of `val` for events whose timestamp is within `[now - window, now]`.
fn current_window_sum(ev: &[(i64, u64, u64)], now_ms: i64, window_ms: i64, output: bool) -> u64 {
    ev.iter()
        .filter(|(t, _, _)| *t >= now_ms - window_ms && *t <= now_ms)
        .map(|(_, tot, out)| if output { *out } else { *tot })
        .sum()
}

/// Largest sum of `val` over any time window of width `window_ms` across the
/// (ascending-by-ts) event list — i.e. the busiest such window ever observed.
fn max_window_sum(ev: &[(i64, u64, u64)], window_ms: i64, output: bool) -> u64 {
    let val = |e: &(i64, u64, u64)| if output { e.2 } else { e.1 };
    let mut max = 0u64;
    let mut sum = 0u64;
    let mut i = 0usize;
    for j in 0..ev.len() {
        sum += val(&ev[j]);
        while ev[j].0 - ev[i].0 >= window_ms {
            sum -= val(&ev[i]);
            i += 1;
        }
        max = max.max(sum);
    }
    max
}

/// Count of `prompt_ts` within `[now - window, now]`.
fn count_in_window(prompt_ts: &[i64], now_ms: i64, window_ms: i64) -> u64 {
    prompt_ts
        .iter()
        .filter(|t| **t >= now_ms - window_ms && **t <= now_ms)
        .count() as u64
}

/// Compute rate stats as of `now_ms`. `prompt_ts` are user-prompt timestamps
/// (from `history.jsonl`) — the unit the subscription 5h limit is measured in;
/// `records` carry token usage for the OTPM gauge and token volume.
pub fn rate_stats(
    records: &[UsageRecord],
    prompt_ts: &[i64],
    now_ms: i64,
    cfg: &crate::config::RateLimit,
) -> RateStats {
    // --- token events (for OTPM + 5h volume + sparkline) ---
    let mut ev: Vec<(i64, u64, u64)> = records
        .iter()
        .filter_map(|r| {
            r.ts_ms.map(|t| {
                (
                    t,
                    r.input + r.output + r.cache_write + r.cache_read,
                    r.output,
                )
            })
        })
        .collect();
    ev.sort_by_key(|e| e.0);

    let output_1m = current_window_sum(&ev, now_ms, WINDOW_1M_MS, true);
    let tokens_5h = current_window_sum(&ev, now_ms, WINDOW_5H_MS, false);
    let cap_out_1m = cfg
        .output_per_min
        .unwrap_or_else(|| max_window_sum(&ev, WINDOW_1M_MS, true));
    let burn_events: Vec<(i64, f64)> = ev.iter().map(|(t, _, out)| (*t, *out as f64)).collect();

    // --- prompt events (for the 5h subscription window) ---
    let prompts_5h = count_in_window(prompt_ts, now_ms, WINDOW_5H_MS);
    let resets_in_secs = prompt_ts
        .iter()
        .copied()
        .filter(|t| *t >= now_ms - WINDOW_5H_MS && *t <= now_ms)
        .min()
        .map(|oldest| ((oldest + WINDOW_5H_MS - now_ms).max(0) / 1000) as u64);

    // Cap precedence: explicit prompts_5h > plan preset > auto (busiest 5h).
    let (cap_prompts_5h, plan_label, auto_prompts_5h) = if let Some(n) = cfg.prompts_5h {
        let label = cfg
            .plan
            .as_deref()
            .and_then(crate::config::plan_prompt_cap)
            .map(|(_, l)| l.to_string())
            .unwrap_or_default();
        (n, label, false)
    } else if let Some((n, label)) = cfg.plan.as_deref().and_then(crate::config::plan_prompt_cap) {
        (n, label.to_string(), false)
    } else {
        let pe: Vec<(i64, u64, u64)> = prompt_ts.iter().map(|t| (*t, 1, 1)).collect();
        // pe is already ascending iff prompt_ts is; sort to be safe.
        let mut pe = pe;
        pe.sort_by_key(|e| e.0);
        (
            max_window_sum(&pe, WINDOW_5H_MS, false),
            String::new(),
            true,
        )
    };

    RateStats {
        prompts_5h,
        cap_prompts_5h,
        auto_prompts_5h,
        plan_label,
        resets_in_secs,
        output_1m,
        cap_out_1m,
        auto_out_1m: cfg.output_per_min.is_none(),
        tokens_5h,
        burn_trend: crate::trend::bucketize(&burn_events, 48),
    }
}

/// Parse one session's JSONL into deduped per-message UsageRecords.
/// `day_fallback` (YYYY-MM-DD) is used when a line lacks a `timestamp`.
pub fn parse_session(jsonl: &str, day_fallback: &str) -> Vec<UsageRecord> {
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
        let Some(usage) = msg.get("usage") else {
            continue;
        };
        let id = msg.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue; // dedupe by message.id
        }
        let ts_str = v.get("timestamp").and_then(|x| x.as_str());
        let day = ts_str
            .and_then(|t| t.split('T').next())
            .unwrap_or(day_fallback)
            .to_string();
        let ts_ms = ts_str
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());
        let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        out.push(UsageRecord {
            day,
            ts_ms,
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
    let mut model: HashMap<String, (f64, u64, u64, u64, u64)> = HashMap::new();
    let mut model_day: HashMap<String, HashMap<String, (f64, u64)>> = HashMap::new();
    let mut cost_events: Vec<(i64, f64)> = Vec::new();
    let mut totals = UsageTotals::default();
    for r in records {
        let cost = record_cost(r, prices);
        if let Some(ts) = r.ts_ms {
            cost_events.push((ts, cost));
        }
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
        m.3 += r.cache_write;
        m.4 += r.cache_read;
        let md = model_day
            .entry(r.model.clone())
            .or_default()
            .entry(r.day.clone())
            .or_default();
        md.0 += cost;
        md.1 += toks;
    }
    totals.by_day = day
        .into_iter()
        .map(|(day, (cost_usd, tokens))| DayUsage {
            day,
            cost_usd,
            tokens,
        })
        .collect();
    totals.by_day.sort_by(|a, b| a.day.cmp(&b.day));
    totals.by_model = model
        .into_iter()
        .map(
            |(model, (cost_usd, input, output, cache_write, cache_read))| ModelUsage {
                model,
                cost_usd,
                input,
                output,
                cache_write,
                cache_read,
            },
        )
        .collect();
    totals.by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    totals.by_model_day = model_day
        .into_iter()
        .map(|(model, days)| {
            let mut v: Vec<DayUsage> = days
                .into_iter()
                .map(|(day, (cost_usd, tokens))| DayUsage {
                    day,
                    cost_usd,
                    tokens,
                })
                .collect();
            v.sort_by(|a, b| a.day.cmp(&b.day));
            (model, v)
        })
        .collect();
    // Adaptive spend curve: hourly when all usage is from ~one day, daily over weeks.
    totals.cost_trend = crate::trend::bucketize(&cost_events, 30);
    totals
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Per-file cache: path -> (mtime, len, parsed records). Re-parse only on change.
pub type UsageCache = HashMap<PathBuf, (SystemTime, u64, Vec<UsageRecord>)>;

/// Recursively collect `*.jsonl` files under `dir`.
pub(crate) fn collect_jsonl(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
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

/// Scan all session transcripts under ~/.claude/projects, re-parsing only changed
/// files (keyed by mtime+len), and return aggregated totals.
pub fn scan_all(cache: &mut UsageCache) -> UsageTotals {
    let prices = crate::collect::pricing::load();
    let Some(home) = crate::util::claude_home() else {
        return UsageTotals::default();
    };
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
    let mut totals = totalize(&all, &prices);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let prompt_ts = crate::collect::activity::read_history();
    totals.rate = rate_stats(&all, &prompt_ts, now_ms, &crate::config::load().rate_limit);
    totals
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

    fn rec(ts_ms: i64, input: u64, output: u64) -> UsageRecord {
        UsageRecord {
            day: "d".into(),
            ts_ms: Some(ts_ms),
            model: "claude-x".into(),
            input,
            output,
            cache_write: 0,
            cache_read: 0,
        }
    }

    #[test]
    fn rate_windows_and_auto_caps() {
        let now = 100_000_000_000i64; // arbitrary fixed "now" in ms
        let recs = vec![
            rec(now - 6 * 3_600_000, 1000, 100), // 6h ago: outside the 5h window
            rec(now - 2 * 3_600_000, 2000, 200), // 2h ago: inside 5h, outside 1m
            rec(now - 30_000, 500, 50),          // 30s ago: inside both windows
            rec(now - 10_000, 300, 30),          // 10s ago: inside both windows
        ];
        // One prompt 6h ago (out of window) and three within the 5h window.
        let prompts = vec![
            now - 6 * 3_600_000,
            now - 2 * 3_600_000,
            now - 30_000,
            now - 10_000,
        ];
        let cfg = crate::config::RateLimit::default(); // all auto, no plan
        let r = rate_stats(&recs, &prompts, now, &cfg);

        // 5h token volume excludes the 6h-old record.
        assert_eq!(r.tokens_5h, 2200 + 550 + 330);
        // OTPM = output tokens in the trailing minute (last two records).
        assert_eq!(r.output_1m, 50 + 30);
        // 3 prompts fall inside the 5h window.
        assert_eq!(r.prompts_5h, 3);
        // Auto caps with no plan/config; cap covers current usage.
        assert!(r.auto_prompts_5h && r.auto_out_1m && r.plan_label.is_empty());
        assert!(r.cap_prompts_5h >= r.prompts_5h);
        // Oldest in-window prompt is 2h old → resets in ~3h.
        let resets = r.resets_in_secs.unwrap();
        assert!((10_700..=10_900).contains(&resets), "resets={resets}");
    }

    #[test]
    fn rate_uses_plan_and_configured_caps() {
        let now = 100_000_000_000i64;
        let recs = vec![rec(now - 10_000, 1000, 500)];
        let prompts = vec![now - 10_000];
        let cfg = crate::config::RateLimit {
            plan: Some("max5x".into()),
            prompts_5h: None,
            output_per_min: Some(100_000),
        };
        let r = rate_stats(&recs, &prompts, now, &cfg);
        assert_eq!(r.cap_prompts_5h, 100); // Max 5x preset
        assert_eq!(r.plan_label, "Max 5x");
        assert!(!r.auto_prompts_5h);
        assert_eq!(r.cap_out_1m, 100_000);
        assert!(!r.auto_out_1m);

        // Explicit prompts_5h overrides the plan preset.
        let cfg2 = crate::config::RateLimit {
            plan: Some("pro".into()),
            prompts_5h: Some(50),
            output_per_min: None,
        };
        let r2 = rate_stats(&recs, &prompts, now, &cfg2);
        assert_eq!(r2.cap_prompts_5h, 50);
        assert!(r2.auto_out_1m); // OTPM left unset -> auto
    }

    #[test]
    fn dedups_by_message_id_then_sums() {
        let lines = [
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
            ts_ms: None,
            model: "claude-x".into(),
            input: 1000,
            output: 100,
            cache_write: 0,
            cache_read: 0,
        }];
        let mut map = HashMap::new();
        map.insert(
            "claude-x".to_string(),
            ModelPrice {
                input: 1e-6,
                output: 5e-6,
                cache_write: 0.0,
                cache_read: 0.0,
            },
        );
        let totals = totalize(&recs, &PriceTable(map));
        assert!((totals.total_cost_usd - (1000.0 * 1e-6 + 100.0 * 5e-6)).abs() < 1e-12);
        assert_eq!(totals.fresh_input, 1000);
    }

    #[test]
    fn per_model_cache_split_and_daily() {
        let recs = vec![
            UsageRecord {
                day: "2026-06-22".into(),
                ts_ms: None,
                model: "claude-x".into(),
                input: 100,
                output: 10,
                cache_write: 5,
                cache_read: 50,
            },
            UsageRecord {
                day: "2026-06-23".into(),
                ts_ms: None,
                model: "claude-x".into(),
                input: 200,
                output: 20,
                cache_write: 0,
                cache_read: 80,
            },
        ];
        let mut map = HashMap::new();
        map.insert(
            "claude-x".to_string(),
            ModelPrice {
                input: 1e-6,
                output: 5e-6,
                cache_write: 0.0,
                cache_read: 0.0,
            },
        );
        let totals = totalize(&recs, &PriceTable(map));

        let m = &totals.by_model[0];
        assert_eq!(m.cache_write, 5);
        assert_eq!(m.cache_read, 130);

        let days = totals
            .by_model_day
            .get("claude-x")
            .expect("model day history");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].day, "2026-06-22"); // ascending
        assert_eq!(days[1].day, "2026-06-23");
    }
}
