//! Docker container monitoring via the `docker` CLI (no daemon library dependency).
//! Everything is best-effort: if `docker` is missing or the daemon is down, the
//! gather functions return empty and never panic.
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub state: String,
    pub status: String,
    pub health: Option<String>,
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub ports: Vec<String>,
}

/// Extract a health word from a status string, e.g. "Up 2 hours (healthy)" -> "healthy".
/// Only recognised health words are returned; numeric exit codes are ignored.
fn extract_health(status: &str) -> Option<String> {
    let start = status.find('(')?;
    let rest = &status[start + 1..];
    let end = rest.find(')')?;
    let word = &rest[..end];
    matches!(word, "healthy" | "unhealthy" | "starting" | "health: starting")
        .then(|| word.to_string())
}

/// Parse one `docker ps --format '{{json .}}'` line.
pub fn parse_ps_line(line: &str) -> Option<Container> {
    let v: Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let status = s("Status");
    let ports_raw = s("Ports");
    let ports = if ports_raw.is_empty() {
        vec![]
    } else {
        ports_raw.split(", ").map(|x| x.to_string()).collect()
    };
    Some(Container {
        id: s("ID"),
        name: s("Names"),
        state: s("State"),
        health: extract_health(&status),
        status,
        cpu_pct: 0.0,
        mem_used: 0,
        mem_limit: 0,
        ports,
    })
}

/// Parse a memory token like "256MiB" / "1.5GiB" / "0B" into bytes.
pub fn parse_mem(tok: &str) -> u64 {
    let tok = tok.trim();
    let (num, mult): (&str, f64) =
        if let Some(n) = tok.strip_suffix("GiB") {
            (n, 1024.0 * 1024.0 * 1024.0)
        } else if let Some(n) = tok.strip_suffix("MiB") {
            (n, 1024.0 * 1024.0)
        } else if let Some(n) = tok.strip_suffix("KiB") {
            (n, 1024.0)
        } else if let Some(n) = tok.strip_suffix("GB") {
            (n, 1e9)
        } else if let Some(n) = tok.strip_suffix("MB") {
            (n, 1e6)
        } else if let Some(n) = tok.strip_suffix("kB") {
            (n, 1e3)
        } else if let Some(n) = tok.strip_suffix('B') {
            (n, 1.0)
        } else {
            (tok, 1.0)
        };
    (num.trim().parse::<f64>().unwrap_or(0.0) * mult) as u64
}

/// Parse one `docker stats --no-stream --format '{{json .}}'` line -> (id, cpu%, used, limit).
pub fn parse_stats_line(line: &str) -> Option<(String, f32, u64, u64)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let id = v
        .get("Container")
        .or_else(|| v.get("ID"))
        .and_then(|x| x.as_str())?
        .to_string();
    let cpu = v
        .get("CPUPerc")
        .and_then(|x| x.as_str())
        .unwrap_or("0%")
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    let mem = v
        .get("MemUsage")
        .and_then(|x| x.as_str())
        .unwrap_or("0B / 0B");
    let mut parts = mem.split('/');
    let used = parse_mem(parts.next().unwrap_or("0B"));
    let limit = parse_mem(parts.next().unwrap_or("0B"));
    Some((id, cpu, used, limit))
}

fn run(args: &[&str]) -> String {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Gather all containers, joining `docker ps` with `docker stats` (empty if docker unavailable).
pub fn gather_containers() -> Vec<Container> {
    let ps = run(&["ps", "--all", "--no-trunc", "--format", "{{json .}}"]);
    let mut containers: Vec<Container> = ps.lines().filter_map(parse_ps_line).collect();
    let stats = run(&["stats", "--no-stream", "--format", "{{json .}}"]);
    for line in stats.lines() {
        if let Some((id, cpu, used, limit)) = parse_stats_line(line) {
            for c in containers.iter_mut() {
                if c.id == id || c.id.starts_with(&id) || c.name == id {
                    c.cpu_pct = cpu;
                    c.mem_used = used;
                    c.mem_limit = limit;
                }
            }
        }
    }
    containers
}

/// Recent logs for a container (best-effort; merges stdout+stderr since docker logs uses both).
pub fn container_logs(id: &str, tail: usize) -> Vec<String> {
    let t = tail.to_string();
    match std::process::Command::new("docker")
        .args(["logs", "--tail", &t, id])
        .output()
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s.lines().map(|l| l.to_string()).collect()
        }
        Err(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_json_line() {
        let l = r#"{"ID":"abc","Names":"sovra-backend","State":"running","Status":"Up 2 hours (healthy)","Ports":"0.0.0.0:8080->8080/tcp"}"#;
        let c = parse_ps_line(l).unwrap();
        assert_eq!(c.name, "sovra-backend");
        assert_eq!(c.state, "running");
        assert_eq!(c.health.as_deref(), Some("healthy"));
        assert!(c.ports.iter().any(|p| p.contains("8080")));
    }

    #[test]
    fn parses_stats_json_line() {
        let l = r#"{"Container":"abc","CPUPerc":"12.5%","MemUsage":"256MiB / 2GiB"}"#;
        let (id, cpu, used, limit) = parse_stats_line(l).unwrap();
        assert_eq!(id, "abc");
        assert!((cpu - 12.5).abs() < 0.01);
        assert!(used > 0 && limit > used);
    }

    #[test]
    fn parses_mem_units() {
        assert_eq!(parse_mem("256MiB"), 256 * 1024 * 1024);
        assert_eq!(parse_mem("2GiB"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_mem("0B"), 0);
    }

    #[test]
    fn no_health_marker() {
        let l = r#"{"ID":"x","Names":"n","State":"exited","Status":"Exited (0) 3 minutes ago","Ports":""}"#;
        let c = parse_ps_line(l).unwrap();
        assert_eq!(c.health, None);
        assert!(c.ports.is_empty());
    }
}
