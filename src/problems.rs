//! Things the user should know are wrong but that don't deserve a widget:
//! collectors that panicked on their last run and config files that didn't
//! parse cleanly. Surfaced as a "⚠ N" header segment and a palette entry.

use crate::refresh::SliceError;

/// One line per problem: collector failures first, then config diagnostics.
pub fn describe(health: &[SliceError], config: &[String]) -> Vec<String> {
    let collectors = health
        .iter()
        .map(|e| format!("collector {}: {}", e.slice, e.message));
    let configs = config.iter().map(|d| format!("config {d}"));
    collectors.chain(configs).collect()
}

/// Current problems for the project at `root` (empty when none is open).
pub fn collect(root: &str) -> Vec<String> {
    let config = if root.is_empty() {
        Vec::new()
    } else {
        crate::config::load_cached(root).1
    };
    describe(&crate::refresh::health(), &config)
}

/// The lines for the "Show problems" view.
pub fn report(problems: &[String]) -> Vec<String> {
    if problems.is_empty() {
        return vec![
            "No problems: every collector is healthy and the config parsed cleanly.".into(),
        ];
    }
    let noun = if problems.len() == 1 {
        "problem"
    } else {
        "problems"
    };
    let mut out = vec![format!("{} {noun}", problems.len()), String::new()];
    out.extend(problems.iter().map(|p| format!("⚠ {p}")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collectors_then_config() {
        let h = vec![SliceError {
            slice: "docker",
            message: "boom".into(),
            at: std::time::SystemTime::UNIX_EPOCH,
        }];
        let c = vec!["/r/.claude-cockpit.toml: unknown key `foo`".to_string()];
        assert_eq!(
            describe(&h, &c),
            vec![
                "collector docker: boom".to_string(),
                "config /r/.claude-cockpit.toml: unknown key `foo`".to_string(),
            ]
        );
        assert!(describe(&[], &[]).is_empty());
    }

    #[test]
    fn report_says_when_all_is_well() {
        assert_eq!(report(&[]).len(), 1);
        let r = report(&["collector loc: x".into()]);
        assert_eq!(r[0], "1 problem");
        assert_eq!(r[2], "⚠ collector loc: x");
    }
}
