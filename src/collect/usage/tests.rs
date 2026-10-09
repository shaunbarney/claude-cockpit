use super::*;
use crate::collect::pricing::{ModelPrice, PriceTable};
use std::collections::HashMap;

fn asst(id: &str, model: &str, input: u64, output: u64, cw: u64, cr: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"2026-06-22T10:00:00Z","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_creation_input_tokens":{cw},"cache_read_input_tokens":{cr}}}}}}}"#
    )
}

fn price(input: f64, output: f64) -> ModelPrice {
    ModelPrice {
        input,
        output,
        cache_write: input * 1.25,
        cache_write_1h: input * 2.0,
        cache_read: input * 0.1,
    }
}

fn table(model: &str, p: ModelPrice) -> PriceTable {
    let mut map = HashMap::new();
    map.insert(model.to_string(), p);
    PriceTable(map)
}

fn rec(day: &str, model: &str, input: u64, output: u64, cw: u64, cr: u64) -> UsageRecord {
    UsageRecord {
        day: day.into(),
        model: model.into(),
        input,
        output,
        cache_write: cw,
        cache_read: cr,
        ..Default::default()
    }
}

#[test]
fn dedups_by_message_id_then_sums() {
    let lines = [
        asst("m1", "claude-opus-4-8", 100, 10, 0, 0),
        asst("m1", "claude-opus-4-8", 100, 10, 0, 0), // streaming dupe
        asst("m2", "claude-opus-4-8", 200, 20, 0, 0),
    ]
    .join("\n");
    let recs = parse_session(&lines);
    let in_tokens: u64 = recs.iter().map(|r| r.input).sum();
    assert_eq!(recs.len(), 2);
    assert_eq!(in_tokens, 300);
}

#[test]
fn streaming_copies_keep_finalized_usage() {
    // Real streaming shape: early copies carry partial output_tokens.
    let lines = [5, 5, 5, 5, 337]
        .iter()
        .map(|o| asst("m1", "claude-opus-5", 3, *o, 1000, 0))
        .collect::<Vec<_>>()
        .join("\n");
    let recs = parse_session(&lines);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].output, 337);
    assert_eq!(recs[0].cache_write, 1000);
}

#[test]
fn skips_synthetic_model() {
    let recs = parse_session(&asst("m1", "<synthetic>", 0, 0, 0, 0));
    assert!(recs.is_empty());
}

#[test]
fn one_hour_cache_writes_priced_at_2x() {
    let line = r#"{"type":"assistant","timestamp":"2026-06-22T10:00:00Z","message":{"id":"m1","model":"claude-x","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":300,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":200}}}}"#;
    let recs = parse_session(line);
    assert_eq!(recs[0].cache_write, 300);
    assert_eq!(recs[0].cache_write_1h, 200);
    let t = totalize(&recs, &table("claude-x", price(1e-6, 5e-6)), 0);
    // 100 @ 1.25x + 200 @ 2x input.
    let want = 100.0 * 1.25e-6 + 200.0 * 2e-6;
    assert!(
        (t.total_cost_usd - want).abs() < 1e-12,
        "{}",
        t.total_cost_usd
    );
}

#[test]
fn dedupe_is_global_across_files() {
    // A resumed session copies m1 into a second transcript.
    let mut a = UsageFile::default();
    let mut b = UsageFile::default();
    parse_lines(&mut a, &asst("m1", "claude-x", 100, 5, 0, 0), true);
    parse_lines(
        &mut b,
        &[
            asst("m1", "claude-x", 100, 50, 0, 0),
            asst("m2", "claude-x", 10, 1, 0, 0),
        ]
        .join("\n"),
        true,
    );
    let (recs, _) = merge_files([&a, &b].into_iter());
    assert_eq!(recs.len(), 2);
    let m1 = recs.iter().find(|r| r.input == 100).unwrap();
    assert_eq!(m1.output, 50); // per-field max across copies
}

#[test]
fn prompts_come_from_main_session_human_turns() {
    let lines = [
        r#"{"type":"user","uuid":"u1","timestamp":"2026-06-22T10:00:00Z","message":{"role":"user","content":"hello"}}"#,
        r#"{"type":"user","uuid":"u2","timestamp":"2026-06-22T10:01:00Z","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}"#,
        r#"{"type":"user","uuid":"u3","timestamp":"2026-06-22T10:02:00Z","message":{"role":"user","content":"<command-name>/model</command-name>"}}"#,
    ]
    .join("\n");
    let mut main = UsageFile::default();
    parse_lines(&mut main, &lines, true);
    let mut sub = UsageFile::default();
    parse_lines(&mut sub, &lines, false);
    // A forked copy of the same prompt (same uuid) counts once.
    let mut fork = UsageFile::default();
    parse_lines(&mut fork, &lines, true);
    let (_, prompts) = merge_files([&main, &sub, &fork].into_iter());
    assert_eq!(prompts.len(), 1);
}

#[test]
fn subagent_paths_detected() {
    assert!(is_subagent(Path::new("/p/-x/sess/subagents/agent-a.jsonl")));
    assert!(!is_subagent(Path::new("/p/-x/sess.jsonl")));
}

#[test]
fn costs_with_table() {
    let recs = vec![rec("d", "claude-x", 1000, 100, 0, 0)];
    let p = ModelPrice {
        input: 1e-6,
        output: 5e-6,
        cache_write: 0.0,
        cache_write_1h: 0.0,
        cache_read: 0.0,
    };
    let totals = totalize(&recs, &table("claude-x", p), 0);
    assert!((totals.total_cost_usd - (1000.0 * 1e-6 + 100.0 * 5e-6)).abs() < 1e-12);
    assert_eq!(totals.fresh_input, 1000);
    assert!(totals.unpriced.is_empty());
}

#[test]
fn unknown_models_are_flagged_unpriced() {
    let recs = vec![
        rec("2026-06-22", "claude-x", 10, 1, 0, 0),
        rec("2026-06-22", "mystery-model", 10, 1, 0, 0),
    ];
    let t = totalize(&recs, &table("claude-x", price(1e-6, 5e-6)), 0);
    assert_eq!(t.unpriced, vec!["mystery-model".to_string()]);
}

#[test]
fn since_day_and_period() {
    let recs = vec![
        rec("2026-06-20", "claude-x", 1, 1, 0, 0),
        rec("2026-06-22", "claude-x", 1, 1, 0, 0),
        rec("unknown", "claude-x", 1, 1, 0, 0),
    ];
    let t = totalize(&recs, &table("claude-x", price(1e-6, 5e-6)), 0);
    assert_eq!(t.since_day.as_deref(), Some("2026-06-20"));
    assert_eq!(t.period_days("2026-06-22"), Some(3));
}

#[test]
fn day_keys_are_local() {
    let line = asst("m1", "claude-x", 1, 1, 0, 0);
    let recs = parse_session(&line);
    let ts = chrono::DateTime::parse_from_rfc3339("2026-06-22T10:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(Some(recs[0].day.clone()), local_day(ts));
    assert_eq!(today_local().len(), 10);
}

#[test]
fn per_model_cache_split_and_daily() {
    let recs = vec![
        rec("2026-06-22", "claude-x", 100, 10, 5, 50),
        rec("2026-06-23", "claude-x", 200, 20, 0, 80),
    ];
    let totals = totalize(&recs, &table("claude-x", price(1e-6, 5e-6)), 0);

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
