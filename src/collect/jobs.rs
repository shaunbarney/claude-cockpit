//! Claude Code background-job monitoring: ~/.claude/jobs/*/state.json + per-job timeline.jsonl.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::collect::git::Worktree;
use crate::util::claude_home;

/// One background job's snapshot.
///
/// `state` is the raw state string (`working`, `blocked`, `done`, `failed`, …);
/// use [`is_error_state`] rather than matching error spellings by hand.
#[derive(Debug, Clone, Default)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub state: String,
    pub tempo: String,
    pub intent: String,
    pub tasks: u32,
    pub queued: u32,
    /// Working directory the job runs in.
    pub cwd: String,
    /// Directory the job was launched from (`originCwd`), if recorded.
    pub origin_cwd: Option<String>,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    /// Latest one-line status the agent reported (`detail`).
    pub detail: String,
    /// What a blocked job is waiting on (`needs`).
    pub needs: String,
    /// The agent's proposed answer to its own question (`suggestedReply`).
    pub suggested_reply: String,
    /// When the job last reached a terminal state (`lastTerminalAt`).
    pub last_terminal_at: Option<i64>,
    /// Final result text (`output.result`).
    pub output_result: String,
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

/// A timestamp field that may be an RFC3339 string or a bare epoch integer.
fn epoch_field(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_str().and_then(iso_to_epoch).or_else(|| v.as_i64())
}

/// True for states that mean the job ended badly.
pub fn is_error_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "failed" | "error" | "errored" | "crashed" | "killed"
    )
}

