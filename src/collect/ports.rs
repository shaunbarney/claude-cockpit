//! Dev-endpoint health: TCP connect + latency, plus who owns each port.
//!
//! Where the endpoint list comes from, in priority order:
//! 1. `[[endpoints]]` in the config (always shown, in config order);
//! 2. with `discover_endpoints` on (the default), the published ports of this
//!    repo's **running** containers (Compose `working_dir` under the repo),
//!    labelled by Compose service — the ground truth for what is actually up;
//! 3. only if (2) is empty, ports declared in the repo's *default* Compose
//!    files (see [`compose`]); variant files and Dockerfile `EXPOSE` are never
//!    used;
//! 4. if all of that is empty, every loopback-reachable listening port.
//!
//! Ownership comes from one listener snapshot per refresh ([`listen`]). A port
//! published by a Docker container is attributed to that container, because
//! the process holding it (`docker-proxy`, root-owned) is not useful or not
//! even visible.
pub mod compose;
pub mod listen;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::collect::docker::Container;
use listen::Listener;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Endpoint {
    pub label: String,
    pub host: String,
    pub port: u16,
    pub up: bool,
    pub latency_ms: Option<u64>,
    /// PID of the listening process, when visible (own processes, or root).
    /// `None` for Docker-published ports — see [`Endpoint::container`].
    pub pid: Option<u32>,
    /// Command name of the listening process, when visible.
    pub process: Option<String>,
    /// Name of the running container that publishes this port, if any.
    pub container: Option<String>,
}

impl Endpoint {
    /// `host:port`, bracketing IPv6 literals.
    pub fn addr(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Short owner text for tables: container name, else PID, else `None`.
    pub fn owner(&self) -> Option<String> {
        self.container
            .clone()
            .or_else(|| self.pid.map(|p| p.to_string()))
    }
}

/// A host-reachable port to probe, with its label.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredPort {
    pub label: String,
    /// Host to connect to (wildcard binds are mapped to `127.0.0.1`).
    pub host: String,
    pub port: u16,
}

/// Host ports a container publishes, from docker's `Ports` strings such as
/// `0.0.0.0:3000-3001->3000-3001/tcp` or `[::1]:9000->9000/tcp`. Unpublished
/// (`8787/tcp`) and UDP mappings are skipped.
pub fn published_ports(c: &Container) -> Vec<(String, u16)> {
    c.ports
        .iter()
        .filter_map(|p| {
            let (host_side, ctr_side) = p.split_once("->")?;
            if !ctr_side.ends_with("/tcp") {
                return None;
            }
            let (ip, spec) = host_side.rsplit_once(':')?;
            let host = compose::connect_host(ip);
            Some(
                compose::expand_range(spec)
                    .into_iter()
                    .map(move |port| (host.clone(), port)),
            )
        })
        .flatten()
        .collect()
}

/// Endpoints for this repo's running containers, labelled by Compose service
/// (or container name), sorted by port then label.
pub fn ports_from_containers(containers: &[Container]) -> Vec<DiscoveredPort> {
    let mut v: Vec<DiscoveredPort> = containers
        .iter()
        .filter(|c| c.in_repo && c.is_running())
        .flat_map(|c| {
            let label = if c.service.is_empty() {
                c.name.clone()
            } else {
                c.service.clone()
            };
            published_ports(c)
                .into_iter()
                .map(move |(host, port)| DiscoveredPort {
                    label: label.clone(),
                    host,
                    port,
                })
        })
        .collect();
    sort_discovered(&mut v);
    v
}

fn sort_discovered(v: &mut [DiscoveredPort]) {
    v.sort_by(|a, b| (a.port, &a.label, &a.host).cmp(&(b.port, &b.label, &b.host)));
}

/// Dedupe key: every spelling of loopback/wildcard is the same host.
fn host_key(host: &str) -> &str {
    match host.trim_matches(|c| c == '[' || c == ']') {
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "::" | "" => "loopback",
        other => other,
    }
}

/// Drop later entries that probe the same (normalised host, port).
fn dedupe(specs: Vec<DiscoveredPort>) -> Vec<DiscoveredPort> {
    let mut seen = HashSet::new();
    specs
        .into_iter()
        .filter(|d| seen.insert((host_key(&d.host).to_string(), d.port)))
        .collect()
}

