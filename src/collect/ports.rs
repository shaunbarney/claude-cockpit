//! Dev-endpoint health: TCP connect + latency, plus owning PID via lsof.
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub label: String,
    pub host: String,
    pub port: u16,
    pub up: bool,
    pub latency_ms: Option<u64>,
    pub pid: Option<u32>,
}

/// First PID from `lsof -ti :PORT` output.
pub fn parse_lsof_pid(out: &str) -> Option<u32> {
    out.lines().next()?.trim().parse().ok()
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN` into deduped (command, port) listeners.
pub fn parse_listeners(out: &str) -> Vec<(String, u16)> {
    let mut seen = std::collections::BTreeSet::new();
    let mut v = Vec::new();
    for line in out.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 { continue; }
        let cmd = cols[0].to_string();
        // Find a token shaped like ADDR:PORT (must contain ':' with a valid port suffix).
        for tok in &cols {
            if !tok.contains(':') { continue; }
            if let Some(p) = tok.rsplit(':').next().and_then(|s| s.parse::<u16>().ok()) {
                if seen.insert(p) {
                    v.push((cmd.clone(), p));
                }
                break;
            }
        }
    }
    v
}

fn lsof_pid(port: u16) -> Option<u32> {
    let out = std::process::Command::new("lsof").args(["-ti", &format!(":{port}")]).output().ok()?;
    parse_lsof_pid(&String::from_utf8_lossy(&out.stdout))
}

/// TCP-connect health check (500ms timeout) + latency + owning PID.
pub fn check(label: &str, host: &str, port: u16) -> Endpoint {
    let start = Instant::now();
    let latency = format!("{host}:{port}").to_socket_addrs().ok()
        .and_then(|mut it| it.next())
        .and_then(|sa| TcpStream::connect_timeout(&sa, Duration::from_millis(500)).ok())
        .map(|_| start.elapsed().as_millis() as u64);
    Endpoint {
        label: label.to_string(), host: host.to_string(), port,
        up: latency.is_some(), latency_ms: latency, pid: lsof_pid(port),
    }
}

/// Gather endpoint health. Uses config endpoints if any; else auto-discovers LISTEN ports.
pub fn gather_endpoints(cfg: &crate::config::Config) -> Vec<Endpoint> {
    if !cfg.endpoints.is_empty() {
        return cfg.endpoints.iter().map(|e| check(&e.label, &e.host, e.port)).collect();
    }
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    parse_listeners(&out).into_iter()
        .map(|(cmd, port)| check(&cmd, "127.0.0.1", port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pid_from_lsof() {
        assert_eq!(parse_lsof_pid("12345\n67890\n"), Some(12345));
        assert_eq!(parse_lsof_pid(""), None);
    }
    #[test]
    fn listeners_from_lsof() {
        let out = "COMMAND  PID USER  FD TYPE DEVICE SIZE/OFF NODE NAME\nnode 111 u IPv4 0t0 TCP 127.0.0.1:3000 (LISTEN)\npython 222 u IPv4 0t0 TCP *:8080 (LISTEN)";
        let v = parse_listeners(out);
        assert!(v.contains(&("node".to_string(), 3000)));
        assert!(v.contains(&("python".to_string(), 8080)));
    }
}
