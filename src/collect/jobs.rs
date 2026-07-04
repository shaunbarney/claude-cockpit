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
    // reserved: job working-directory display in the job detail header.
    #[allow(dead_code)]
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// One timeline event.
#[derive(Debug, Clone)]
pub struct JobEvent {
    // reserved: per-event timestamps in the job timeline view.
    #[allow(dead_code)]
    pub at: i64,
    pub state: String,
    pub detail: String,
    pub text: String,
}

/// RFC3339 timestamp -> epoch seconds.
pub fn iso_to_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Parse a job's `state.json`. Defensive: tolerates missing/optional fields.
pub fn parse_state(id: &str, json: &str) -> Option<Job> {
    let v: Value = serde_json::from_str(json).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let opt = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let inflight = v.get("inFlight");
    let num = |k: &str| {
        inflight
            .and_then(|o| o.get(k))
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32
    };
    let raw_name = s("name");
    let name = if raw_name.is_empty() {
        id.to_string()
    } else {
        raw_name
    };
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
        created_at: v
            .get("createdAt")
            .and_then(|x| x.as_str())
            .and_then(iso_to_epoch),
        updated_at: v
            .get("updatedAt")
            .and_then(|x| x.as_str())
            .and_then(iso_to_epoch),
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
    let Some(home) = claude_home() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(home.join("jobs")) else {
        return vec![];
    };
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

/// Lowercased alphanumerics only — so "RemoteLogging" and "remote-logging-mcp"
/// can be compared by containment.
fn norm_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Make every worktree visible in the Jobs pane.
///
/// For each worktree no job's `worktreePath` owns, first try to attribute it to
/// an existing agent by name: a job with no worktree of its own whose
/// normalised name is contained in the worktree's folder/branch name (e.g.
/// agent `RemoteLogging` ↔ worktree `remote-logging-mcp`). Only an
/// **unambiguous** match links (one candidate job for the worktree, and that
/// job matches no other orphan). Anything still unowned gets a synthetic
/// "no agent" row so the pane reflects all worktrees on disk.
pub fn merge_orphan_worktrees(
    mut jobs: Vec<Job>,
    worktrees: &[crate::collect::git::Worktree],
) -> Vec<Job> {
    let norm_path = |p: &str| p.trim_end_matches('/').to_string();
    let owned: std::collections::HashSet<String> = jobs
        .iter()
        .filter_map(|j| j.worktree_path.as_deref().map(norm_path))
        .collect();
    let orphans: Vec<&crate::collect::git::Worktree> = worktrees
        .iter()
        .filter(|wt| !owned.contains(&norm_path(&wt.path)))
        .collect();

    // Candidate (job index, orphan index) name matches.
    let job_matches = |j: &Job, wt: &crate::collect::git::Worktree| {
        if j.worktree_path.is_some() {
            return false;
        }
        let jn = norm_name(&j.name);
        !jn.is_empty() && (norm_name(&wt.name).contains(&jn) || norm_name(&wt.branch).contains(&jn))
    };
    let mut attached: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (wi, wt) in orphans.iter().enumerate() {
        let candidates: Vec<usize> = jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| job_matches(j, wt))
            .map(|(i, _)| i)
            .collect();
        // Link only when exactly one job matches this worktree AND that job
        // doesn't also match another orphan (ambiguity → leave both unlinked).
        if let [ji] = candidates.as_slice() {
            let unique = orphans
                .iter()
                .enumerate()
                .filter(|(oi, o)| *oi != wi && job_matches(&jobs[*ji], o))
                .count()
                == 0;
            if unique {
                jobs[*ji].worktree_path = Some(wt.path.clone());
                jobs[*ji].worktree_branch = Some(wt.branch.clone());
                attached.insert(wi);
            }
        }
    }

    for (wi, wt) in orphans.iter().enumerate() {
        if attached.contains(&wi) {
            continue;
        }
        jobs.push(Job {
            id: format!("worktree:{}", wt.path),
            name: wt.name.clone(),
            state: "no agent".into(),
            tempo: String::new(),
            intent: String::new(),
            tasks: 0,
            queued: 0,
            cwd: wt.path.clone(),
            worktree_path: Some(wt.path.clone()),
            worktree_branch: Some(wt.branch.clone()),
            created_at: None,
            updated_at: None,
        });
    }
    jobs
}

