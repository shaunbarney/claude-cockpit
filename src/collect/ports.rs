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
        if cols.len() < 2 {
            continue;
        }
        let cmd = cols[0].to_string();
        // Find a token shaped like ADDR:PORT (must contain ':' with a valid port suffix).
        for tok in &cols {
            if !tok.contains(':') {
                continue;
            }
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
    let out = std::process::Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output()
        .ok()?;
    parse_lsof_pid(&String::from_utf8_lossy(&out.stdout))
}

/// TCP-connect health check (500ms timeout) + latency + owning PID.
pub fn check(label: &str, host: &str, port: u16) -> Endpoint {
    let start = Instant::now();
    let latency = format!("{host}:{port}")
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .and_then(|sa| TcpStream::connect_timeout(&sa, Duration::from_millis(500)).ok())
        .map(|_| start.elapsed().as_millis() as u64);
    Endpoint {
        label: label.to_string(),
        host: host.to_string(),
        port,
        up: latency.is_some(),
        latency_ms: latency,
        pid: lsof_pid(port),
    }
}

/// A host-reachable port discovered from a docker artifact, with a label.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredPort {
    pub label: String,
    pub port: u16,
}

/// Extract the host-side port from a compose short-syntax `ports:` value.
/// Handles `"8080:80"`, `"127.0.0.1:8080:80"`, `"8080:80/tcp"` and host ranges
/// (`"3000-3005:..."` -> 3000). A bare container port (`"3000"`, host side is
/// ephemeral and not reachable by a fixed number) yields `None`.
fn host_port_from_short(val: &str) -> Option<u16> {
    let val = val.trim().trim_matches('"').trim_matches('\'');
    let val = val.split('/').next().unwrap_or(val); // drop /tcp|/udp
    let parts: Vec<&str> = val.split(':').collect();
    let host_part = match parts.as_slice() {
        [_container] => return None,      // host port is random
        [host, _container] => *host,      // HOST:CONTAINER
        [_ip, host, _container] => *host, // IP:HOST:CONTAINER
        _ => return None,
    };
    // A range like "3000-3005" -> take the first port.
    let first = host_part.split('-').next().unwrap_or(host_part);
    first.trim().parse().ok()
}

/// Pull a port out of a compose long-syntax `published:` field
/// (`published: 8080` or `published: "8080"`).
fn published_port(line: &str) -> Option<u16> {
    let rest = line.split("published:").nth(1)?;
    rest.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .parse()
        .ok()
}

/// Parse host-reachable ports out of a docker-compose file. Tracks the current
/// service name (used as the label) by indentation, and reads both the short
/// (`- "8080:80"`) and long (`- target: 80 / published: 8080`) port syntaxes.
pub fn parse_compose_ports(yaml: &str) -> Vec<DiscoveredPort> {
    let mut out: Vec<DiscoveredPort> = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new(); // (indent, key) ancestor path
    let mut service = String::from("compose");
    let mut in_ports = false;
    let mut ports_indent = 0usize;

    for raw in yaml.lines() {
        let line = raw.split('#').next().unwrap_or(raw); // strip comments
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();

        if in_ports {
            if indent > ports_indent {
                if let Some(rest) = trimmed.strip_prefix('-') {
                    let item = rest.trim();
                    if item.contains("published:") {
                        if let Some(p) = published_port(item) {
                            out.push(DiscoveredPort {
                                label: service.clone(),
                                port: p,
                            });
                        }
                    } else if !item.starts_with("target:") && !item.starts_with("protocol:") {
                        if let Some(p) = host_port_from_short(item) {
                            out.push(DiscoveredPort {
                                label: service.clone(),
                                port: p,
                            });
                        }
                    }
                } else if trimmed.contains("published:") {
                    if let Some(p) = published_port(trimmed) {
                        out.push(DiscoveredPort {
                            label: service.clone(),
                            port: p,
                        });
                    }
                }
                continue;
            }
            in_ports = false; // dedented out of the ports block; reprocess this line
        }

        // Maintain the ancestor path by indentation.
        while stack.last().map(|(i, _)| *i >= indent).unwrap_or(false) {
            stack.pop();
        }

        if trimmed == "ports:" || trimmed.starts_with("ports:") {
            service = stack
                .last()
                .map(|(_, k)| k.clone())
                .unwrap_or_else(|| "compose".to_string());
            in_ports = true;
            ports_indent = indent;
            continue;
        }
        if let Some(key) = trimmed.strip_suffix(':') {
            stack.push((indent, key.to_string()));
        }
    }
    dedup_ports(out)
}

/// Parse `EXPOSE` lines from a Dockerfile (`EXPOSE 8080 9090/tcp`, ranges ok).
pub fn parse_dockerfile_expose(text: &str, label: &str) -> Vec<DiscoveredPort> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut toks = line.split_whitespace();
        // The EXPOSE keyword must be the first token on the line.
        if !toks
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("expose"))
        {
            continue;
        }
        for tok in toks {
            let tok = tok.split('/').next().unwrap_or(tok); // drop /tcp|/udp
            let first = tok.split('-').next().unwrap_or(tok); // range -> first
            if let Ok(p) = first.parse::<u16>() {
                out.push(DiscoveredPort {
                    label: label.to_string(),
                    port: p,
                });
            }
        }
    }
    dedup_ports(out)
}