/// TCP connect latency, or `None` if unreachable within the timeout.
fn probe(host: &str, port: u16) -> Option<u64> {
    let start = Instant::now();
    let target = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let sa = target.to_socket_addrs().ok()?.next()?;
    TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT).ok()?;
    Some(start.elapsed().as_millis() as u64)
}

/// Attach ownership: the publishing container wins over a `docker-proxy` PID.
fn build_endpoint(
    d: &DiscoveredPort,
    latency: Option<u64>,
    listeners: &BTreeMap<u16, Listener>,
    publishers: &HashMap<u16, String>,
) -> Endpoint {
    let l = listeners.get(&d.port);
    let container = publishers.get(&d.port).cloned();
    let is_proxy = l.and_then(|l| l.command.as_deref()) == Some("docker-proxy");
    let (pid, process) = match l {
        Some(l) if !(is_proxy || container.is_some() && l.pid.is_none()) => {
            (l.pid, l.command.clone())
        }
        _ => (None, None),
    };
    Endpoint {
        label: d.label.clone(),
        host: d.host.clone(),
        port: d.port,
        up: latency.is_some(),
        latency_ms: latency,
        pid,
        process,
        container,
    }
}

/// Map each host port published by a running container to that container.
fn publishers(containers: &[Container]) -> HashMap<u16, String> {
    let mut m = HashMap::new();
    for c in containers.iter().filter(|c| c.is_running()) {
        for (_, port) in published_ports(c) {
            m.entry(port).or_insert_with(|| c.name.clone());
        }
    }
    m
}

/// Last resort: every loopback-reachable listening port, sorted by port.
fn from_listeners(
    listeners: &BTreeMap<u16, Listener>,
    publishers: &HashMap<u16, String>,
) -> Vec<DiscoveredPort> {
    listeners
        .values()
        .filter(|l| l.loopback_reachable())
        .map(|l| DiscoveredPort {
            label: publishers
                .get(&l.port)
                .cloned()
                .or_else(|| l.command.clone())
                .unwrap_or_else(|| format!("port {}", l.port)),
            host: "127.0.0.1".into(),
            port: l.port,
        })
        .collect()
}

/// The ordered, deduped list of ports to probe (see the module docs).
pub fn endpoint_specs(
    cfg: &crate::config::Config,
    root: &str,
    containers: &[Container],
    listeners: &BTreeMap<u16, Listener>,
) -> Vec<DiscoveredPort> {
    let mut specs: Vec<DiscoveredPort> = cfg
        .endpoints
        .iter()
        .map(|e| DiscoveredPort {
            label: e.label.clone(),
            host: e.host.clone(),
            port: e.port,
        })
        .collect();
    if cfg.discover_endpoints {
        let mut found = ports_from_containers(containers);
        if found.is_empty() {
            found = compose::discover(root);
            sort_discovered(&mut found);
        }
        specs.extend(found);
    }
    if specs.is_empty() {
        specs = from_listeners(listeners, &publishers(containers));
    }
    dedupe(specs)
}

