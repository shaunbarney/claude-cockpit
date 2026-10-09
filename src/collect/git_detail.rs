//! Per-worktree git detail: changed files, commit log, merge readiness, file diffs.

use crate::collect::git::{base_ref, git, git_ok, git_with_status};

/// One changed file. `path` is the current path; `old_path` is set for a
/// rename/copy. `untracked` files are new and not yet added (their line count
/// is reported as `added`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
    pub staged: bool,
    pub old_path: Option<String>,
    pub untracked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitRow {
    pub short: String,
    pub subject: String,
    pub age: String,
}

/// Merge readiness of a worktree's branch against the base ref.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MergeStatus {
    /// Commits to merge, merges cleanly, base hasn't moved.
    Clean,
    /// Merging would conflict in these files.
    Conflicts(Vec<String>),
    /// Commits to merge and merges cleanly, but the base has `n` new commits.
    Behind(u32),
    /// No commits of its own and identical to the base.
    UpToDate,
    /// No commits of its own; the base is `n` commits ahead (stale branch).
    NothingToMerge(u32),
    /// `n` uncommitted changes (incl. untracked) — commit before judging a merge.
    Dirty(u32),
    /// Couldn't determine (missing checkout, no base ref, merge-tree failed).
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub enum DiffMode {
    Unstaged,
    Staged,
    Committed,
}

#[derive(Debug, Clone, Default)]
pub struct WorktreeDetail {
    pub name: String,
    pub branch: String,
    pub path: String,
    /// The ref everything is compared against, e.g. `origin/main`.
    pub base: String,
    pub committed_files: Vec<FileChange>,
    pub uncommitted_files: Vec<FileChange>,
    pub commits: Vec<CommitRow>,
    pub merge: MergeStatus,
}

/// Parse `git diff --numstat -z` output. Binary files show "-\t-" → 0/0.
///
/// Normal records are `A\tD\tPATH\0`; renames/copies are `A\tD\t\0OLD\0NEW\0`.
/// With `-z` paths are verbatim (no C-quoting, no `a => b` brace form).
pub fn parse_numstat(s: &str, staged: bool) -> Vec<FileChange> {
    let mut out = Vec::new();
    let mut it = s.split('\0');
    while let Some(rec) = it.next() {
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let a = a.trim_start_matches('\n');
        let (path, old_path) = if p.is_empty() {
            let old = it.next().unwrap_or("").to_string();
            (it.next().unwrap_or("").to_string(), Some(old))
        } else {
            (p.to_string(), None)
        };
        if path.is_empty() {
            continue;
        }
        out.push(FileChange {
            path,
            added: a.parse().unwrap_or(0),
            deleted: d.parse().unwrap_or(0),
            staged,
            old_path,
            untracked: false,
        });
    }
    out
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

/// Pure merge classification.
///
/// `ahead`/`behind` are `None` when they couldn't be counted; `conflicts` is
/// `None` when merge-tree failed or wasn't run. Precedence: uncommitted work →
/// nothing of its own to merge → unknown → conflicts → behind → clean.
pub fn classify_merge(
    ahead: Option<u32>,
    behind: Option<u32>,
    dirty: u32,
    conflicts: Option<Vec<String>>,
) -> MergeStatus {
    let (Some(ahead), Some(behind)) = (ahead, behind) else {
        return MergeStatus::Unknown;
    };
    if dirty > 0 {
        return MergeStatus::Dirty(dirty);
    }
    if ahead == 0 {
        return match behind {
            0 => MergeStatus::UpToDate,
            n => MergeStatus::NothingToMerge(n),
        };
    }
    match conflicts {
        None => MergeStatus::Unknown,
        Some(c) if !c.is_empty() => MergeStatus::Conflicts(c),
        Some(_) if behind > 0 => MergeStatus::Behind(behind),
        Some(_) => MergeStatus::Clean,
    }
}

/// Conflicted paths from `merge-tree --write-tree --name-only` stdout: the
/// lines between the tree OID and the first blank line, deduplicated.
pub fn parse_merge_tree_conflicts(out: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for line in out.lines().skip(1).take_while(|l| !l.is_empty()) {
        if !files.iter().any(|f| f == line) {
            files.push(line.to_string());
        }
    }
    files
}

/// Run merge-tree (git >= 2.38). Exit 0 = clean, 1 = conflicts, else unknown.
fn merge_tree_conflicts(path: &str, base: &str) -> Option<Vec<String>> {
    let args = [
        "-C",
        path,
        "merge-tree",
        "--write-tree",
        "--name-only",
        base,
        "HEAD",
    ];
    match git_with_status(&args)? {
        (0, _) => Some(vec![]),
        (1, out) => {
            let files = parse_merge_tree_conflicts(&out);
            Some(if files.is_empty() {
                vec!["(unknown)".to_string()]
            } else {
                files
            })
        }
        _ => None,
    }
}

fn count(args: &[&str]) -> Option<u32> {
    git_ok(args)?.trim().parse().ok()
}

/// Merge readiness against an explicit base ref (shells out).
pub fn merge_status_against(path: &str, base: &str) -> MergeStatus {
    let ahead = count(&["-C", path, "rev-list", "--count", &format!("{base}..HEAD")]);
    let behind = count(&["-C", path, "rev-list", "--count", &format!("HEAD..{base}")]);
    let dirty = git_ok(&["-C", path, "status", "--porcelain"])
        .map(|s| s.lines().count() as u32)
        .unwrap_or(0);
    let needs_tree = dirty == 0 && ahead.is_some_and(|a| a > 0);
    let conflicts = if needs_tree {
        merge_tree_conflicts(path, base)
    } else {
        None
    };
    classify_merge(ahead, behind, dirty, conflicts)
}

/// Best-effort merge readiness vs the repo's base ref (see [`base_ref`]).
pub fn merge_status(path: &str) -> MergeStatus {
    merge_status_against(path, &base_ref(path))
}

/// Count lines of a small text file (0 for binary/huge/unreadable files).
fn text_line_count(p: &std::path::Path) -> u32 {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    let small = std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES);
    let Some(bytes) = small.then(|| std::fs::read(p).ok()).flatten() else {
        return 0;
    };
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return 0;
    }
    let nl = bytes.iter().filter(|&&b| b == b'\n').count();
    let tail = usize::from(bytes.last().is_some_and(|&b| b != b'\n'));
    (nl + tail) as u32
}

