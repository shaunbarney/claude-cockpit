//! Adaptive time-bucketing for trend charts.
//!
//! A fixed daily granularity makes a brand-new machine — which only has a few
//! hours of `~/.claude` data — look broken: a single day is one bar / one point.
//! `bucketize` instead picks its granularity from the data's own span, so the
//! same code draws an hourly curve for today and a daily curve over weeks.

/// A continuous, gap-filled value series plus a short human label describing
/// the granularity actually chosen (e.g. `"last 9h"` or `"last 24 days"`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trend {
    pub values: Vec<f64>,
    pub label: String,
    /// True when each bucket is one hour (otherwise one local day).
    pub hourly: bool,
}

impl Trend {
    /// Per-bucket unit for axis captions: `"hour"` or `"day"`.
    pub fn unit(&self) -> &'static str {
        if self.hourly {
            "hour"
        } else {
            "day"
        }
    }
}

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

/// Bucket `(timestamp_ms, value)` events into a gap-filled, ascending series
/// that runs up to `now_ms` (so idle time shows as trailing zeros rather than
/// the curve silently ending at the last event).
///
/// Granularity adapts to the span from the first event to now: under ~36h it
/// buckets by hour, otherwise by day. Buckets are aligned to the **local**
/// hour/day boundary and gaps are zero-filled. At most the most-recent
/// `max_buckets` are kept. Empty input yields an empty `Trend`.
pub fn bucketize(events: &[(i64, f64)], max_buckets: usize, now_ms: i64) -> Trend {
    let offset_ms = i64::from(chrono::Local::now().offset().local_minus_utc()) * 1000;
    bucketize_at(events, max_buckets, now_ms, offset_ms)
}

/// [`bucketize`] with an explicit UTC offset (ms) for bucket alignment.
fn bucketize_at(events: &[(i64, f64)], max_buckets: usize, now_ms: i64, offset_ms: i64) -> Trend {
    if events.is_empty() || max_buckets == 0 {
        return Trend::default();
    }
    let min = events.iter().map(|(t, _)| *t).min().unwrap();
    let max = events.iter().map(|(t, _)| *t).max().unwrap().max(now_ms);
    let hourly = (max - min) < 36 * HOUR_MS;
    let step = if hourly { HOUR_MS } else { DAY_MS };

    // Align the first bucket to a local step boundary for stable labels.
    let start = (min + offset_ms).div_euclid(step) * step - offset_ms;
    let n = ((max - start).div_euclid(step) + 1) as usize;
    let mut values = vec![0.0; n];
    for (t, v) in events {
        let idx = (t - start).div_euclid(step) as usize;
        values[idx] += *v;
    }

    // Keep only the most recent `max_buckets`.
    if values.len() > max_buckets {
        values = values.split_off(values.len() - max_buckets);
    }

    let label = if hourly {
        format!("last {}h", values.len())
    } else {
        format!("last {} days", values.len())
    };
    Trend {
        values,
        label,
        hourly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = HOUR_MS;
    const D: i64 = DAY_MS;

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(bucketize_at(&[], 30, 0, 0), Trend::default());
        assert_eq!(bucketize_at(&[(0, 1.0)], 0, 0, 0), Trend::default());
    }

    #[test]
    fn single_day_buckets_hourly_and_fills_gaps() {
        // Events at hour 0, hour 0, and hour 3 of the same day; now = hour 3.
        let base = 10 * D; // aligned to a day boundary
        let ev = [(base, 1.0), (base + 5_000, 1.0), (base + 3 * H, 1.0)];
        let t = bucketize_at(&ev, 30, base + 3 * H, 0);
        // Buckets for hours 0..=3 → length 4, gaps zero-filled.
        assert_eq!(t.values, vec![2.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.label, "last 4h");
        assert!(t.hourly);
    }

    #[test]
    fn zero_fills_up_to_now() {
        let base = 10 * D;
        // Last event at hour 1, but it's now hour 4: trailing idle hours show as 0.
        let t = bucketize_at(&[(base, 1.0), (base + H, 2.0)], 30, base + 4 * H, 0);
        assert_eq!(t.values, vec![1.0, 2.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn multi_day_buckets_daily() {
        let base = 10 * D;
        let t = bucketize_at(&[(base, 2.0), (base + 2 * D, 3.0)], 30, base + 2 * D, 0);
        assert_eq!(t.values, vec![2.0, 0.0, 3.0]);
        assert_eq!(t.label, "last 3 days");
        assert_eq!(t.unit(), "day");
    }

    #[test]
    fn daily_buckets_align_to_local_midnight() {
        // UTC+4: 21:00Z and 23:00Z on day 10 are 01:00 / 03:00 local on day 11,
        // so they share a bucket with 10:00Z on day 11 — not with day 10.
        let off = 4 * H;
        let base = 10 * D;
        let ev = [
            (base - 2 * D, 1.0), // force daily granularity
            (base + 21 * H, 1.0),
            (base + 23 * H, 1.0),
            (base + D + 10 * H, 1.0),
        ];
        let t = bucketize_at(&ev, 30, base + D + 10 * H, off);
        assert_eq!(t.values.last(), Some(&3.0));
    }

    #[test]
    fn keeps_only_most_recent_max_buckets() {
        let base = 10 * D;
        let events: Vec<(i64, f64)> = (0..10).map(|i| (base + i * D, i as f64)).collect();
        let t = bucketize_at(&events, 3, base + 9 * D, 0);
        // Most recent 3 daily buckets: values 7,8,9.
        assert_eq!(t.values, vec![7.0, 8.0, 9.0]);
        assert_eq!(t.label, "last 3 days");
    }
}
