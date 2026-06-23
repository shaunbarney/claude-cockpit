//! Developer-process resource snapshot via sysinfo (claude/node/python/etc.).
use std::cmp::Ordering;

use sysinfo::System;

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cmd: String,
    pub cpu_pct: f32,
    pub mem_bytes: u64,
    pub uptime_secs: u64,
}

const DEFAULT_NAMES: &[&str] = &[
    "claude", "node", "python", "cargo", "rustc", "uvicorn", "vite",
    "next", "deno", "bun", "ruby", "postgres", "docker",
];

/// Is this process name one we monitor? Matches default dev tools + any config extras
/// (case-insensitive substring).
pub fn is_dev_proc(name: &str, extras: &[String]) -> bool {
    let n = name.to_lowercase();
    DEFAULT_NAMES.iter().any(|d| n.contains(d))
        || extras.iter().any(|e| n.contains(&e.to_lowercase()))
}

/// Sort key: CPU% desc, then memory desc.
pub fn rank_proc(a: &Proc, b: &Proc) -> Ordering {
    b.cpu_pct
        .partial_cmp(&a.cpu_pct)
        .unwrap_or(Ordering::Equal)
        .then(b.mem_bytes.cmp(&a.mem_bytes))
}

/// Refresh `sys` and return ranked dev-process snapshots. `sys` MUST be reused across
/// calls — sysinfo needs two samples to compute CPU%, so a fresh System reports 0% first tick.
pub fn gather_procs(sys: &mut System, extras: &[String]) -> Vec<Proc> {
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let mut v: Vec<Proc> = sys
        .processes()
        .values()
        .filter(|p| is_dev_proc(&p.name().to_string_lossy(), extras))
        .map(|p| Proc {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            cmd: p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            cpu_pct: p.cpu_usage(),
            mem_bytes: p.memory(),
            uptime_secs: p.run_time(),
        })
        .collect();
    v.sort_by(rank_proc);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(cpu: f32, mem: u64) -> Proc {
        Proc {
            pid: 1,
            name: "x".into(),
            cmd: String::new(),
            cpu_pct: cpu,
            mem_bytes: mem,
            uptime_secs: 0,
        }
    }

    #[test]
    fn matches_dev_processes() {
        assert!(is_dev_proc("claude", &[]));
        assert!(is_dev_proc("node", &[]));
        assert!(is_dev_proc("python3.12", &[]));
        assert!(!is_dev_proc("Finder", &[]));
        assert!(is_dev_proc("myapp", &["myapp".to_string()])); // config extra
    }

    #[test]
    fn ranks_by_cpu_then_mem() {
        let mut v = vec![proc(1.0, 100), proc(9.0, 10), proc(1.0, 500)];
        v.sort_by(rank_proc);
        assert_eq!(v[0].cpu_pct, 9.0);
        assert_eq!(v[1].mem_bytes, 500); // tie on cpu (1.0) → higher mem first
    }
}