/// Untracked (not ignored) files, with their line counts as `added`.
pub fn untracked_files(path: &str) -> Vec<FileChange> {
    let out = git(&[
        "-C",
        path,
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ]);
    out.split('\0')
        .filter(|s| !s.is_empty())
        .map(|rel| FileChange {
            path: rel.to_string(),
            added: text_line_count(&std::path::Path::new(path).join(rel)),
            untracked: true,
            ..Default::default()
        })
        .collect()
}

/// Gather full detail for a worktree at `path` (name/branch from caller).
pub fn worktree_detail(path: &str, name: &str, branch: &str) -> WorktreeDetail {
    let base = base_ref(path);
    let range = format!("{base}...HEAD");
    let numstat = |extra: &[&str]| {
        let mut args = vec!["-C", path, "diff", "--numstat", "-z", "-M"];
        args.extend_from_slice(extra);
        git(&args)
    };
    let committed_files = parse_numstat(&numstat(&[&range]), false);
    let mut uncommitted_files = parse_numstat(&numstat(&["--cached"]), true);
    uncommitted_files.extend(parse_numstat(&numstat(&[]), false));
    uncommitted_files.extend(untracked_files(path));
    let commits = parse_commit_log(&git(&[
        "-C",
        path,
        "log",
        "-20",
        "--format=%h%x1f%s%x1f%cr",
        &format!("{base}..HEAD"),
    ]));
    let merge = merge_status_against(path, &base);
    WorktreeDetail {
        name: name.to_string(),
        branch: branch.to_string(),
        path: path.to_string(),
        base,
        committed_files,
        uncommitted_files,
        commits,
        merge,
    }
}

/// The raw diff lines for one file in the given mode (shells out).
/// Equivalent to [`file_diff_paths`] without rename information.
pub fn file_diff(path: &str, file: &str, mode: DiffMode) -> Vec<String> {
    file_diff_paths(path, file, None, mode)
}

/// The raw diff lines for one file; pass `old_path` (from
/// [`FileChange::old_path`]) so a rename diffs as a rename rather than a whole
/// new file. An untracked file in `Unstaged` mode diffs against `/dev/null`.
pub fn file_diff_paths(
    path: &str,
    file: &str,
    old_path: Option<&str>,
    mode: DiffMode,
) -> Vec<String> {
    let range;
    let mut args = vec!["-C", path, "diff", "-M"];
    match mode {
        DiffMode::Staged => args.push("--cached"),
        DiffMode::Committed => {
            range = format!("{}...HEAD", base_ref(path));
            args.push(&range);
        }
        DiffMode::Unstaged => {}
    }
    args.push("--");
    if let Some(old) = old_path {
        args.push(old);
    }
    args.push(file);
    let out = git(&args);
    if out.is_empty() && matches!(mode, DiffMode::Unstaged) && is_untracked(path, file) {
        return git(&["-C", path, "diff", "--no-index", "--", "/dev/null", file])
            .lines()
            .map(str::to_string)
            .collect();
    }
    out.lines().map(str::to_string).collect()
}

