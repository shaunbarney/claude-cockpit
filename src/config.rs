//! User configuration: `claude-cockpit.toml` (CWD) or `~/.config/claude-cockpit/config.toml`.
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
        }
    }
}

/// Parse a TOML config body; returns default on any parse error.
pub fn parse(toml_str: &str) -> Config {
    toml::from_str(toml_str).unwrap_or_default()
}

/// Candidate config paths, in priority order.
fn candidate_paths() -> Vec<std::path::PathBuf> {
    let mut v = vec![std::path::PathBuf::from("claude-cockpit.toml")];
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".config/claude-cockpit/config.toml"));
    }
    v
}

/// Load config from the first existing candidate path; default if none.
pub fn load() -> Config {
    for p in candidate_paths() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            return parse(&s);
        }
    }
    Config::default()
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
    fn discover_endpoints_defaults_on_and_is_toggleable() {
        assert!(parse("").discover_endpoints); // default true
        assert!(!parse("discover_endpoints = false").discover_endpoints);
    }
}
