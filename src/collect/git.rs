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

/// One worktree's health snapshot.
///
/// `branch` is the checked-out branch, or `"(detached <sha7>)"` when `detached`.
/// `missing` means the checkout can't be inspected (folder gone, git marks it
/// `prunable`, or `git status` fails there); its counts are then all zero and
/// must not be read as "clean". `locked` mirrors `git worktree lock`.
#[derive(Debug, Clone, Default)]
pub struct Worktree {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub ahead: u32,
    pub dirty: u32,
    pub committed: (u32, u32),
    pub uncommitted: (u32, u32),
    pub age: String,
    pub detached: bool,
    pub missing: bool,
    pub locked: bool,
}

/// Run a git command, returning stdout (empty string on any failure, and the
/// exit status is ignored). Use [`git_ok`] where a failure must not be
/// mistaken for an empty/zero answer.
pub(crate) fn git(args: &[&str]) -> String {
    crate::util::git_cmd()
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Run a git command, returning `(exit code, stdout)`; `None` if git could not
/// be spawned. A signal-terminated process reports code `-1`.
pub(crate) fn git_with_status(args: &[&str]) -> Option<(i32, String)> {
    let o = crate::util::git_cmd().args(args).output().ok()?;
    Some((
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
    ))
}

/// Run a git command, returning stdout only when it exits 0.
pub(crate) fn git_ok(args: &[&str]) -> Option<String> {
    match git_with_status(args)? {
        (0, out) => Some(out),
        _ => None,
    }
}

/// Parse a count printed by `rev-list --count` (None on failure).
fn git_count(args: &[&str]) -> Option<u32> {
    git_ok(args)?.trim().parse().ok()
}

/// Identity of the repo the cockpit is pointed at, for the header bar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoInfo {
    pub name: String,
    pub branch: String,
}

