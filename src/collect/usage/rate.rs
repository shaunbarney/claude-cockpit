//! Rate-limit proximity (rolling 5-hour prompt window + per-minute output burn).

use super::UsageRecord;

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
/// (human turns from the transcripts, see `collect::activity`) — the unit the
/// subscription 5h limit is measured in;
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
        burn_trend: crate::trend::bucketize(&burn_events, 48, now_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts_ms: i64, input: u64, output: u64) -> UsageRecord {
        UsageRecord {
            day: "d".into(),
            ts_ms: Some(ts_ms),
            model: "claude-x".into(),
            input,
            output,
            ..Default::default()
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
}
