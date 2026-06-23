//! Prompt-cadence from ~/.claude/history.jsonl.
use serde_json::Value;

/// Count prompts per UTC day from millisecond timestamps, ascending by day.
pub fn daily_counts(timestamps_ms: &[i64]) -> Vec<(String, u32)> {
    use std::collections::BTreeMap;
    let mut m: BTreeMap<String, u32> = BTreeMap::new();
    for &ts in timestamps_ms {
        if let Some(dt) = chrono::DateTime::from_timestamp_millis(ts) {
            *m.entry(dt.format("%Y-%m-%d").to_string()).or_default() += 1;
        }
    }
    m.into_iter().collect()
}

/// Read prompt timestamps (ms) from ~/.claude/history.jsonl (empty on IO failure).
pub fn read_history() -> Vec<i64> {
    let Some(home) = crate::util::claude_home() else { return vec![] };
    let Ok(txt) = std::fs::read_to_string(home.join("history.jsonl")) else { return vec![] };
    txt.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()?.get("timestamp")?.as_i64())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(date: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&format!("{date}T12:00:00Z"))
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn buckets_by_utc_day() {
        let v = daily_counts(&[ms("2026-06-22"), ms("2026-06-22"), ms("2026-06-23")]);
        assert_eq!(
            v,
            vec![("2026-06-22".to_string(), 2), ("2026-06-23".to_string(), 1)]
        );
    }

    #[test]
    fn empty_is_empty() {
        assert!(daily_counts(&[]).is_empty());
    }
}
