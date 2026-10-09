//! Docker container monitoring via the `docker` CLI (no daemon library dependency).
//! Everything is best-effort: if `docker` is missing or the daemon is down, the
//! gather functions return empty and never panic. Every call is bounded by a
//! timeout so a wedged daemon can't stall a refresh thread.
//!
//! Containers are tagged [`Container::in_repo`] when their Compose
//! `working_dir` label sits under one of this repository's worktrees (or their
//! Compose project name matches the repo's), so the dashboard can show *this
//! project's* stack by default instead of every container on the host.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::proc::{output_with_timeout, stdout_of, DEFAULT_TIMEOUT};

/// `docker stats --no-stream` samples for ~2 s per container batch.
const STATS_TIMEOUT: Duration = Duration::from_secs(8);

const LABEL_WORKING_DIR: &str = "com.docker.compose.project.working_dir";
const LABEL_SERVICE: &str = "com.docker.compose.service";
const LABEL_PROJECT: &str = "com.docker.compose.project";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub state: String,
    pub status: String,
    pub health: Option<String>,
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_limit: u64,
    /// Published port mappings as docker prints them (`0.0.0.0:8080->80/tcp`),
    /// with the IPv6 `[::]` twin of an IPv4 mapping dropped.
    pub ports: Vec<String>,
    /// Compose project label (empty for non-Compose containers).
    pub project: String,
    /// Compose service label (empty for non-Compose containers).
    pub service: String,
    /// Compose `working_dir` label: the directory `docker compose` ran in.
    pub working_dir: String,
    /// True when this container belongs to the current repository.
    pub in_repo: bool,
}

impl Container {
    /// True for a container that is currently up.
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
}

/// Which containers the dashboard shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// This repository's containers (falls back to all when there are none).
    #[default]
    Repo,
    /// Every container on the host.
    All,
}

/// Extract a health word from a status string, e.g. "Up 2 hours (healthy)" -> "healthy".
/// Only recognised health words are returned; numeric exit codes are ignored.
fn extract_health(status: &str) -> Option<String> {
    let start = status.find('(')?;
    let rest = &status[start + 1..];
    let end = rest.find(')')?;
    let word = &rest[..end];
    matches!(
        word,
        "healthy" | "unhealthy" | "starting" | "health: starting"
    )
    .then(|| word.to_string())
}