/// Parse a `timeline.jsonl` body, skipping unparseable lines.
pub fn parse_timeline(s: &str) -> Vec<JobEvent> {
    s.lines()
        .filter_map(|line| {
            let v: Value = serde_json::from_str(line).ok()?;
            Some(JobEvent {
                at: v.get("at").and_then(|x| x.as_i64()).unwrap_or(0),
                state: v
                    .get("state")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                detail: v
                    .get("detail")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                text: v
                    .get("text")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

/// Read the last `tail` events of a job's timeline (empty on IO failure).
pub fn read_timeline(job_id: &str, tail: usize) -> Vec<JobEvent> {
    let Some(home) = claude_home() else {
        return vec![];
    };
    let p = home.join("jobs").join(job_id).join("timeline.jsonl");
    let Ok(txt) = std::fs::read_to_string(p) else {
        return vec![];
    };
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
    use crate::collect::git::Worktree;

    fn wt(name: &str, path: &str, branch: &str) -> Worktree {
        Worktree {
            name: name.into(),
            path: path.into(),
            branch: branch.into(),
            ahead: 0,
            dirty: 0,
            committed: (0, 0),
            uncommitted: (0, 0),
            age: String::new(),
        }
    }

    #[test]
    fn appends_orphan_worktrees_only() {
        let owned = parse_state("id", SAMPLE).unwrap(); // owns /repo/.claude/worktrees/x
        let worktrees = vec![
            wt("x", "/repo/.claude/worktrees/x", "worktree-x"), // owned → skipped
            wt("y", "/repo/.claude/worktrees/y", "feature/y"),  // orphan → appended
        ];
        let merged = merge_orphan_worktrees(vec![owned], &worktrees);
        assert_eq!(merged.len(), 2);
        let orphan = merged.iter().find(|j| j.name == "y").unwrap();
        assert_eq!(orphan.state, "no agent");
        assert_eq!(orphan.worktree_branch.as_deref(), Some("feature/y"));
        // The owned worktree must not be duplicated as an orphan.
        assert_eq!(merged.iter().filter(|j| j.name == "x").count(), 0);
        assert!(merged.iter().any(|j| j.name == "Governour"));
    }

    fn bare_job(name: &str) -> Job {
        Job {
            id: name.to_lowercase(),
            name: name.into(),
            state: "blocked".into(),
            tempo: String::new(),
            intent: String::new(),
            tasks: 0,
            queued: 0,
            cwd: String::new(),
            worktree_path: None,
            worktree_branch: None,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn attributes_orphan_worktree_to_agent_by_name() {
        // Agent "RemoteLogging" has no worktree; worktree "remote-logging-mcp"
        // has no owner → they should link, with NO synthetic row created.
        let jobs = vec![bare_job("RemoteLogging"), bare_job("Dotfiles")];
        let worktrees = vec![
            wt(
                "remote-logging-mcp",
                "/r/.claude/worktrees/remote-logging-mcp",
                "feature/logs",
            ),
            wt(
                "admin-backoffice",
                "/r/.claude/worktrees/admin-backoffice",
                "feature/admin",
            ),
        ];
        let merged = merge_orphan_worktrees(jobs, &worktrees);

        let rl = merged.iter().find(|j| j.name == "RemoteLogging").unwrap();
        assert_eq!(
            rl.worktree_path.as_deref(),
            Some("/r/.claude/worktrees/remote-logging-mcp")
        );
        assert_eq!(rl.state, "blocked"); // keeps its real state
                                         // No synthetic row for the attributed worktree…
        assert!(!merged.iter().any(|j| j.name == "remote-logging-mcp"));
        // …but the truly unmatched one still gets its 'no agent' row.
        let orphan = merged
            .iter()
            .find(|j| j.name == "admin-backoffice")
            .unwrap();
        assert_eq!(orphan.state, "no agent");
        // Dotfiles matched nothing and stays worktree-less.
        let dot = merged.iter().find(|j| j.name == "Dotfiles").unwrap();
        assert!(dot.worktree_path.is_none());
    }

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
            id: "x".into(),
            name: "x".into(),
            state: state.into(),
            tempo: tempo.into(),
            intent: String::new(),
            tasks: 0,
            queued: 0,
            cwd: String::new(),
            worktree_path: None,
            worktree_branch: None,
            created_at: None,
            updated_at: Some(updated),
        }
    }

    #[test]
    fn ranks_active_first() {
        let mut v = [
            mk("done", "idle", 100),
            mk("working", "active", 50),
            mk("blocked", "blocked", 80),
        ];
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
