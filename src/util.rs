//! Shared formatters and path helpers.
use std::path::PathBuf;

/// Claude home (`~/.claude`), respecting $HOME / dirs.
pub fn claude_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
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
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// Strip a leading "claude-" prefix so model names fit narrow columns.
pub fn short_model(name: &str) -> String {
    name.strip_prefix("claude-").unwrap_or(name).to_string()
}

/// Display string for a job's worktree: branch, else the worktree folder name, else "—".
pub fn worktree_label(branch: Option<&str>, path: Option<&str>) -> String {
    if let Some(b) = branch {
        if !b.is_empty() {
            return b.to_string();
        }
    }
    if let Some(p) = path {
        if let Some(name) = std::path::Path::new(p).file_name().and_then(|s| s.to_str()) {
            if !name.is_empty() {
                return name.to_string();
            }
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
    fn duration_human() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(90), "1m");
        assert_eq!(human_duration(3 * 3600 + 120), "3h");
    }
    #[test]
    fn thousands_sep() {
        assert_eq!(thousands(12345), "12,345");
    }
}
