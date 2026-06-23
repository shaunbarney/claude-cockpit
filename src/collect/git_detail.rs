//! Per-worktree git detail: changed files, commit log, merge readiness, file diffs.

use crate::collect::git::git;

#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitRow {
    pub short: String,
    pub subject: String,
    pub age: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeStatus {
    Clean,
    Conflicts(Vec<String>),
    Behind(u32),
    UpToDate,
}

#[derive(Debug, Clone, Copy)]
pub enum DiffMode {
    Unstaged,
    Staged,
    Committed,
}

#[derive(Debug, Clone)]
pub struct WorktreeDetail {
    pub name: String,
    pub branch: String,
    pub path: String,
    pub committed_files: Vec<FileChange>,
    pub uncommitted_files: Vec<FileChange>,
    pub commits: Vec<CommitRow>,
    pub merge: MergeStatus,
}

/// Parse `git diff --numstat` output. Binary files show "-\t-" → 0/0.
pub fn parse_numstat(s: &str, staged: bool) -> Vec<FileChange> {
    s.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let a = parts.next()?;
            let d = parts.next()?;
            let path = parts.next()?.to_string();
            if path.is_empty() {
                return None;
            }
            Some(FileChange {
                path,
                added: a.parse().unwrap_or(0),
                deleted: d.parse().unwrap_or(0),
                staged,
            })
        })
        .collect()
}

/// Parse `git log --format=%h%x1f%s%x1f%cr` (unit-separator delimited).
pub fn parse_commit_log(s: &str) -> Vec<CommitRow> {
    s.lines()
        .filter_map(|line| {
            let mut p = line.split('\u{1f}');
            let short = p.next()?.to_string();
            if short.is_empty() {
                return None;
            }
            Some(CommitRow {
                short,
                subject: p.next().unwrap_or("").to_string(),
                age: p.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Pure merge classification from counts + detected conflict files.
pub fn classify_merge(ahead: u32, behind: u32, conflicts: Vec<String>) -> MergeStatus {
    if ahead == 0 {
        MergeStatus::UpToDate
    } else if !conflicts.is_empty() {
        MergeStatus::Conflicts(conflicts)
    } else if behind > 0 {
        MergeStatus::Behind(behind)
    } else {
        MergeStatus::Clean
    }
}

/// Best-effort merge readiness vs `main` (shells out).
pub fn merge_status(path: &str) -> MergeStatus {
    let ahead: u32 = git(&["-C", path, "rev-list", "--count", "main..HEAD"])
        .trim()
        .parse()
        .unwrap_or(0);
    let behind: u32 = git(&["-C", path, "rev-list", "--count", "HEAD..main"])
        .trim()
        .parse()
        .unwrap_or(0);
    // git >= 2.38: merge-tree --write-tree prints CONFLICT lines on conflict. Best-effort; empty on older git.
    let mt = git(&["-C", path, "merge-tree", "--write-tree", "--name-only", "main", "HEAD"]);
    let conflicts: Vec<String> = mt
        .lines()
        .filter(|l| l.contains("CONFLICT"))
        .map(|l| l.to_string())
        .collect();
    classify_merge(ahead, behind, conflicts)
}

/// Gather full detail for a worktree at `path` (name/branch from caller).
pub fn worktree_detail(path: &str, name: &str, branch: &str) -> WorktreeDetail {
    let committed_files =
        parse_numstat(&git(&["-C", path, "diff", "--numstat", "main...HEAD"]), false);
    let mut uncommitted_files =
        parse_numstat(&git(&["-C", path, "diff", "--numstat", "--cached"]), true);
    uncommitted_files.extend(parse_numstat(&git(&["-C", path, "diff", "--numstat"]), false));
    let commits = parse_commit_log(&git(&[
        "-C",
        path,
        "log",
        "-20",
        "--format=%h%x1f%s%x1f%cr",
        "main..HEAD",
    ]));
    let merge = merge_status(path);
    WorktreeDetail {
        name: name.to_string(),
        branch: branch.to_string(),
        path: path.to_string(),
        committed_files,
        uncommitted_files,
        commits,
        merge,
    }
}

/// The raw diff lines for one file in the given mode (shells out).
pub fn file_diff(path: &str, file: &str, mode: DiffMode) -> Vec<String> {
    let mut args = vec!["-C", path, "diff"];
    match mode {
        DiffMode::Staged => args.push("--cached"),
        DiffMode::Committed => args.push("main...HEAD"),
        DiffMode::Unstaged => {}
    }
    args.push("--");
    args.push(file);
    git(&args).lines().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat() {
        let s = "12\t3\tsrc/a.rs\n0\t5\tsrc/b.rs\n-\t-\tbin.png";
        let f = parse_numstat(s, false);
        assert_eq!(f.len(), 3);
        assert_eq!(
            f[0],
            FileChange { path: "src/a.rs".into(), added: 12, deleted: 3, staged: false }
        );
        assert_eq!(f[2].added, 0); // binary "-" → 0
        assert_eq!(f[2].deleted, 0);
    }

    #[test]
    fn parses_commit_log() {
        let s = "abc123\u{1f}fix: thing\u{1f}2 hours ago\n";
        let c = parse_commit_log(s);
        assert_eq!(c[0].short, "abc123");
        assert_eq!(c[0].subject, "fix: thing");
        assert_eq!(c[0].age, "2 hours ago");
    }

    #[test]
    fn classifies_merge() {
        assert_eq!(classify_merge(0, 0, vec![]), MergeStatus::UpToDate);
        assert_eq!(classify_merge(3, 0, vec![]), MergeStatus::Clean);
        assert_eq!(classify_merge(3, 2, vec![]), MergeStatus::Behind(2));
        assert!(matches!(classify_merge(3, 0, vec!["x".into()]), MergeStatus::Conflicts(_)));
    }
}