/// Split docker's `Ports` column and drop IPv6 duplicates of IPv4 mappings
/// (`[::]:8080->80/tcp` next to `0.0.0.0:8080->80/tcp`).
fn split_ports(raw: &str) -> Vec<String> {
    let all: Vec<&str> = raw
        .split(", ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    all.iter()
        .filter(|p| {
            let v6_rest = p.strip_prefix("[::]:").or_else(|| p.strip_prefix(":::"));
            match v6_rest {
                Some(rest) => !all.contains(&format!("0.0.0.0:{rest}").as_str()),
                None => true,
            }
        })
        .map(|s| s.to_string())
        .collect()
}

/// The `docker ps` format: the JSON record, then the Compose labels we scope
/// by, tab-separated (JSON never contains a raw tab).
fn ps_format() -> String {
    format!(
        "{{{{json .}}}}\t{{{{.Label \"{LABEL_WORKING_DIR}\"}}}}\t{{{{.Label \"{LABEL_SERVICE}\"}}}}\t{{{{.Label \"{LABEL_PROJECT}\"}}}}"
    )
}

/// Parse one `docker ps` line: either a bare `{{json .}}` record or the
/// labelled form `gather_containers` asks for: the JSON, then tab-separated
/// Compose labels (working dir, service, project).
pub fn parse_ps_line(line: &str) -> Option<Container> {
    let mut fields = line.split('\t');
    let v: Value = serde_json::from_str(fields.next()?).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut next = || fields.next().unwrap_or("").trim().to_string();
    let (working_dir, service, project) = (next(), next(), next());
    let status = s("Status");
    Some(Container {
        id: s("ID"),
        name: s("Names"),
        state: s("State"),
        health: extract_health(&status),
        status,
        ports: split_ports(&s("Ports")),
        working_dir,
        service,
        project,
        ..Container::default()
    })
}

/// Parse a memory token like "256MiB" / "1.5GiB" / "0B" into bytes.
pub fn parse_mem(tok: &str) -> u64 {
    const UNITS: &[(&str, f64)] = &[
        ("TiB", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("GiB", 1024.0 * 1024.0 * 1024.0),
        ("MiB", 1024.0 * 1024.0),
        ("KiB", 1024.0),
        ("TB", 1e12),
        ("GB", 1e9),
        ("MB", 1e6),
        ("kB", 1e3),
        ("KB", 1e3),
        ("B", 1.0),
    ];
    let tok = tok.trim();
    let (num, mult) = UNITS
        .iter()
        .find_map(|(suffix, m)| tok.strip_suffix(suffix).map(|n| (n, *m)))
        .unwrap_or((tok, 1.0));
    (num.trim().parse::<f64>().unwrap_or(0.0) * mult) as u64
}

/// Parse one `docker stats --no-stream --format '{{json .}}'` line -> (id, cpu%, used, limit).
pub fn parse_stats_line(line: &str) -> Option<(String, f32, u64, u64)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let id = v
        .get("Container")
        .or_else(|| v.get("ID"))
        .and_then(|x| x.as_str())?
        .trim()
        .to_string();
    if id.is_empty() {
        return None;
    }
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

/// True if a stats row id (short id, full id, or name) refers to `c`.
fn stats_match(c: &Container, id: &str) -> bool {
    !id.is_empty() && (c.id == id || c.name == id || (id.len() >= 12 && c.id.starts_with(id)))
}

/// Docker Compose's default project name for a directory: lower-cased, with
/// everything but `[a-z0-9_-]` removed.
pub fn compose_project_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// Does `c` belong to the repository whose worktrees are `roots`?
///
/// A container belongs if its Compose `working_dir` is inside any worktree, or
/// its Compose project equals a worktree directory's default project name.
pub fn belongs_to(c: &Container, roots: &[PathBuf]) -> bool {
    if !c.working_dir.is_empty() {
        let wd = Path::new(&c.working_dir);
        if roots.iter().any(|r| wd.starts_with(r)) {
            return true;
        }
    }
    !c.project.is_empty() && roots.iter().any(|r| compose_project_name(r) == c.project)
}

/// Every worktree path of the repo at `root` (main checkout included), plus
/// `root` itself; canonicalised where possible. Empty when `root` is empty.
pub fn repo_scope_roots(root: &str) -> Vec<PathBuf> {
    if root.is_empty() {
        return Vec::new();
    }
    let porcelain = stdout_of(
        "git",
        &["-C", root, "worktree", "list", "--porcelain"],
        DEFAULT_TIMEOUT,
    );
    let mut roots: Vec<PathBuf> = std::iter::once(root)
        .chain(
            porcelain
                .lines()
                .filter_map(|l| l.strip_prefix("worktree ")),
        )
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p)))
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// Narrow the full container list to what the widget shows for `scope`.
///
/// `Repo`: this repo's containers; if it has none, every container (the widget
/// then notes the fallback, since no row is `in_repo`). In both scopes exited
/// containers are hidden unless nothing is running.
pub fn scoped(all: &[Container], scope: Scope) -> Vec<Container> {
    let pool: Vec<&Container> = match scope {
        Scope::Repo if all.iter().any(|c| c.in_repo) => all.iter().filter(|c| c.in_repo).collect(),
        _ => all.iter().collect(),
    };
    let running: Vec<Container> = pool
        .iter()
        .filter(|c| c.is_running())
        .map(|c| (*c).clone())
        .collect();
    if running.is_empty() {
        pool.into_iter().cloned().collect()
    } else {
        running
    }
}

/// Every container on the host (running and exited), joined with `docker
/// stats` and tagged `in_repo` against `roots`. Empty if docker is unavailable.
pub fn gather_containers(roots: &[PathBuf]) -> Vec<Container> {
    let fmt = ps_format();
    let ps = stdout_of(
        "docker",
        &["ps", "--all", "--no-trunc", "--format", &fmt],
        DEFAULT_TIMEOUT,
    );
    let mut containers: Vec<Container> = ps.lines().filter_map(parse_ps_line).collect();
    for c in containers.iter_mut() {
        c.in_repo = belongs_to(c, roots);
    }
    if containers.iter().any(Container::is_running) {
        let stats = stdout_of(
            "docker",
            &["stats", "--no-stream", "--format", "{{json .}}"],
            STATS_TIMEOUT,
        );
        apply_stats(&mut containers, &stats);
    }
    containers
}

fn apply_stats(containers: &mut [Container], stats: &str) {
    for (id, cpu, used, limit) in stats.lines().filter_map(parse_stats_line) {
        for c in containers.iter_mut().filter(|c| stats_match(c, &id)) {
            c.cpu_pct = cpu;
            c.mem_used = used;
            c.mem_limit = limit;
        }
    }
}

/// Merge `docker logs --timestamps` stdout and stderr into one chronological
/// stream, then drop the timestamp prefix. Docker's RFC 3339 UTC timestamps
/// sort lexically; the sort is stable so same-instant lines keep their order.
pub fn merge_timestamped(stdout: &str, stderr: &str, tail: usize) -> Vec<String> {
    let mut lines: Vec<&str> = stdout.lines().chain(stderr.lines()).collect();
    lines.sort_by_key(|l| l.split_once(' ').map(|(ts, _)| ts).unwrap_or(""));
    let skip = lines.len().saturating_sub(tail);
    lines
        .into_iter()
        .skip(skip)
        .map(|l| match l.split_once(' ') {
            Some((ts, rest)) if ts.len() >= 20 && ts.as_bytes()[4] == b'-' => rest.to_string(),
            _ => l.to_string(),
        })
        .collect()
}