/// Parse a job's `state.json`. Defensive: tolerates missing/optional fields.
pub fn parse_state(id: &str, json: &str) -> Option<Job> {
    let v: Value = serde_json::from_str(json).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let opt = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
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
    let output_result = v
        .get("output")
        .and_then(|o| o.get("result"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(Job {
        id: id.to_string(),
        name,
        state: s("state"),
        tempo: s("tempo"),
        intent: s("intent"),
        tasks: num("tasks"),
        queued: num("queued"),
        cwd: s("cwd"),
        origin_cwd: opt("originCwd"),
        worktree_path: opt("worktreePath"),
        worktree_branch: opt("worktreeBranch"),
        created_at: epoch_field(v.get("createdAt")),
        updated_at: epoch_field(v.get("updatedAt")),
        detail: s("detail"),
        needs: s("needs"),
        suggested_reply: s("suggestedReply"),
        last_terminal_at: epoch_field(v.get("lastTerminalAt")),
        output_result,
    })
}

/// Sort key: working, then blocked, then failed, then others; newest
/// `updated_at` first within a bucket.
pub fn rank_job(j: &Job) -> (u8, i64) {
    let bucket = match j.state.as_str() {
        "working" => 0,
        "blocked" => 1,
        s if is_error_state(s) => 2,
        _ => 3,
    };
    (bucket, -j.updated_at.unwrap_or(0))
}

/// Scan `~/.claude/jobs/*/state.json` into ranked jobs (empty on any IO failure).
/// Unfiltered: every repo's jobs. Use [`filter_for_repo`] to scope them.
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

/// True when `p` is `root` or lies beneath it (component-wise, so `/r2` is
/// not inside `/r`).
fn path_inside(p: &str, root: &Path) -> bool {
    !p.is_empty() && Path::new(p.trim_end_matches('/')).starts_with(root)
}

/// Does `job` belong to the repo whose checkouts are `roots` (main toplevel +
/// every worktree path)? Any of its worktree path, cwd or origin cwd inside a
/// root counts — except a bare `$HOME`, which says nothing about the repo.
pub fn job_belongs(job: &Job, roots: &[PathBuf]) -> bool {
    let home = dirs::home_dir();
    let candidates = [
        job.worktree_path.as_deref(),
        Some(job.cwd.as_str()),
        job.origin_cwd.as_deref(),
    ];
    candidates.into_iter().flatten().any(|p| {
        let is_home = home
            .as_deref()
            .is_some_and(|h| Path::new(p.trim_end_matches('/')) == h);
        !is_home && roots.iter().any(|r| path_inside(p, r))
    })
}

/// The main checkout's toplevel for any path in a repo (works from inside a
/// linked worktree). `None` outside a repo.
fn common_toplevel(path: &str) -> Option<PathBuf> {
    let out = crate::collect::git::git_ok(&[
        "-C",
        path,
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ])?;
    let dir = PathBuf::from(out.trim());
    match dir.file_name() {
        Some(n) if n == ".git" => dir.parent().map(Path::to_path_buf),
        _ => Some(dir),
    }
}

/// The checkout roots of a repo: `root` itself, the main toplevel, and every worktree.
pub fn repo_roots(root: &str, worktrees: &[Worktree]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if !root.is_empty() {
        roots.push(PathBuf::from(root.trim_end_matches('/')));
        roots.extend(common_toplevel(root));
    }
    roots.extend(
        worktrees
            .iter()
            .map(|w| PathBuf::from(w.path.trim_end_matches('/'))),
    );
    roots.dedup();
    roots
}

/// Keep only the jobs that belong to the repo at `root` (see [`job_belongs`]).
pub fn filter_for_repo(jobs: Vec<Job>, root: &str, worktrees: &[Worktree]) -> Vec<Job> {
    let roots = repo_roots(root, worktrees);
    jobs.into_iter()
        .filter(|j| job_belongs(j, &roots))
        .collect()
}

/// Lowercased alphanumerics only — so "RemoteLogging" and "remote-logging-mcp"
/// can be compared.
fn norm_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Shortest normalised job name eligible for a name match.
const MIN_NAME_MATCH: usize = 4;

/// True when `needle` (normalised) equals a run of consecutive whole tokens
/// of `hay` (split on non-alphanumerics): "remotelogging" matches
/// "remote-logging-mcp" but "log" does not match "logging".
fn token_run_matches(needle: &str, hay: &str) -> bool {
    let toks: Vec<String> = hay
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect();
    (0..toks.len()).any(|i| {
        let mut acc = String::new();
        toks[i..].iter().any(|t| {
            acc.push_str(t);
            acc == needle
        })
    })
}

fn name_matches(j: &Job, wt: &Worktree, roots: &[PathBuf]) -> bool {
    if j.worktree_path.is_some() || j.state == "done" || !job_belongs(j, roots) {
        return false;
    }
    let jn = norm_name(&j.name);
    jn.len() >= MIN_NAME_MATCH
        && (token_run_matches(&jn, &wt.name) || token_run_matches(&jn, &wt.branch))
}

fn synthetic_row(wt: &Worktree) -> Job {
    Job {
        id: format!("worktree:{}", wt.path),
        name: wt.name.clone(),
        state: "no agent".into(),
        cwd: wt.path.clone(),
        worktree_path: Some(wt.path.clone()),
        worktree_branch: Some(wt.branch.clone()),
        ..Default::default()
    }
}

/// [`merge_orphan_worktrees_for`] with the repo root derived from the
/// worktrees themselves (via git). Kept for callers without a root at hand.
pub fn merge_orphan_worktrees(jobs: Vec<Job>, worktrees: &[Worktree]) -> Vec<Job> {
    let root = worktrees
        .iter()
        .find_map(|w| common_toplevel(&w.path))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    merge_orphan_worktrees_for(jobs, worktrees, &root)
}

/// Make every worktree visible in the Jobs pane.
///
/// A job whose `worktreePath` equals a worktree's path owns it outright. For
/// each remaining worktree, try to attribute it to an agent by name: a
/// not-done job of this repo with no worktree of its own whose normalised
/// name (≥ 4 chars) is a whole-token run of the worktree's folder/branch name
/// (agent `RemoteLogging` ↔ worktree `remote-logging-mcp`). Only an
/// **unambiguous** match links. Anything still unowned gets a synthetic
/// "no agent" row so the pane reflects all worktrees on disk.
pub fn merge_orphan_worktrees_for(
    mut jobs: Vec<Job>,
    worktrees: &[Worktree],
    root: &str,
) -> Vec<Job> {
    let roots = repo_roots(root, worktrees);
    let norm_path = |p: &str| p.trim_end_matches('/').to_string();
    let owned: std::collections::HashSet<String> = jobs
        .iter()
        .filter_map(|j| j.worktree_path.as_deref().map(norm_path))
        .collect();
    let orphans: Vec<&Worktree> = worktrees
        .iter()
        .filter(|wt| !owned.contains(&norm_path(&wt.path)))
        .collect();

    let mut attached: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (wi, wt) in orphans.iter().enumerate() {
        let candidates: Vec<usize> = jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| name_matches(j, wt, &roots))
            .map(|(i, _)| i)
            .collect();
        // Link only when exactly one job matches this worktree AND that job
        // doesn't also match another orphan (ambiguity → leave both unlinked).
        if let [ji] = candidates.as_slice() {
            let unique = !orphans
                .iter()
                .enumerate()
                .any(|(oi, o)| oi != wi && name_matches(&jobs[*ji], o, &roots));
            if unique {
                jobs[*ji].worktree_path = Some(wt.path.clone());
                jobs[*ji].worktree_branch = Some(wt.branch.clone());
                attached.insert(wi);
            }
        }
    }

    for (wi, wt) in orphans.iter().enumerate() {
        if !attached.contains(&wi) {
            jobs.push(synthetic_row(wt));
        }
    }
    jobs
}

/// Parse a `timeline.jsonl` body, skipping unparseable lines. `at` is an
/// RFC3339 string in practice; a bare epoch integer is accepted too.
pub fn parse_timeline(s: &str) -> Vec<JobEvent> {
    s.lines()
        .filter_map(|line| {
            let v: Value = serde_json::from_str(line).ok()?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            Some(JobEvent {
                at: epoch_field(v.get("at")).unwrap_or(0),
                state: s("state"),
                detail: s("detail"),
                text: s("text"),
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

    fn wt(name: &str, path: &str, branch: &str) -> Worktree {
        Worktree {
            name: name.into(),
            path: path.into(),
            branch: branch.into(),
            ..Default::default()
        }
    }

    #[test]
    fn appends_orphan_worktrees_only() {
        let owned = parse_state("id", SAMPLE).unwrap(); // owns /repo/.claude/worktrees/x
        let worktrees = vec![
            wt("x", "/repo/.claude/worktrees/x", "worktree-x"), // owned → skipped
            wt("y", "/repo/.claude/worktrees/y", "feature/y"),  // orphan → appended
        ];
        let merged = merge_orphan_worktrees_for(vec![owned], &worktrees, "/repo");
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
            cwd: "/r".into(),
            ..Default::default()
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
        let merged = merge_orphan_worktrees_for(jobs, &worktrees, "/r");

        let rl = merged.iter().find(|j| j.name == "RemoteLogging").unwrap();
        assert_eq!(
            rl.worktree_path.as_deref(),
            Some("/r/.claude/worktrees/remote-logging-mcp")
        );
        assert_eq!(rl.state, "blocked"); // keeps its real state
        assert!(!merged.iter().any(|j| j.name == "remote-logging-mcp"));
        let orphan = merged
            .iter()
            .find(|j| j.name == "admin-backoffice")
            .unwrap();
        assert_eq!(orphan.state, "no agent");
        let dot = merged.iter().find(|j| j.name == "Dotfiles").unwrap();
        assert!(dot.worktree_path.is_none());
    }

    #[test]
    fn name_match_rejects_short_partial_done_and_foreign() {
        let worktrees = vec![wt(
            "logging-ui",
            "/r/.claude/worktrees/logging-ui",
            "feat/x",
        )];
        let link = |j: Job| {
            merge_orphan_worktrees_for(vec![j], &worktrees, "/r")[0]
                .worktree_path
                .is_some()
        };
        assert!(link(bare_job("Logging")));
        assert!(!link(bare_job("log"))); // too short
        assert!(!link(bare_job("Logg"))); // not a whole token
        assert!(!link(Job {
            state: "done".into(),
            ..bare_job("Logging")
        }));
        assert!(!link(Job {
            cwd: "/elsewhere".into(),
            ..bare_job("Logging")
        }));
    }

    #[test]
    fn job_belongs_by_any_path_but_not_home() {
        let roots = vec![PathBuf::from("/r"), PathBuf::from("/tmp/wt")];
        let j = |cwd: &str, origin: Option<&str>, wtp: Option<&str>| Job {
            cwd: cwd.into(),
            origin_cwd: origin.map(str::to_string),
            worktree_path: wtp.map(str::to_string),
            ..Default::default()
        };
        assert!(job_belongs(&j("/r/sub", None, None), &roots));
        assert!(job_belongs(&j("/x", Some("/r"), None), &roots));
        assert!(job_belongs(&j("/x", None, Some("/tmp/wt/")), &roots));
        assert!(!job_belongs(&j("/r2", None, None), &roots));
        let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        let home_roots = vec![PathBuf::from(&home)];
        assert!(!job_belongs(&j(&home, Some(&home), None), &home_roots));
        let kept = filter_for_repo(
            vec![j("/r", None, None), j("/other", None, None)],
            "/r",
            &[],
        );
        assert_eq!(kept.len(), 1);
    }

    const SAMPLE: &str = r#"{"state":"working","tempo":"active","inFlight":{"tasks":2,"queued":1},
        "intent":"do the thing","name":"Governour","cwd":"/repo","originCwd":"/repo",
        "worktreePath":"/repo/.claude/worktrees/x","worktreeBranch":"worktree-x",
        "detail":"running tests","needs":"approve deploy","suggestedReply":"yes",
        "output":{"result":"all done"},"lastTerminalAt":"2026-06-22T13:40:00.000Z",
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
        assert_eq!(j.origin_cwd.as_deref(), Some("/repo"));
        assert_eq!(j.detail, "running tests");
        assert_eq!(j.needs, "approve deploy");
        assert_eq!(j.suggested_reply, "yes");
        assert_eq!(j.output_result, "all done");
        assert!(j.last_terminal_at.unwrap() > j.updated_at.unwrap());
    }

    #[test]
    fn tolerates_missing_optional() {
        let j = parse_state("z", r#"{"state":"idle","output":null}"#).unwrap();
        assert_eq!(j.tasks, 0);
        assert!(j.worktree_path.is_none());
        assert!(j.origin_cwd.is_none());
        assert_eq!(j.output_result, "");
        assert_eq!(j.name, "z"); // falls back to id when name absent
    }

    fn mk(state: &str, updated: i64) -> Job {
        Job {
            id: "x".into(),
            name: "x".into(),
            state: state.into(),
            updated_at: Some(updated),
            ..Default::default()
        }
    }

    #[test]
    fn ranks_active_then_blocked_then_failed() {
        let mut v = [
            mk("done", 100),
            mk("failed", 10),
            mk("working", 50),
            mk("blocked", 80),
        ];
        v.sort_by_key(rank_job);
        let order: Vec<&str> = v.iter().map(|j| j.state.as_str()).collect();
        assert_eq!(order, ["working", "blocked", "failed", "done"]);
        assert!(is_error_state("failed") && is_error_state("Error"));
        assert!(!is_error_state("done"));
        // The raw state is preserved.
        let j = parse_state("f", r#"{"state":"failed"}"#).unwrap();
        assert_eq!(j.state, "failed");
    }

    #[test]
    fn parses_timeline_lines() {
        let s = "{\"at\":\"2026-09-12T02:18:53.459Z\",\"state\":\"working\",\"detail\":\"a\",\"text\":\"x\"}\nbad line\n{\"at\":2,\"state\":\"done\",\"detail\":\"b\",\"text\":\"y\"}";
        let ev = parse_timeline(s);
        assert_eq!(ev.len(), 2); // bad line skipped
        assert_eq!(ev[0].at, iso_to_epoch("2026-09-12T02:18:53.459Z").unwrap());
        assert_eq!(ev[1].at, 2);
        assert_eq!(ev[1].state, "done");
    }
}
