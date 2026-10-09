//! User configuration.
//!
//! Looked up in order — `<repo_root>/claude-cockpit.toml`, then
//! `$XDG_CONFIG_HOME/claude-cockpit/config.toml` (default
//! `~/.config/claude-cockpit/config.toml`) — and the first file found is used
//! as-is (no merging). Problems are reported, not swallowed: see
//! [`load_with_diagnostics`].
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EndpointSpec {
    pub label: String,
    #[serde(default = "default_host")]
    pub host: String,
    pub port: u16,
}
fn default_host() -> String {
    "127.0.0.1".to_string()
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub endpoints: Vec<EndpointSpec>,
    /// Auto-discover endpoints from docker-compose / Dockerfile port mappings
    /// in the repo (in addition to any `[[endpoints]]`). Defaults to `true`.
    #[serde(default = "default_true")]
    pub discover_endpoints: bool,
    /// Optional rate-limit caps for the Rate widget gauges. Any field left
    /// unset auto-scales to your own observed peak instead.
    #[serde(default)]
    pub rate_limit: RateLimit,
    /// Colour depth and accent overrides.
    #[serde(default)]
    pub theme: ThemeConfig,
}

/// Theme overrides. Both fields are optional; with neither set the palette is
/// detected from the environment (`NO_COLOR`, then `COLORTERM`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ThemeConfig {
    /// `"mono"`, `"ansi"`, or `"true"`. Unset (or unrecognised) auto-detects.
    /// `NO_COLOR` in the environment always wins over this.
    pub mode: Option<String>,
    /// Accent as `#rrggbb`. Only applies on the truecolor tier.
    pub accent: Option<String>,
}

/// Known rate-limit caps. All optional — unset fields auto-scale to the
/// busiest window observed in your local usage history.
///
/// The Claude Code subscription limit is measured in **prompts per rolling
/// 5-hour window**, so set `plan` (or `prompts_5h`) for a true gauge. The API
/// per-minute limit that bites first is **OTPM** (output tokens/min).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RateLimit {
    /// Subscription plan: `"pro"`, `"max5x"`, or `"max20x"` — sets the 5-hour
    /// prompt cap (~20 / ~100 / ~400). Plan can't be detected locally; run
    /// `/status` in Claude Code if unsure. Ignored if `prompts_5h` is set.
    pub plan: Option<String>,
    /// Explicit prompt cap for the rolling 5-hour window (overrides `plan`).
    pub prompts_5h: Option<u64>,
    /// Output tokens per minute (OTPM) cap.
    pub output_per_min: Option<u64>,
}

/// Approximate 5-hour prompt cap and display label for a configured plan name.
/// Figures are post-2026-05-06 doubling and approximate — Anthropic tunes them.
pub fn plan_prompt_cap(plan: &str) -> Option<(u64, &'static str)> {
    match plan.to_lowercase().replace([' ', '_', '-'], "").as_str() {
        "pro" => Some((20, "Pro")),
        "max5x" | "max5" | "max" => Some((100, "Max 5x")),
        "max20x" | "max20" => Some((400, "Max 20x")),
        _ => None,
    }
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            endpoints: Vec::new(),
            discover_endpoints: true,
            rate_limit: RateLimit::default(),
            theme: ThemeConfig::default(),
        }
    }
}

/// Parse a TOML config body; returns default on any parse error.
///
/// Prefer [`parse_with_diagnostics`], which says *why* a file was ignored.
pub fn parse(toml_str: &str) -> Config {
    parse_with_diagnostics(toml_str).0
}

/// Parse a TOML config body, collecting human-readable diagnostics.
///
/// A syntax or type error yields the default config plus the error; unknown
/// keys (typos, or a key placed under the wrong `[table]`) are ignored with a
/// warning each, never fatal.
pub fn parse_with_diagnostics(toml_str: &str) -> (Config, Vec<String>) {
    let mut unknown = Vec::new();
    let de = toml::Deserializer::new(toml_str);
    match serde_ignored::deserialize(de, |path| unknown.push(path.to_string())) {
        Ok(cfg) => {
            let diags = unknown
                .into_iter()
                .map(|k| format!("unknown key `{k}` (ignored)"))
                .collect();
            (cfg, diags)
        }
        Err(e) => (
            Config::default(),
            vec![format!(
                "invalid config, using defaults: {}",
                e.to_string().trim()
            )],
        ),
    }
}