/// Dedup discovered ports by port number, keeping the first label seen.
fn dedup_ports(v: Vec<DiscoveredPort>) -> Vec<DiscoveredPort> {
    let mut seen = std::collections::BTreeSet::new();
    v.into_iter().filter(|d| seen.insert(d.port)).collect()
}

const DOCKER_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

fn is_compose_file(name: &str) -> bool {
    matches!(
        name,
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml"
    ) || (name.starts_with("docker-compose.")
        && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

fn is_dockerfile(name: &str) -> bool {
    name == "Dockerfile" || name.starts_with("Dockerfile.") || name.ends_with(".Dockerfile")
}

/// Walk `root` (bounded depth, skipping vendor dirs) and extract host-reachable
/// ports from every docker-compose file and Dockerfile found.
pub fn discover_docker_ports(root: &std::path::Path) -> Vec<DiscoveredPort> {
    fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<DiscoveredPort>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !DOCKER_SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    walk(&path, depth - 1, out);
                }
            } else if is_compose_file(&name) {
                if let Ok(txt) = std::fs::read_to_string(&path) {
                    out.extend(parse_compose_ports(&txt));
                }
            } else if is_dockerfile(&name) {
                if let Ok(txt) = std::fs::read_to_string(&path) {
                    let label = path
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "docker".to_string());
                    out.extend(parse_dockerfile_expose(&txt, &label));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 4, &mut out);
    dedup_ports(out)
}

/// Gather endpoint health for the dashboard.
///
/// Combines, in priority order: explicit `[[endpoints]]` from config, then —
/// when `discover_endpoints` is on (default) — ports auto-discovered from
/// docker-compose / Dockerfile artifacts under `root`. If that yields nothing,
/// falls back to auto-discovering local TCP LISTEN ports via `lsof`.
pub fn gather_endpoints(cfg: &crate::config::Config, root: &str) -> Vec<Endpoint> {
    let mut eps: Vec<Endpoint> = cfg
        .endpoints
        .iter()
        .map(|e| check(&e.label, &e.host, e.port))
        .collect();

    if cfg.discover_endpoints {
        for d in discover_docker_ports(std::path::Path::new(root)) {
            let dup = eps
                .iter()
                .any(|e| e.port == d.port && e.host == "127.0.0.1");
            if !dup {
                eps.push(check(&d.label, "127.0.0.1", d.port));
            }
        }
    }

    if !eps.is_empty() {
        return eps;
    }

    // Last resort: whatever is listening locally right now.
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    parse_listeners(&out)
        .into_iter()
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

    #[test]
    fn short_syntax_host_ports() {
        assert_eq!(host_port_from_short("8080:80"), Some(8080));
        assert_eq!(host_port_from_short("\"127.0.0.1:5000:5000\""), Some(5000));
        assert_eq!(host_port_from_short("9000:9000/tcp"), Some(9000));
        assert_eq!(host_port_from_short("3000-3005:3000-3005"), Some(3000));
        assert_eq!(host_port_from_short("3000"), None); // bare container port
    }

    #[test]
    fn compose_short_and_long_syntax() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - 9000:9000
    environment:
      FOO: bar
  db:
    ports:
      - target: 5432
        published: 5433
        protocol: tcp
"#;
        let ports = parse_compose_ports(yaml);
        let pairs: Vec<(&str, u16)> = ports.iter().map(|d| (d.label.as_str(), d.port)).collect();
        assert!(pairs.contains(&("web", 8080)), "{pairs:?}");
        assert!(pairs.contains(&("web", 9000)), "{pairs:?}");
        assert!(pairs.contains(&("db", 5433)), "{pairs:?}");
        // The `environment:` block under web must not be mistaken for a service.
        assert!(!pairs.iter().any(|(l, _)| *l == "environment"));
    }

    #[test]
    fn compose_top_level_services_legacy() {
        // Legacy v1 compose: services at the top level, no `services:` key.
        let yaml = "api:\n  ports:\n    - 4000:4000\n";
        let ports = parse_compose_ports(yaml);
        assert_eq!(
            ports,
            vec![DiscoveredPort {
                label: "api".into(),
                port: 4000
            }]
        );
    }

    #[test]
    fn dockerfile_expose_ports() {
        let txt = "FROM rust\nEXPOSE 8080 9090/tcp\nEXPOSE 3000-3001\nRUN echo hi\n";
        let ports = parse_dockerfile_expose(txt, "svc");
        let nums: Vec<u16> = ports.iter().map(|d| d.port).collect();
        assert_eq!(nums, vec![8080, 9090, 3000]);
        assert!(ports.iter().all(|d| d.label == "svc"));
    }

    #[test]
    fn dockerfile_with_multibyte_lines_does_not_panic() {
        // Comment/box-drawing lines must not be byte-sliced (regression).
        let txt = "# ─── build ───\nFROM rust\nEXPOSE 8080\n# café ☕\n";
        let ports = parse_dockerfile_expose(txt, "svc");
        assert_eq!(
            ports,
            vec![DiscoveredPort {
                label: "svc".into(),
                port: 8080
            }]
        );
    }

    #[test]
    fn file_name_matchers() {
        assert!(is_compose_file("docker-compose.yml"));
        assert!(is_compose_file("compose.yaml"));
        assert!(is_compose_file("docker-compose.prod.yml"));
        assert!(!is_compose_file("compose.txt"));
        assert!(is_dockerfile("Dockerfile"));
        assert!(is_dockerfile("Dockerfile.dev"));
        assert!(is_dockerfile("api.Dockerfile"));
        assert!(!is_dockerfile("Makefile"));
    }
}
