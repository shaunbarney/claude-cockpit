//! Shared formatters and path helpers.
use std::path::PathBuf;

/// Claude home (`~/.claude`), respecting $HOME / dirs.
pub fn claude_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}

/// A `git` command that ignores repository-location variables inherited from
/// the environment (`GIT_DIR`, `GIT_WORK_TREE`, …). Every call here names its
/// repository with `-C`; under a git hook or `rebase -x` those variables would
/// otherwise silently redirect it — including test fixtures — at the
/// enclosing repository.
pub fn git_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("git");
    for v in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_PREFIX",
    ] {
        c.env_remove(v);
    }
    c
}

/// Bytes -> "1.5 KB" / "5.0 MB" (binary units, 1 decimal above KB).
pub fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

/// Seconds -> compact "45s" / "1m" / "3h" / "2d".
pub fn human_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// 12345 -> "12,345".
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// Compact count to three significant figures: 950 -> "950",
/// 5_740 -> "5.74K", 574_000 -> "574K", 3_050_000 -> "3.05M".
pub fn compact_count(n: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];
    for (div, unit) in UNITS {
        if n >= div {
            let v = n as f64 / div as f64;
            // Pick decimals so the rounded value keeps three significant figures.
            let s = if v >= 99.95 {
                format!("{v:.0}")
            } else if v >= 9.995 {
                format!("{v:.1}")
            } else {
                format!("{v:.2}")
            };
            return format!("{s}{unit}");
        }
    }
    n.to_string()
}

/// Strip a leading "claude-" prefix so model names fit narrow columns.
pub fn short_model(name: &str) -> String {
    name.strip_prefix("claude-").unwrap_or(name).to_string()
}

/// Display string for a job's worktree: the worktree folder name, else the
/// branch, else "—". Shown verbatim (no prefix stripping) so it matches the
/// Worktrees pane exactly.
pub fn worktree_label(branch: Option<&str>, path: Option<&str>) -> String {
    // Prefer the worktree FOLDER name: it's what `git worktree list` and the
    // Worktrees widget show, so the Jobs "Worktree" column matches it exactly.
    if let Some(p) = path {
        if let Some(name) = std::path::Path::new(p).file_name().and_then(|s| s.to_str()) {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    if let Some(b) = branch {
        if !b.is_empty() {
            return b.to_string();
        }
    }
    "—".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bytes_human() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
    #[test]
    fn compact_count_keeps_three_figures() {
        assert_eq!(compact_count(950), "950");
        assert_eq!(compact_count(5_740), "5.74K");
        assert_eq!(compact_count(57_400), "57.4K");
        assert_eq!(compact_count(574_321), "574K");
        assert_eq!(compact_count(3_050_000), "3.05M");
        assert_eq!(compact_count(1_000_000), "1.00M");
    }

    #[test]
    fn duration_human() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(90), "1m");
        assert_eq!(human_duration(3 * 3600 + 120), "3h");
    }
    #[test]
    fn thousands_sep() {
        assert_eq!(thousands(12345), "12,345");
    }
    #[test]
    fn worktree_label_prefers_folder_verbatim() {
        assert_eq!(worktree_label(Some("worktree-x"), None), "worktree-x");
        assert_eq!(
            worktree_label(None, Some("/r/.claude/worktrees/worktree-y")),
            "worktree-y"
        );
        assert_eq!(worktree_label(Some("feat/demo"), None), "feat/demo");
        assert_eq!(worktree_label(None, Some("/repo/.claude/worktrees/y")), "y");
        // Folder name wins over branch so Jobs matches the Worktrees widget.
        assert_eq!(
            worktree_label(Some("feature/goal"), Some("/r/.claude/worktrees/goal-m0")),
            "goal-m0"
        );
        assert_eq!(worktree_label(None, None), "—");
    }
}
