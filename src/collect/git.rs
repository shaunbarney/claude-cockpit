//! Git data gathering for worktree health.

/// Parse `git diff --shortstat` output into `(insertions, deletions)`.
/// Mirrors the Python `shortstat()` regex behaviour: missing numbers are 0.
pub fn parse_shortstat(s: &str) -> (u32, u32) {
    let grab = |needle: &str| -> u32 {
        s.split(',')
            .find(|part| part.contains(needle))
            .and_then(|part| part.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    (grab("insertion"), grab("deletion"))
}

use std::process::Command;

/// One worktree's health snapshot.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub ahead: u32,
    pub dirty: u32,
    pub committed: (u32, u32),
    pub uncommitted: (u32, u32),
    pub age: String,
}

/// Run a git command, returning trimmed stdout (empty string on any failure).
pub(crate) fn git(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Repository top-level directory.
pub fn repo_root() -> String {
    git(&["rev-parse", "--show-toplevel"]).trim().to_string()
}

/// `git fetch origin --quiet` (best-effort; ignores failure, same as Python).
pub fn fetch_origin(root: &str) {
    let _ = Command::new("git")
        .args(["-C", root, "fetch", "origin", "--quiet"])
        .output();
}

/// Gather every non-main worktree, ranked: ahead first, then dirty, then clean.
pub fn gather_worktrees(root: &str) -> Vec<Worktree> {
    let porc = git(&["-C", root, "worktree", "list", "--porcelain"]);
    let mut paths: Vec<(String, Option<String>)> = Vec::new();
    let mut cur_path: Option<String> = None;
    let mut cur_branch: Option<String> = None;
    for line in porc.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(prev) = cur_path.take() {
                paths.push((prev, cur_branch.take()));
            }
            cur_path = Some(p.to_string());
            cur_branch = None;
        } else if let Some(b) = line.strip_prefix("branch ") {
            cur_branch = Some(b.replace("refs/heads/", ""));
        }
    }
    if let Some(prev) = cur_path.take() {
        paths.push((prev, cur_branch.take()));
    }

    let mut rows: Vec<Worktree> = Vec::new();
    for (path, branch) in paths {
        let b = match branch {
            Some(ref b) if b != "main" => b.clone(),
            _ => continue,
        };
        let ahead: u32 = git(&["-C", &path, "rev-list", "--count", "main..HEAD"])
            .trim()
            .parse()
            .unwrap_or(0);
        let dirty = git(&["-C", &path, "status", "--porcelain"]).lines().count() as u32;
        let committed =
            parse_shortstat(&git(&["-C", &path, "diff", "--shortstat", "main...HEAD"]));
        let uncommitted = parse_shortstat(&git(&["-C", &path, "diff", "--shortstat", "HEAD"]));
        let age = git(&["-C", &path, "log", "-1", "--format=%cr"]).trim().to_string();
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        rows.push(Worktree { name, path: path.clone(), branch: b, ahead, dirty, committed, uncommitted, age });
    }

    rows.sort_by_key(rank);
    rows
}

/// Sort key matching the Python `rank()`: (bucket, tiebreak).
/// bucket 0 = ahead (most ahead first), 1 = dirty (most dirty first), 2 = clean.
pub fn rank(r: &Worktree) -> (u8, i64) {
    if r.ahead > 0 {
        (0, -(r.ahead as i64))
    } else if r.dirty > 0 {
        (1, -(r.dirty as i64))
    } else {
        (2, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_insertions_and_deletions() {
        let s = " 3 files changed, 12 insertions(+), 4 deletions(-)";
        assert_eq!(parse_shortstat(s), (12, 4));
    }

    #[test]
    fn handles_missing_sides_and_empty() {
        assert_eq!(parse_shortstat(" 1 file changed, 5 insertions(+)"), (5, 0));
        assert_eq!(parse_shortstat(" 1 file changed, 2 deletions(-)"), (0, 2));
        assert_eq!(parse_shortstat(""), (0, 0));
    }

    fn wt(ahead: u32, dirty: u32) -> Worktree {
        Worktree {
            name: "x".into(),
            path: String::new(),
            branch: String::new(),
            ahead,
            dirty,
            committed: (0, 0),
            uncommitted: (0, 0),
            age: String::new(),
        }
    }

    #[test]
    fn ranks_ahead_then_dirty_then_clean() {
        let mut v = vec![wt(0, 0), wt(0, 3), wt(2, 0), wt(5, 0)];
        v.sort_by_key(rank);
        let order: Vec<(u32, u32)> = v.iter().map(|w| (w.ahead, w.dirty)).collect();
        assert_eq!(order, vec![(5, 0), (2, 0), (0, 3), (0, 0)]);
    }
}
