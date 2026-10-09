//! One status row per project, for the picker and the header.
//!
//! Gathered off the UI thread. Per-project work is a handful of cheap git
//! calls; everything Claude-related (activity, jobs, last session) comes from
//! global files read once per pass and attributed by path prefix.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::discover::{self, Suggestion};
use super::model::Registry;
use super::paths::canonical;
use super::store::State;

/// What a job contributes to a project row: the paths it touches and whether
/// it is still running. Decoupled from `collect::jobs::Job` so the overview
/// does not care how jobs are parsed.
#[derive(Debug, Clone)]
pub struct JobRef {
    pub paths: Vec<PathBuf>,
    pub active: bool,
    pub blocked: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectRow {
    pub name: String,
    pub path: PathBuf,
    pub registered: bool,
    pub pinned: bool,
    pub tags: Vec<String>,
    pub exists: bool,
    pub branch: String,
    /// Changed files in the main checkout.
    pub dirty: u32,
    /// Linked worktrees (the main checkout not counted).
    pub worktrees: u32,
    pub jobs_active: u32,
    pub jobs_blocked: u32,
    /// Newest Claude Code transcript activity anywhere in the repo, Unix seconds.
    pub last_active: Option<i64>,
    /// `lastCost` of the most recent Claude Code session started here.
    pub last_cost: Option<f64>,
}

/// Rows for every registered project, then every suggestion.
pub fn gather(reg: &Registry, state: &State, jobs: &[JobRef]) -> Vec<ProjectRow> {
    let home = dirs::home_dir().unwrap_or_default();
    let activity = discover::activity_by_cwd(&home.join(".claude/projects"));
    let costs = last_costs(&home.join(".claude.json"));
    let suggestions = discover::suggestions(reg, state);

    let registered = reg.ordered().into_iter().map(|p| ProjectRow {
        name: p.name.clone(),
        path: p.path.clone(),
        registered: true,
        pinned: p.pinned,
        tags: p.tags.clone(),
        exists: p.exists(),
        ..ProjectRow::default()
    });
    let suggested = suggestions.into_iter().map(|s: Suggestion| ProjectRow {
        name: s.name,
        path: s.path,
        exists: true,
        last_active: s.last_active,
        ..ProjectRow::default()
    });

    registered
        .chain(suggested)
        .map(|mut row| {
            let root = canonical(&row.path);
            row.last_active = row.last_active.max(newest_under(&root, &activity));
            row.last_cost = costs.get(&root).copied();
            tally_jobs(&mut row, &root, jobs);
            if row.exists {
                fill_git(&mut row, &root);
            }
            row
        })
        .collect()
}

fn newest_under(root: &Path, activity: &[(PathBuf, i64)]) -> Option<i64> {
    activity
        .iter()
        .filter(|(cwd, _)| canonical(cwd).starts_with(root))
        .map(|(_, t)| *t)
        .max()
}

fn tally_jobs(row: &mut ProjectRow, root: &Path, jobs: &[JobRef]) {
    for j in jobs
        .iter()
        .filter(|j| j.paths.iter().any(|p| inside(p, root)))
    {
        row.jobs_active += u32::from(j.active);
        row.jobs_blocked += u32::from(j.blocked);
    }
}

/// `p` is `root` or below it. Paths are compared canonically when they exist
/// (worktrees under symlinked dirs), lexically otherwise (deleted worktrees).
pub fn inside(p: &Path, root: &Path) -> bool {
    p.starts_with(root) || canonical(p).starts_with(root)
}

fn fill_git(row: &mut ProjectRow, root: &Path) {
    row.branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    row.dirty = git(root, &["status", "--porcelain"])
        .map(|s| s.lines().count() as u32)
        .unwrap_or(0);
    row.worktrees = git(root, &["worktree", "list", "--porcelain"])
        .map(|s| {
            s.lines()
                .filter(|l| l.starts_with("worktree "))
                .count()
                .saturating_sub(1) as u32
        })
        .unwrap_or(0);
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = crate::util::git_cmd()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `~/.claude.json` → `projects.<dir>.lastCost`, keyed by repo root. Several
/// directories inside one repo keep the newest-started session's figure.
fn last_costs(path: &Path) -> HashMap<PathBuf, f64> {
    let mut best: HashMap<PathBuf, (i64, f64)> = HashMap::new();
    let Some(v) = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return HashMap::new();
    };
    let Some(map) = v.get("projects").and_then(|m| m.as_object()) else {
        return HashMap::new();
    };
    for (dir, entry) in map {
        let (Some(cost), started) = (
            entry.get("lastCost").and_then(|c| c.as_f64()),
            entry
                .get("lastStartTime")
                .and_then(|t| t.as_i64())
                .unwrap_or(0),
        ) else {
            continue;
        };
        let dir = Path::new(dir);
        let key = super::paths::repo_root_of(dir).map_or_else(|| canonical(dir), |r| canonical(&r));
        let slot = best.entry(key).or_insert((started, cost));
        if started >= slot.0 {
            *slot = (started, cost);
        }
    }
    best.into_iter().map(|(k, (_, c))| (k, c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::testutil::temp_repo;

    #[test]
    fn git_fields_and_jobs_are_attributed_by_path() {
        let repo = temp_repo("overview");
        std::fs::write(repo.join("new.txt"), "x").unwrap();
        let wt = repo.join(".claude/worktrees/w1");
        crate::util::git_cmd()
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "-q", "-b", "w1"])
            .arg(&wt)
            .status()
            .unwrap();
        let mut row = ProjectRow {
            path: repo.clone(),
            exists: true,
            ..ProjectRow::default()
        };
        let root = canonical(&repo);
        fill_git(&mut row, &root);
        assert_eq!(row.branch, "main");
        assert_eq!(row.dirty, 2); // new.txt + untracked .claude/
        assert_eq!(row.worktrees, 1);

        let jobs = vec![
            JobRef {
                paths: vec![wt.clone()],
                active: true,
                blocked: false,
            },
            JobRef {
                paths: vec![PathBuf::from("/elsewhere")],
                active: true,
                blocked: false,
            },
            JobRef {
                paths: vec![repo.clone()],
                active: false,
                blocked: true,
            },
        ];
        tally_jobs(&mut row, &root, &jobs);
        assert_eq!((row.jobs_active, row.jobs_blocked), (1, 1));
    }

    #[test]
    fn last_cost_keeps_the_newest_session_per_repo() {
        let repo = temp_repo("overview-cost");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        let f = repo.join("claude.json");
        let body = serde_json::json!({"projects": {
            repo.to_str().unwrap(): {"lastCost": 1.5, "lastStartTime": 10},
            repo.join("sub").to_str().unwrap(): {"lastCost": 9.0, "lastStartTime": 5},
        }});
        std::fs::write(&f, body.to_string()).unwrap();
        let got = last_costs(&f);
        assert_eq!(got.get(&canonical(&repo)), Some(&1.5));
    }
}