/// Candidate config paths, in priority order: `<repo_root>/claude-cockpit.toml`,
/// then `$XDG_CONFIG_HOME/claude-cockpit/config.toml` (default
/// `~/.config/claude-cockpit/config.toml`). The **first file that exists wins**
/// outright — files are not merged.
pub fn candidate_paths(root: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if !root.is_empty() {
        v.push(Path::new(root).join("claude-cockpit.toml"));
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
    if let Some(base) = xdg {
        v.push(base.join("claude-cockpit").join("config.toml"));
    }
    v
}

/// Load the config for the repo at `root`, with diagnostics (each prefixed by
/// the file it concerns). Missing files are not an error.
pub fn load_with_diagnostics(root: &str) -> (Config, Vec<String>) {
    for p in candidate_paths(root) {
        match std::fs::read_to_string(&p) {
            Ok(s) => {
                let (cfg, diags) = parse_with_diagnostics(&s);
                let diags = diags
                    .into_iter()
                    .map(|d| format!("{}: {d}", p.display()))
                    .collect();
                return (cfg, diags);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return (
                    Config::default(),
                    vec![format!("{}: unreadable, using defaults: {e}", p.display())],
                )
            }
        }
    }
    (Config::default(), Vec::new())
}

/// Load the config for the repo at `root`, re-reading from disk only when the
/// chosen file (or which file exists) changed since the last call.
pub fn load_for(root: &str) -> Config {
    load_cached(root).0
}

/// Which candidate files exist, and their (mtime, len): the cache key.
type CacheKey = Vec<(PathBuf, Option<(SystemTime, u64)>)>;
/// (root, key, config, diagnostics) of the last load.
type CacheEntry = (String, CacheKey, Config, Vec<String>);

/// Like [`load_with_diagnostics`] but cached on (path, mtime, len).
pub fn load_cached(root: &str) -> (Config, Vec<String>) {
    static CACHE: Mutex<Option<CacheEntry>> = Mutex::new(None);
    let key: CacheKey = candidate_paths(root)
        .into_iter()
        .map(|p| {
            let meta = std::fs::metadata(&p).ok();
            let stamp = meta.and_then(|m| Some((m.modified().ok()?, m.len())));
            (p, stamp)
        })
        .collect();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((r, k, cfg, diags)) = cache.as_ref() {
        if r == root && *k == key {
            return (cfg.clone(), diags.clone());
        }
    }
    let (cfg, diags) = load_with_diagnostics(root);
    *cache = Some((root.to_string(), key, cfg.clone(), diags.clone()));
    (cfg, diags)
}

/// Load config for the repo containing the current directory (cached).
///
/// Kept for callers that don't carry a repo root; prefer [`load_for`].
pub fn load() -> Config {
    static ROOT: OnceLock<String> = OnceLock::new();
    let root = ROOT.get_or_init(crate::collect::git::repo_root);
    load_for(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_endpoints() {
        let cfg = parse(
            r#"
            [[endpoints]]
            label = "frontend"
            port = 3000
            [[endpoints]]
            label = "backend"
            host = "localhost"
            port = 8080
        "#,
        );
        assert_eq!(cfg.endpoints.len(), 2);
        assert_eq!(cfg.endpoints[0].label, "frontend");
        assert_eq!(cfg.endpoints[0].host, "127.0.0.1"); // default
        assert_eq!(cfg.endpoints[1].host, "localhost");
        assert_eq!(cfg.endpoints[1].port, 8080);
    }
    #[test]
    fn empty_is_default() {
        assert_eq!(parse(""), Config::default());
        assert_eq!(parse("garbage = ["), Config::default()); // parse error -> default
    }
    #[test]
    fn parse_errors_are_reported() {
        let (cfg, diags) = parse_with_diagnostics("garbage = [");
        assert_eq!(cfg, Config::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].contains("invalid config"), "{diags:?}");
        let (_, diags) = parse_with_diagnostics("[[endpoints]]\nlabel = \"x\"\nport = \"nope\"");
        assert!(diags[0].contains("invalid config"), "{diags:?}");
    }
    #[test]
    fn unknown_keys_warn_but_do_not_fail() {
        // The classic mistake: a top-level key written after `[theme]` lands
        // inside the theme table.
        let (cfg, diags) =
            parse_with_diagnostics("[theme]\nmode = \"mono\"\ndiscover_endpoints = false\n");
        assert_eq!(cfg.theme.mode.as_deref(), Some("mono"));
        assert!(cfg.discover_endpoints, "misplaced key must not apply");
        assert_eq!(
            diags,
            vec!["unknown key `theme.discover_endpoints` (ignored)"]
        );
        assert!(parse_with_diagnostics("discover_endpoints = false")
            .1
            .is_empty());
    }
    #[test]
    fn candidate_paths_start_at_repo_root() {
        let v = candidate_paths("/some/repo");
        assert_eq!(v[0], Path::new("/some/repo/claude-cockpit.toml"));
        assert!(v.len() <= 2);
        assert!(v
            .iter()
            .skip(1)
            .all(|p| p.ends_with("claude-cockpit/config.toml")));
        assert!(!candidate_paths("")
            .iter()
            .any(|p| p.ends_with("claude-cockpit.toml")));
    }
    #[test]
    fn repo_file_wins_and_is_cached_until_it_changes() {
        let dir = std::env::temp_dir().join(format!("cockpit-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.to_string_lossy().into_owned();
        let f = dir.join("claude-cockpit.toml");
        std::fs::write(&f, "discover_endpoints = false\n").unwrap();
        assert!(!load_for(&root).discover_endpoints);
        std::fs::write(&f, "discover_endpoints = true\nbogus = 1\n").unwrap();
        let (cfg, diags) = load_cached(&root);
        assert!(cfg.discover_endpoints);
        assert!(diags[0].contains("claude-cockpit.toml") && diags[0].contains("bogus"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn parses_theme_overrides() {
        let cfg = parse("[theme]\nmode = \"mono\"\naccent = \"#ff8800\"");
        assert_eq!(cfg.theme.mode.as_deref(), Some("mono"));
        assert_eq!(cfg.theme.accent.as_deref(), Some("#ff8800"));
        // Absent section leaves both unset.
        assert_eq!(parse("").theme, ThemeConfig::default());
    }
    #[test]
    fn discover_endpoints_defaults_on_and_is_toggleable() {
        assert!(parse("").discover_endpoints); // default true
        assert!(!parse("discover_endpoints = false").discover_endpoints);
    }
}
