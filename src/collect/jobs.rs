//! Claude Code background-job monitoring: ~/.claude/jobs/*/state.json + per-job timeline.jsonl.

use serde_json::Value;

use crate::util::claude_home;

/// One background job's snapshot.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub state: String,
    pub tempo: String,
    pub intent: String,
    pub tasks: u32,
    pub queued: u32,
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// One timeline event.
#[derive(Debug, Clone)]
pub struct JobEvent {
    pub at: i64,
    pub state: String,
    pub detail: String,
    pub text: String,
}

/// RFC3339 timestamp -> epoch seconds.
pub fn iso_to_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}

/// Parse a job's `state.json`. Defensive: tolerates missing/optional fields.
pub fn parse_state(id: &str, json: &str) -> Option<Job> {
    let v: Value = serde_json::from_str(json).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let opt = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let inflight = v.get("inFlight");
    let num = |k: &str| {
        inflight.and_then(|o| o.get(k)).and_then(|x| x.as_u64()).unwrap_or(0) as u32
    };
    let raw_name = s("name");
    let name = if raw_name.is_empty() { id.to_string() } else { raw_name };
    Some(Job {
        id: id.to_string(),
        name,
        state: s("state"),
        tempo: s("tempo"),
        intent: s("intent"),
        tasks: num("tasks"),
        queued: num("queued"),
        cwd: s("cwd"),
        worktree_path: opt("worktreePath"),
        worktree_branch: opt("worktreeBranch"),
        created_at: v.get("createdAt").and_then(|x| x.as_str()).and_then(iso_to_epoch),
        updated_at: v.get("updatedAt").and_then(|x| x.as_str()).and_then(iso_to_epoch),
    })
}

/// Sort key: working first, then blocked, then others; newest `updated_at` first within a bucket.
pub fn rank_job(j: &Job) -> (u8, i64) {
    let bucket = match j.state.as_str() {
        "working" => 0,
        "blocked" => 1,
        _ => 2,
    };
    (bucket, -j.updated_at.unwrap_or(0))
}

/// Scan `~/.claude/jobs/*/state.json` into ranked jobs (empty on any IO failure).
pub fn gather_jobs() -> Vec<Job> {
    let Some(home) = claude_home() else { return vec![] };
    let Ok(entries) = std::fs::read_dir(home.join("jobs")) else { return vec![] };
    let mut out = Vec::new();
    for e in entries.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let id = e.file_name().to_string_lossy().into_owned();
        if let Ok(txt) = std::fs::read_to_string(e.path().join("state.json")) {
            if let Some(job) = parse_state(&id, &txt) {
                out.push(job);
            }
        }
    }
    out.sort_by_key(rank_job);
    out
}

/// Parse a `timeline.jsonl` body, skipping unparseable lines.
pub fn parse_timeline(s: &str) -> Vec<JobEvent> {
    s.lines()
        .filter_map(|line| {
            let v: Value = serde_json::from_str(line).ok()?;
            Some(JobEvent {
                at: v.get("at").and_then(|x| x.as_i64()).unwrap_or(0),
                state: v.get("state").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                detail: v.get("detail").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                text: v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Read the last `tail` events of a job's timeline (empty on IO failure).
pub fn read_timeline(job_id: &str, tail: usize) -> Vec<JobEvent> {
    let Some(home) = claude_home() else { return vec![] };
    let p = home.join("jobs").join(job_id).join("timeline.jsonl");
    let Ok(txt) = std::fs::read_to_string(p) else { return vec![] };
    let all = parse_timeline(&txt);
    if all.len() > tail {
        all[all.len() - tail..].to_vec()
    } else {
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"state":"working","tempo":"active","inFlight":{"tasks":2,"queued":1},
        "intent":"do the thing","name":"Governour","cwd":"/repo",
        "worktreePath":"/repo/.claude/worktrees/x","worktreeBranch":"worktree-x",
        "createdAt":"2026-06-22T13:36:49.894Z","updatedAt":"2026-06-22T13:39:17.364Z"}"#;

    #[test]
    fn parses_core_fields() {
        let j = parse_state("abc123", SAMPLE).unwrap();
        assert_eq!(j.id, "abc123");
        assert_eq!(j.name, "Governour");
        assert_eq!(j.state, "working");
        assert_eq!(j.tasks, 2);
        assert_eq!(j.queued, 1);
        assert_eq!(j.worktree_branch.as_deref(), Some("worktree-x"));
        assert!(j.updated_at.unwrap() > j.created_at.unwrap());
    }

    #[test]
    fn tolerates_missing_optional() {
        let j = parse_state("z", r#"{"state":"idle"}"#).unwrap();
        assert_eq!(j.tasks, 0);
        assert!(j.worktree_path.is_none());
        assert_eq!(j.name, "z"); // falls back to id when name absent
    }

    fn mk(state: &str, tempo: &str, updated: i64) -> Job {
        Job {
            id: "x".into(), name: "x".into(), state: state.into(), tempo: tempo.into(),
            intent: String::new(), tasks: 0, queued: 0, cwd: String::new(),
            worktree_path: None, worktree_branch: None,
            created_at: None, updated_at: Some(updated),
        }
    }

    #[test]
    fn ranks_active_first() {
        let mut v = vec![mk("done", "idle", 100), mk("working", "active", 50), mk("blocked", "blocked", 80)];
        v.sort_by_key(rank_job);
        assert_eq!(v[0].state, "working");
        assert_eq!(v[1].state, "blocked");
        assert_eq!(v[2].state, "done");
    }

    #[test]
    fn parses_timeline_lines() {
        let s = "{\"at\":1,\"state\":\"working\",\"detail\":\"a\",\"text\":\"x\"}\nbad line\n{\"at\":2,\"state\":\"done\",\"detail\":\"b\",\"text\":\"y\"}";
        let ev = parse_timeline(s);
        assert_eq!(ev.len(), 2); // bad line skipped
        assert_eq!(ev[1].state, "done");
    }
}
