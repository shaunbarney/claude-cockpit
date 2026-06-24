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
}

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

/// Bucket `(timestamp_ms, value)` events into a gap-filled, ascending series.
///
/// Granularity adapts to the span between the first and last event: under ~36h
/// it buckets by hour, otherwise by day. Buckets are aligned to the step
/// boundary and gaps are zero-filled so the curve is continuous. At most the
/// most-recent `max_buckets` are kept. Empty input yields an empty `Trend`.
pub fn bucketize(events: &[(i64, f64)], max_buckets: usize) -> Trend {
    if events.is_empty() || max_buckets == 0 {
        return Trend::default();
    }
    let min = events.iter().map(|(t, _)| *t).min().unwrap();
    let max = events.iter().map(|(t, _)| *t).max().unwrap();
    let hourly = (max - min) < 36 * HOUR_MS;
    let step = if hourly { HOUR_MS } else { DAY_MS };

    // Align the first bucket to a step boundary for stable labels.
    let start = min.div_euclid(step) * step;
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
    Trend { values, label }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = HOUR_MS;
    const D: i64 = DAY_MS;

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(bucketize(&[], 30), Trend::default());
        assert_eq!(bucketize(&[(0, 1.0)], 0), Trend::default());
    }

    #[test]
    fn single_day_buckets_hourly_and_fills_gaps() {
        // Events at hour 0, hour 0, and hour 3 of the same day.
        let base = 10 * D; // aligned to a day boundary
        let t = bucketize(&[(base, 1.0), (base + 5_000, 1.0), (base + 3 * H, 1.0)], 30);
        // Buckets for hours 0..=3 → length 4, gaps zero-filled.
        assert_eq!(t.values, vec![2.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.label, "last 4h");
    }

    #[test]
    fn multi_day_buckets_daily() {
        let base = 10 * D;
        let t = bucketize(&[(base, 2.0), (base + 2 * D, 3.0)], 30);
        assert_eq!(t.values, vec![2.0, 0.0, 3.0]);
        assert_eq!(t.label, "last 3 days");
    }

    #[test]
    fn keeps_only_most_recent_max_buckets() {
        let base = 10 * D;
        let events: Vec<(i64, f64)> = (0..10).map(|i| (base + i * D, i as f64)).collect();
        let t = bucketize(&events, 3);
        // Most recent 3 daily buckets: values 7,8,9.
        assert_eq!(t.values, vec![7.0, 8.0, 9.0]);
        assert_eq!(t.label, "last 3 days");
    }
}