/// Recent logs for a container (best-effort), stdout and stderr interleaved in
/// time order. Oldest first; the detail view opens scrolled to the bottom.
pub fn container_logs(id: &str, tail: usize) -> Vec<String> {
    let t = tail.to_string();
    let Some(o) = output_with_timeout(
        Command::new("docker").args(["logs", "--timestamps", "--tail", &t, id]),
        DEFAULT_TIMEOUT,
    ) else {
        return vec![];
    };
    merge_timestamped(
        &String::from_utf8_lossy(&o.stdout),
        &String::from_utf8_lossy(&o.stderr),
        tail,
    )
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
        assert!(c.working_dir.is_empty() && !c.in_repo);
    }

    #[test]
    fn parses_labels_and_drops_v6_twins() {
        let l = "{\"ID\":\"abc\",\"Names\":\"sovra-frontend-dev\",\"State\":\"running\",\"Status\":\"Up\",\"Ports\":\"0.0.0.0:3000->3000/tcp, [::]:3000->3000/tcp, [::1]:9000->9000/tcp\"}\t/repo/deployments/docker\tfrontend\tsovra";
        let c = parse_ps_line(l).unwrap();
        assert_eq!(c.working_dir, "/repo/deployments/docker");
        assert_eq!(c.service, "frontend");
        assert_eq!(c.project, "sovra");
        assert_eq!(
            c.ports,
            vec!["0.0.0.0:3000->3000/tcp", "[::1]:9000->9000/tcp"]
        );
    }

    #[test]
    fn scoping_by_working_dir_and_project() {
        let roots = vec![PathBuf::from("/home/u/MyRepo")];
        let mk = |wd: &str, project: &str| Container {
            working_dir: wd.into(),
            project: project.into(),
            ..Container::default()
        };
        assert!(belongs_to(&mk("/home/u/MyRepo/deploy", "x"), &roots));
        assert!(belongs_to(&mk("/home/u/MyRepo", ""), &roots));
        assert!(!belongs_to(&mk("/home/u/MyRepoOther", "x"), &roots));
        assert!(belongs_to(&mk("/elsewhere", "myrepo"), &roots));
        assert!(!belongs_to(&mk("", ""), &roots));
    }

    #[test]
    fn scoped_prefers_repo_running_then_falls_back() {
        let mk = |name: &str, state: &str, in_repo: bool| Container {
            name: name.into(),
            state: state.into(),
            in_repo,
            ..Container::default()
        };
        let all = vec![
            mk("mine", "running", true),
            mk("mine-old", "exited", true),
            mk("theirs", "running", false),
        ];
        let names = |v: Vec<Container>| v.into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(scoped(&all, Scope::Repo)), vec!["mine"]);
        assert_eq!(names(scoped(&all, Scope::All)), vec!["mine", "theirs"]);
        // Nothing in the repo: fall back to everything (running first).
        let others = vec![mk("a", "running", false), mk("b", "exited", false)];
        assert_eq!(names(scoped(&others, Scope::Repo)), vec!["a"]);
        // Only exited containers: show them rather than nothing.
        let dead = vec![mk("x", "exited", true)];
        assert_eq!(names(scoped(&dead, Scope::Repo)), vec!["x"]);
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
    fn empty_stats_id_matches_nothing() {
        assert!(parse_stats_line(r#"{"Container":"","CPUPerc":"1%"}"#).is_none());
        let c = Container {
            id: "abcdef0123456789".into(),
            name: "web".into(),
            ..Container::default()
        };
        assert!(!stats_match(&c, ""));
        assert!(!stats_match(&c, "ab")); // too short to be an id prefix
        assert!(stats_match(&c, "abcdef012345"));
        assert!(stats_match(&c, "web"));
    }

    #[test]
    fn parses_mem_units() {
        assert_eq!(parse_mem("256MiB"), 256 * 1024 * 1024);
        assert_eq!(parse_mem("2GiB"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_mem("1TiB"), 1024u64.pow(4));
        assert_eq!(parse_mem("1.5kB"), 1500);
        assert_eq!(parse_mem("0B"), 0);
    }

    #[test]
    fn merges_log_streams_chronologically() {
        let out = "2026-01-01T00:00:01.000000000Z out-1\n2026-01-01T00:00:03.000000000Z out-3\n";
        let err = "2026-01-01T00:00:02.000000000Z err-2\n";
        assert_eq!(
            merge_timestamped(out, err, 10),
            vec!["out-1", "err-2", "out-3"]
        );
        assert_eq!(merge_timestamped(out, err, 2), vec!["err-2", "out-3"]);
    }

    #[test]
    fn no_health_marker() {
        let l = r#"{"ID":"x","Names":"n","State":"exited","Status":"Exited (0) 3 minutes ago","Ports":""}"#;
        let c = parse_ps_line(l).unwrap();
        assert_eq!(c.health, None);
        assert!(c.ports.is_empty());
    }

    #[test]
    fn compose_project_names() {
        assert_eq!(
            compose_project_name(Path::new("/x/SovraSRX.Production")),
            "sovrasrxproduction"
        );
    }
}