fn is_untracked(path: &str, file: &str) -> bool {
    let tracked = git_ok(&["-C", path, "ls-files", "--error-unmatch", "--", file]).is_some();
    !tracked && std::path::Path::new(path).join(file).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::git::test_repo::*;

    #[test]
    fn parses_numstat_z() {
        let s = "12\t3\tsrc/a.rs\x000\t5\tsrc/b.rs\x00-\t-\tbin.png\x00";
        let f = parse_numstat(s, false);
        assert_eq!(f.len(), 3);
        assert_eq!(
            f[0],
            FileChange {
                path: "src/a.rs".into(),
                added: 12,
                deleted: 3,
                ..Default::default()
            }
        );
        assert_eq!(f[2].added, 0); // binary "-" → 0
        assert_eq!(f[2].deleted, 0);
    }

    #[test]
    fn parses_numstat_z_renames_and_unicode() {
        let s = "1\t1\t\x00src/{old}.rs\x00src/new name.rs\x002\t0\tdocs/caf\u{e9}.md\x00";
        let f = parse_numstat(s, true);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].path, "src/new name.rs");
        assert_eq!(f[0].old_path.as_deref(), Some("src/{old}.rs"));
        assert_eq!(f[1].path, "docs/caf\u{e9}.md");
        assert!(f[1].staged);
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
        let ok = Some(vec![]);
        assert_eq!(
            classify_merge(Some(0), Some(0), 0, None),
            MergeStatus::UpToDate
        );
        assert_eq!(
            classify_merge(Some(0), Some(4), 0, None),
            MergeStatus::NothingToMerge(4)
        );
        assert_eq!(
            classify_merge(Some(3), Some(0), 0, ok.clone()),
            MergeStatus::Clean
        );
        assert_eq!(
            classify_merge(Some(3), Some(2), 0, ok.clone()),
            MergeStatus::Behind(2)
        );
        assert_eq!(
            classify_merge(Some(3), Some(0), 2, ok),
            MergeStatus::Dirty(2)
        );
        assert_eq!(
            classify_merge(Some(3), Some(0), 0, None),
            MergeStatus::Unknown
        );
        assert_eq!(classify_merge(None, Some(0), 0, None), MergeStatus::Unknown);
        assert!(matches!(
            classify_merge(Some(3), Some(0), 0, Some(vec!["x".into()])),
            MergeStatus::Conflicts(_)
        ));
    }

    #[test]
    fn parses_merge_tree_name_only() {
        let out = "abc123\na.txt\na.txt\nb.txt\n\nAuto-merging a.txt\nCONFLICT (content): ...\n";
        assert_eq!(parse_merge_tree_conflicts(out), vec!["a.txt", "b.txt"]);
    }

    /// Repo on `master` (exercises base detection) with a linked worktree.
    fn repo_with_wt(tag: &str) -> (TempDir, TempDir, String) {
        let r = init(tag, "master");
        let w = TempDir::new(&format!("{tag}-wt"));
        let wp = w.path().join("w").to_string_lossy().into_owned();
        run(r.path(), &["worktree", "add", "-q", "-b", "feat", &wp]);
        (r, w, wp)
    }

    #[test]
    fn real_merge_conflict_lists_files() {
        let (r, _w, wp) = repo_with_wt("mconf");
        let wpath = std::path::Path::new(&wp);
        write(wpath, "a.txt", "branch\n");
        commit_all(wpath, "branch edit");
        write(r.path(), "a.txt", "master\n");
        commit_all(r.path(), "master edit");
        assert_eq!(
            merge_status(&wp),
            MergeStatus::Conflicts(vec!["a.txt".into()])
        );
        // Uncommitted work takes precedence.
        write(wpath, "new.txt", "u\n");
        assert_eq!(merge_status(&wp), MergeStatus::Dirty(1));
    }

    #[test]
    fn detail_handles_renames_untracked_and_master_base() {
        let (_r, _w, wp) = repo_with_wt("mdet");
        let wpath = std::path::Path::new(&wp);
        run(wpath, &["mv", "a.txt", "renamed é.txt"]);
        commit_all(wpath, "rename");
        write(wpath, "u.txt", "1\n2\n3");
        let d = worktree_detail(&wp, "w", "feat");
        assert_eq!(d.base, "master");
        assert_eq!(d.committed_files.len(), 1, "{:?}", d.committed_files);
        let c = &d.committed_files[0];
        assert_eq!(c.path, "renamed é.txt");
        assert_eq!(c.old_path.as_deref(), Some("a.txt"));
        let u = d.uncommitted_files.iter().find(|f| f.untracked).unwrap();
        assert_eq!((u.path.as_str(), u.added), ("u.txt", 3));
        assert_eq!(d.merge, MergeStatus::Dirty(1));
        assert_eq!(d.commits.len(), 1);
        let diff = file_diff_paths(&wp, &c.path, c.old_path.as_deref(), DiffMode::Committed);
        assert!(
            diff.iter().any(|l| l.starts_with("rename from a.txt")),
            "{diff:?}"
        );
        let ud = file_diff(&wp, "u.txt", DiffMode::Unstaged);
        assert!(ud.iter().any(|l| l == "+3"), "{ud:?}");
    }

    #[test]
    fn nothing_to_merge_when_base_moved() {
        let (r, _w, wp) = repo_with_wt("mstale");
        write(r.path(), "z.txt", "z\n");
        commit_all(r.path(), "advance");
        assert_eq!(merge_status(&wp), MergeStatus::NothingToMerge(1));
    }
}
