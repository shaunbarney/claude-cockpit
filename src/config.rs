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

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub endpoints: Vec<EndpointSpec>,
    /// Extra process-name substrings to include in the Processes widget.
    #[serde(default)]
    pub processes: Vec<String>,
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
}
