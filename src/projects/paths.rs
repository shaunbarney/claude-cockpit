//! Where the project registry lives, and how a path becomes a project identity.
//!
//! The registry you edit is config (`$XDG_CONFIG_HOME/claude-cockpit/projects.toml`);
//! what the cockpit writes behind your back — last project, recents, hidden
//! suggestions — is state (`$XDG_STATE_HOME/claude-cockpit/state.toml`). Keeping
//! them apart means a project switch never rewrites a file you hand-edit.

use std::path::{Path, PathBuf};

const APP: &str = "claude-cockpit";

fn xdg_dir(var: &str, fallback: &[&str]) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        let p = PathBuf::from(v);
        if p.is_absolute() {
            return Some(p.join(APP));
        }
    }
    let mut p = dirs::home_dir()?;
    for part in fallback {
        p.push(part);
    }
    Some(p.join(APP))
}

/// `$XDG_CONFIG_HOME/claude-cockpit/projects.toml` (default `~/.config/...`).
pub fn registry_file() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", &[".config"]).map(|d| d.join("projects.toml"))
}

/// `$XDG_STATE_HOME/claude-cockpit/state.toml` (default `~/.local/state/...`).
pub fn state_file() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", &[".local", "state"]).map(|d| d.join("state.toml"))
}

/// Expand a leading `~` / `~/`. Other paths pass through untouched.
pub fn expand_tilde(p: &str) -> PathBuf {
    match (p.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(p),
    }
}

/// Render a path with `$HOME` shortened to `~`, for display and for writing
/// the registry (so it survives a home-directory move).
pub fn contract_tilde(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    p.display().to_string()
}

/// The main checkout of the git repository containing `p`.
///
/// A linked worktree resolves to the repository it belongs to — a project is
/// the repo, and its worktrees are rows inside it, not projects of their own.
/// `None` when `p` is not inside a git repository.
pub fn repo_root_of(p: &Path) -> Option<PathBuf> {
    let common = git_out(
        p,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(common);
    if common.file_name().is_some_and(|n| n == ".git") {
        return common.parent().map(Path::to_path_buf);
    }
    // Bare repo or unusual layout: fall back to the working tree's top level.
    git_out(p, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let out = crate::util::git_cmd()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Canonical identity for a project path: symlinks resolved, no trailing slash.
/// Falls back to the path as given when it no longer exists.
pub fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_round_trips() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/x/y"), home.join("x/y"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        // `~user` is not ours to expand.
        assert_eq!(expand_tilde("~bob/x"), PathBuf::from("~bob/x"));
        assert_eq!(contract_tilde(&home.join("a/b")), "~/a/b");
        assert_eq!(contract_tilde(&home), "~");
        assert_eq!(contract_tilde(Path::new("/opt/x")), "/opt/x");
    }

    #[test]
    fn worktree_resolves_to_its_main_repo() {
        let dir = crate::projects::testutil::temp_repo("paths-wt");
        let wt = dir.join("wt");
        let ok = crate::util::git_cmd()
            .arg("-C")
            .arg(&dir)
            .args(["worktree", "add", "-q", "-b", "b1"])
            .arg(&wt)
            .status()
            .unwrap();
        assert!(ok.success());
        let want = canonical(&dir);
        assert_eq!(repo_root_of(&wt).map(|p| canonical(&p)), Some(want.clone()));
        assert_eq!(repo_root_of(&dir).map(|p| canonical(&p)), Some(want));
        assert_eq!(repo_root_of(Path::new("/definitely/not/here")), None);
    }
}