/// Read the repo's folder name and checked-out branch.
///
/// Best-effort like every other collector: outside a repo, or with a detached
/// HEAD, the fields come back empty and the header simply shows less.
pub fn repo_info(root: &str) -> RepoInfo {
    let name = std::path::Path::new(root)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let branch = git(&["-C", root, "rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string();
    RepoInfo { name, branch }
}

/// Repository top-level directory.
pub fn repo_root() -> String {
    git(&["rev-parse", "--show-toplevel"]).trim().to_string()
}

/// `git fetch origin --quiet` (best-effort; ignores failure, same as Python).
pub fn fetch_origin(root: &str) {
    let _ = crate::util::git_cmd()
        .args(["-C", root, "fetch", "origin", "--quiet"])
        .output();
}

fn ref_exists(root: &str, refname: &str) -> bool {
    git_ok(&["-C", root, "rev-parse", "--verify", "--quiet", refname]).is_some()
}

/// True when `name` exists as a local branch or as `origin/<name>`.
fn branch_exists(root: &str, name: &str) -> bool {
    ref_exists(root, &format!("refs/heads/{name}"))
        || ref_exists(root, &format!("refs/remotes/origin/{name}"))
}

/// The repo's integration branch name (e.g. `main`, `master`, `develop`).
///
/// Resolution order: `origin/HEAD` → `init.defaultBranch` (if that branch
/// exists) → `main` → `master`; falls back to `"main"` when nothing matches.
pub fn base_branch(root: &str) -> String {
    let origin_head = git_ok(&[
        "-C",
        root,
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]);
    if let Some(b) = origin_head.as_deref().map(str::trim) {
        let b = b.strip_prefix("origin/").unwrap_or(b);
        if !b.is_empty() {
            return b.to_string();
        }
    }
    let configured = git_ok(&["-C", root, "config", "--get", "init.defaultBranch"]);
    if let Some(b) = configured.as_deref().map(str::trim) {
        if !b.is_empty() && branch_exists(root, b) {
            return b.to_string();
        }
    }
    ["main", "master"]
        .into_iter()
        .find(|b| branch_exists(root, b))
        .unwrap_or("main")
        .to_string()
}

/// The ref worktrees are compared against: `origin/<base>` when that remote
/// branch exists (what a PR merges into), else the local base branch.
pub fn base_ref(root: &str) -> String {
    let base = base_branch(root);
    let remote = format!("origin/{base}");
    if ref_exists(root, &format!("refs/remotes/{remote}")) {
        remote
    } else {
        base
    }
}

/// One `git worktree list --porcelain` record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PorcelainEntry {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Parse `git worktree list --porcelain` (records separated by blank lines;
/// the first record is always the main worktree).
pub fn parse_worktree_porcelain(s: &str) -> Vec<PorcelainEntry> {
    let mut out: Vec<PorcelainEntry> = Vec::new();
    for line in s.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            out.push(PorcelainEntry {
                path: p.to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        let (key, val) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "HEAD" => cur.head = val.to_string(),
            "branch" => cur.branch = Some(val.trim_start_matches("refs/heads/").to_string()),
            "bare" => cur.bare = true,
            "detached" => cur.detached = true,
            "locked" => cur.locked = true,
            "prunable" => cur.prunable = true,
            _ => {}
        }
    }
    out
}

fn branch_label(e: &PorcelainEntry) -> String {
    match &e.branch {
        Some(b) if !e.detached => b.clone(),
        _ => format!("(detached {})", e.head.chars().take(7).collect::<String>()),
    }
}

/// Inspect one linked worktree (shells out several times).
fn inspect_worktree(e: &PorcelainEntry, base: &str) -> Worktree {
    let path = e.path.as_str();
    let name = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let mut wt = Worktree {
        name,
        path: path.to_string(),
        branch: branch_label(e),
        detached: e.detached || e.branch.is_none(),
        locked: e.locked,
        missing: e.prunable || !std::path::Path::new(path).is_dir(),
        ..Default::default()
    };
    if wt.missing {
        return wt;
    }
    let Some(status) = git_ok(&["-C", path, "status", "--porcelain"]) else {
        wt.missing = true;
        return wt;
    };
    wt.dirty = status.lines().count() as u32;
    wt.ahead =
        git_count(&["-C", path, "rev-list", "--count", &format!("{base}..HEAD")]).unwrap_or(0);
    let range = format!("{base}...HEAD");
    wt.committed = parse_shortstat(&git(&["-C", path, "diff", "--shortstat", &range]));
    wt.uncommitted = parse_shortstat(&git(&["-C", path, "diff", "--shortstat", "HEAD"]));
    wt.age = git(&["-C", path, "log", "-1", "--format=%cr"])
        .trim()
        .to_string();
    wt
}

/// Gather every linked worktree (the main checkout and bare entries are
/// skipped), ranked: ahead first, then dirty, then clean, then missing.
pub fn gather_worktrees(root: &str) -> Vec<Worktree> {
    let porc = git(&["-C", root, "worktree", "list", "--porcelain"]);
    let base = base_ref(root);
    let mut rows: Vec<Worktree> = parse_worktree_porcelain(&porc)
        .iter()
        .skip(1)
        .filter(|e| !e.bare)
        .map(|e| inspect_worktree(e, &base))
        .collect();
    rows.sort_by_key(rank);
    rows
}

/// Sort key matching the Python `rank()`: (bucket, tiebreak).
/// bucket 0 = ahead (most ahead first), 1 = dirty (most dirty first), 2 = clean,
/// 3 = missing (can't be inspected).
pub fn rank(r: &Worktree) -> (u8, i64) {
    if r.missing {
        (3, 0)
    } else if r.ahead > 0 {
        (0, -(r.ahead as i64))
    } else if r.dirty > 0 {
        (1, -(r.dirty as i64))
    } else {
        (2, 0)
    }
}

#[cfg(test)]
pub(crate) mod test_repo {
    //! Throwaway git repos for collector tests.
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// A temp dir removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("cockpit-test-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
        pub fn s(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Run git in `dir`, panicking on failure; returns trimmed stdout.
    pub fn run(dir: &Path, args: &[&str]) -> String {
        let o = crate::util::git_cmd()
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c"])
            .arg("commit.gpgsign=false")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    pub fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    pub fn commit_all(dir: &Path, msg: &str) {
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-qm", msg]);
    }

    /// A repo on branch `base` with one commit containing `a.txt`.
    pub fn init(tag: &str, base: &str) -> TempDir {
        let t = TempDir::new(tag);
        run(t.path(), &["init", "-q", "-b", base]);
        write(t.path(), "a.txt", "one\ntwo\nthree\n");
        commit_all(t.path(), "init");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::test_repo::*;
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
            ahead,
            dirty,
            ..Default::default()
        }
    }

    #[test]
    fn ranks_ahead_then_dirty_then_clean_then_missing() {
        let missing = Worktree {
            missing: true,
            ..wt(9, 9)
        };
        let mut v = [missing, wt(0, 0), wt(0, 3), wt(2, 0), wt(5, 0)];
        v.sort_by_key(rank);
        let order: Vec<(u32, u32, bool)> =
            v.iter().map(|w| (w.ahead, w.dirty, w.missing)).collect();
        assert_eq!(
            order,
            vec![
                (5, 0, false),
                (2, 0, false),
                (0, 3, false),
                (0, 0, false),
                (9, 9, true)
            ]
        );
    }

    #[test]
    fn parses_porcelain_flags() {
        let s = "worktree /r\nHEAD aaaa\nbranch refs/heads/main\n\n\
                 worktree /r/wt1\nHEAD 1234567890\ndetached\n\n\
                 worktree /tmp/gone\nHEAD bbbb\nbranch refs/heads/feat/x\nprunable gitdir file points to non-existent location\n\n\
                 worktree /r/lk\nHEAD cccc\nbranch refs/heads/lk\nlocked reason\n";
        let e = parse_worktree_porcelain(s);
        assert_eq!(e.len(), 4);
        assert!(e[1].detached && e[1].branch.is_none());
        assert_eq!(branch_label(&e[1]), "(detached 1234567)");
        assert!(e[2].prunable);
        assert_eq!(e[2].branch.as_deref(), Some("feat/x"));
        assert!(e[3].locked);
    }

    #[test]
    fn base_branch_resolution() {
        let r = init("base-master", "master");
        assert_eq!(base_branch(&r.s()), "master");
        assert_eq!(base_ref(&r.s()), "master");
        // A configured default that exists wins over main/master.
        run(r.path(), &["branch", "develop"]);
        run(r.path(), &["config", "init.defaultBranch", "develop"]);
        assert_eq!(base_branch(&r.s()), "develop");
        // origin/HEAD wins over everything, and base_ref prefers origin/<base>.
        let clone = TempDir::new("base-clone");
        run(
            clone.path(),
            &[
                "clone",
                "-q",
                &r.s(),
                &clone.path().join("c").to_string_lossy(),
            ],
        );
        let c = clone.path().join("c");
        let cs = c.to_string_lossy().into_owned();
        assert_eq!(base_branch(&cs), "master");
        assert_eq!(base_ref(&cs), "origin/master");
    }

    #[test]
    fn gathers_detached_missing_locked_and_skips_primary() {
        let r = init("wts", "main");
        let root = r.s();
        let wts = TempDir::new("wts-linked");
        let p = |n: &str| wts.path().join(n).to_string_lossy().into_owned();
        // A linked worktree on branch "main"-like name must still be listed:
        // the primary is identified by position, not branch name.
        run(r.path(), &["checkout", "-q", "-b", "trunk"]);
        run(r.path(), &["worktree", "add", "-q", &p("onmain"), "main"]);
        run(
            r.path(),
            &["worktree", "add", "-q", "--detach", &p("det"), "HEAD"],
        );
        run(
            r.path(),
            &["worktree", "add", "-q", "-b", "feat", &p("feat")],
        );
        write(&wts.path().join("feat"), "b.txt", "x\n");
        commit_all(&wts.path().join("feat"), "feat");
        run(
            r.path(),
            &["worktree", "add", "-q", "-b", "gone", &p("gone")],
        );
        run(r.path(), &["worktree", "lock", &p("feat")]);
        std::fs::remove_dir_all(wts.path().join("gone")).unwrap();

        let rows = gather_worktrees(&root);
        let names: Vec<&str> = rows.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(rows.len(), 4, "{names:?}");
        assert!(!rows.iter().any(|w| w.path == root));
        let get = |n: &str| rows.iter().find(|w| w.name == n).unwrap();
        assert_eq!(get("onmain").branch, "main");
        let det = get("det");
        assert!(det.detached && det.branch.starts_with("(detached "));
        let feat = get("feat");
        assert!(feat.locked && !feat.missing);
        assert_eq!(feat.ahead, 1);
        let gone = get("gone");
        assert!(gone.missing);
        assert_eq!(rows.last().unwrap().name, "gone");
    }

    #[test]
    fn ahead_is_measured_against_origin_base() {
        let up = init("ab-up", "main");
        let dl = TempDir::new("ab-dl");
        let c = dl.path().join("c");
        run(dl.path(), &["clone", "-q", &up.s(), &c.to_string_lossy()]);
        // Local main moves ahead of origin/main; a worktree branched from it
        // has 1 commit to merge relative to origin, 0 relative to local main.
        write(&c, "local.txt", "l\n");
        commit_all(&c, "local only");
        let w = dl.path().join("w");
        run(
            &c,
            &["worktree", "add", "-q", "-b", "f", &w.to_string_lossy()],
        );
        let rows = gather_worktrees(&c.to_string_lossy());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ahead, 1);
    }

    #[test]
    fn git_ok_reports_failure() {
        let t = TempDir::new("gitok");
        assert!(git_ok(&["-C", &t.s(), "status"]).is_none());
        assert!(git_ok(&["--version"]).is_some());
    }
}