/// Gather endpoint health for the dashboard. `containers` is the full host
/// list from [`crate::collect::docker::gather_containers`] (one `docker ps`
/// serves both widgets). Probes run in parallel so a few dead ports cost one
/// connect timeout, not one each.
pub fn gather_endpoints(
    cfg: &crate::config::Config,
    root: &str,
    containers: &[Container],
) -> Vec<Endpoint> {
    let listeners = listen::gather();
    let specs = endpoint_specs(cfg, root, containers, &listeners);
    let pubs = publishers(containers);
    let latencies: Vec<Option<u64>> = std::thread::scope(|s| {
        let handles: Vec<_> = specs
            .iter()
            .map(|d| s.spawn(move || probe(&d.host, d.port)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(None))
            .collect()
    });
    specs
        .iter()
        .zip(latencies)
        .map(|(d, lat)| build_endpoint(d, lat, &listeners, &pubs))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, EndpointSpec};

    fn ctr(name: &str, service: &str, in_repo: bool, ports: &[&str]) -> Container {
        Container {
            name: name.into(),
            service: service.into(),
            state: "running".into(),
            in_repo,
            ports: ports.iter().map(|s| s.to_string()).collect(),
            ..Container::default()
        }
    }

    #[test]
    fn published_port_strings() {
        let c = ctr(
            "x",
            "",
            true,
            &[
                "0.0.0.0:8001->8080/tcp",
                "127.0.0.1:5432->5432/tcp",
                "0.0.0.0:3000-3001->3000-3001/tcp",
                "[::1]:9000->9000/tcp",
                "8787/tcp",
                "0.0.0.0:53->53/udp",
            ],
        );
        let got = published_ports(&c);
        let lo = |p| ("127.0.0.1".to_string(), p);
        assert_eq!(
            got,
            vec![lo(8001), lo(5432), lo(3000), lo(3001), ("::1".into(), 9000)]
        );
    }

    #[test]
    fn running_repo_containers_win_over_everything_else() {
        let containers = vec![
            ctr(
                "sovra-frontend-dev",
                "frontend",
                true,
                &["0.0.0.0:3000->3000/tcp"],
            ),
            ctr(
                "sovra-backend-dev",
                "backend",
                true,
                &["0.0.0.0:8001->8080/tcp"],
            ),
            // Another project's container on 8080 must not appear.
            ctr(
                "monika-emulators-1",
                "emulators",
                false,
                &["0.0.0.0:8080->8080/tcp"],
            ),
        ];
        let specs = endpoint_specs(&Config::default(), "", &containers, &BTreeMap::new());
        let got: Vec<(&str, u16)> = specs.iter().map(|d| (d.label.as_str(), d.port)).collect();
        assert_eq!(got, vec![("frontend", 3000), ("backend", 8001)]);
    }

    #[test]
    fn config_first_then_dedupe_normalises_loopback() {
        let cfg = Config {
            endpoints: vec![EndpointSpec {
                label: "web".into(),
                host: "localhost".into(),
                port: 3000,
            }],
            ..Config::default()
        };
        let containers = vec![ctr("fe", "frontend", true, &["0.0.0.0:3000->3000/tcp"])];
        let specs = endpoint_specs(&cfg, "", &containers, &BTreeMap::new());
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].label, "web");
    }

    #[test]
    fn listener_fallback_skips_non_loopback_binds() {
        let mut ls = BTreeMap::new();
        for (port, addr) in [
            (53, "127.0.0.53"),
            (631, "127.0.0.1"),
            (11435, "172.17.0.1"),
        ] {
            ls.insert(
                port,
                Listener {
                    port,
                    addr: addr.into(),
                    ..Listener::default()
                },
            );
        }
        let cfg = Config {
            discover_endpoints: false,
            ..Config::default()
        };
        let specs = endpoint_specs(&cfg, "", &[], &ls);
        let ports: Vec<u16> = specs.iter().map(|d| d.port).collect();
        assert_eq!(ports, vec![631]);
        assert_eq!(specs[0].label, "port 631");
    }

    #[test]
    fn docker_published_port_is_owned_by_its_container() {
        let d = DiscoveredPort {
            label: "backend".into(),
            host: "127.0.0.1".into(),
            port: 8001,
        };
        let mut ls = BTreeMap::new();
        ls.insert(
            8001,
            Listener {
                port: 8001,
                addr: "0.0.0.0".into(),
                pid: Some(42),
                command: Some("docker-proxy".into()),
            },
        );
        let pubs = HashMap::from([(8001u16, "sovra-backend-dev".to_string())]);
        let ep = build_endpoint(&d, Some(1), &ls, &pubs);
        assert_eq!(ep.pid, None);
        assert_eq!(ep.container.as_deref(), Some("sovra-backend-dev"));
        assert_eq!(ep.owner().as_deref(), Some("sovra-backend-dev"));

        // A plain host process keeps its PID.
        ls.insert(
            3000,
            Listener {
                port: 3000,
                addr: "*".into(),
                pid: Some(7),
                command: Some("node".into()),
            },
        );
        let d = DiscoveredPort { port: 3000, ..d };
        let ep = build_endpoint(&d, Some(1), &ls, &HashMap::new());
        assert_eq!((ep.pid, ep.process.as_deref()), (Some(7), Some("node")));
    }

    #[test]
    fn ipv6_addr_is_bracketed() {
        let ep = Endpoint {
            host: "::1".into(),
            port: 80,
            ..Endpoint::default()
        };
        assert_eq!(ep.addr(), "[::1]:80");
    }
}
