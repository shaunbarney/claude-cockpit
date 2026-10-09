//! Which process is listening on which TCP port — one snapshot per refresh.
//!
//! `lsof -nP -iTCP -sTCP:LISTEN -Fpcn` gives PIDs and command names (only for
//! your own processes unless run as root). On Linux, `ss -ltnpH` fills the
//! gaps: it lists *every* listening socket, with the owning process where the
//! kernel lets us see it. Ports published by Docker are held by a root-owned
//! `docker-proxy` (or by nftables with no process at all), so the caller maps
//! those to the publishing container instead.
use std::collections::BTreeMap;

use crate::proc::{stdout_of, DEFAULT_TIMEOUT};

/// One listening TCP socket.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Listener {
    pub port: u16,
    /// Bind address as printed (`127.0.0.1`, `*`, `[::]`, `0.0.0.0`).
    pub addr: String,
    pub pid: Option<u32>,
    pub command: Option<String>,
}

impl Listener {
    /// True if a connection to localhost would reach this socket.
    pub fn loopback_reachable(&self) -> bool {
        matches!(
            self.addr.trim_matches(|c| c == '[' || c == ']'),
            "*" | "0.0.0.0" | "::" | "127.0.0.1" | "::1" | "localhost"
        )
    }
}

/// Split `ADDR:PORT` (IPv6 in brackets, or `*:PORT`) at the last colon.
fn split_addr(tok: &str) -> Option<(String, u16)> {
    let (addr, port) = tok.rsplit_once(':')?;
    let addr = addr.split('%').next().unwrap_or(addr); // 127.0.0.53%lo
    Some((addr.to_string(), port.parse().ok()?))
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -Fpcn` field output.
pub fn parse_lsof_fields(out: &str) -> Vec<Listener> {
    let mut v = Vec::new();
    let (mut pid, mut cmd) = (None, None);
    for line in out.lines() {
        let (Some(tag), Some(val)) = (line.get(..1), line.get(1..)) else {
            continue;
        };
        match tag {
            "p" => {
                pid = val.parse().ok();
                cmd = None;
            }
            "c" => cmd = Some(val.to_string()),
            "n" => {
                if let Some((addr, port)) = split_addr(val) {
                    v.push(Listener {
                        port,
                        addr,
                        pid,
                        command: cmd.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    v
}

/// Parse `ss -ltnpH`:
/// `LISTEN 0 4096 127.0.0.1:11434 0.0.0.0:* users:(("ollama",pid=2377,fd=3))`.
pub fn parse_ss(out: &str) -> Vec<Listener> {
    out.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let (addr, port) = split_addr(cols.get(3)?)?;
            let users = line.split_once("users:((").map(|(_, u)| u);
            let command = users
                .and_then(|u| u.strip_prefix('"'))
                .and_then(|u| u.split_once('"'))
                .map(|(name, _)| name.to_string());
            let pid = users
                .and_then(|u| u.split_once("pid="))
                .and_then(|(_, p)| p.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|p| p.parse().ok());
            Some(Listener {
                port,
                addr,
                pid,
                command,
            })
        })
        .collect()
}

/// Merge listener lists into one per port, preferring entries that know their
/// PID and, among equals, loopback-reachable binds. Earlier lists win ties.
pub fn merge(lists: &[Vec<Listener>]) -> BTreeMap<u16, Listener> {
    let mut map: BTreeMap<u16, Listener> = BTreeMap::new();
    for l in lists.iter().flatten() {
        let better = match map.get(&l.port) {
            None => true,
            Some(cur) => {
                (cur.pid.is_none() && l.pid.is_some())
                    || (cur.pid.is_some() == l.pid.is_some()
                        && !cur.loopback_reachable()
                        && l.loopback_reachable())
            }
        };
        if better {
            map.insert(l.port, l.clone());
        }
    }
    map
}

/// Snapshot of every listening TCP port on this machine.
pub fn gather() -> BTreeMap<u16, Listener> {
    let lsof = parse_lsof_fields(&stdout_of(
        "lsof",
        &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"],
        DEFAULT_TIMEOUT,
    ));
    let ss = if cfg!(target_os = "linux") {
        parse_ss(&stdout_of("ss", &["-ltnpH"], DEFAULT_TIMEOUT))
    } else {
        Vec::new()
    };
    merge(&[lsof, ss])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_field_output() {
        let out =
            "p2377\ncollama\nn127.0.0.1:11434\np4377\ncllama-server\nn*:44387\nn[::1]:44388\n";
        let v = parse_lsof_fields(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].pid, Some(2377));
        assert_eq!(v[0].command.as_deref(), Some("ollama"));
        assert_eq!(v[1].port, 44387);
        assert_eq!(v[1].addr, "*");
        assert_eq!(v[2].addr, "[::1]");
        assert_eq!(v[2].command.as_deref(), Some("llama-server"));
    }

    #[test]
    fn ss_output_with_and_without_process() {
        let out = "LISTEN 0      4096       127.0.0.1:11434 0.0.0.0:* users:((\"ollama\",pid=2377,fd=3))\nLISTEN 0      4096         0.0.0.0:8080  0.0.0.0:*\nLISTEN 0      4096   127.0.0.53%lo:53    0.0.0.0:*\nLISTEN 0      4096            [::]:3000     [::]:*\n";
        let v = parse_ss(out);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].pid, Some(2377));
        assert_eq!(v[0].command.as_deref(), Some("ollama"));
        assert_eq!((v[1].port, v[1].pid), (8080, None));
        assert_eq!(v[2].addr, "127.0.0.53");
        assert!(!v[2].loopback_reachable());
        assert!(v[3].loopback_reachable());
    }

    #[test]
    fn merge_prefers_known_pid() {
        let a = vec![Listener {
            port: 80,
            addr: "0.0.0.0".into(),
            ..Listener::default()
        }];
        let b = vec![Listener {
            port: 80,
            addr: "0.0.0.0".into(),
            pid: Some(7),
            command: Some("nginx".into()),
        }];
        let m = merge(&[a, b]);
        assert_eq!(m[&80].pid, Some(7));
    }
}
