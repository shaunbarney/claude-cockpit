//! Background data refresh: gather collectors off the UI thread, publish into shared state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::app::DashboardData;
use crate::collect::git::{self, Worktree};
use crate::collect::jobs::{self, Job};
use crate::collect::loc::{self, LocRow};

pub fn publish_worktrees(data: &Arc<Mutex<DashboardData>>, wts: Vec<Worktree>) {
    let mut d = data.lock().unwrap();
    d.worktrees = wts;
    d.rev += 1;
}

pub fn publish_loc(data: &Arc<Mutex<DashboardData>>, loc: Vec<LocRow>) {
    let mut d = data.lock().unwrap();
    d.loc = loc;
    d.rev += 1;
}

pub fn publish_jobs(data: &Arc<Mutex<DashboardData>>, jobs: Vec<Job>) {
    let mut d = data.lock().unwrap();
    d.jobs = jobs;
    d.rev += 1;
}

pub fn publish_usage(data: &Arc<Mutex<DashboardData>>, totals: crate::collect::usage::UsageTotals) {
    let mut d = data.lock().unwrap();
    d.usage = Some(totals);
    d.rev += 1;
}

pub fn publish_activity(data: &Arc<Mutex<DashboardData>>, timestamps_ms: Vec<i64>) {
    // Daily prompt cadence drives the full-history detail view. (The dashboard
    // Rate widget gets its token-rate trend from the usage scan, not here.)
    let counts = crate::collect::activity::daily_counts(&timestamps_ms);
    let mut d = data.lock().unwrap();
    d.activity = counts;
    d.rev += 1;
}

pub fn publish_containers(
    data: &Arc<Mutex<DashboardData>>,
    list: Vec<crate::collect::docker::Container>,
) {
    let mut d = data.lock().unwrap();
    d.containers = list;
    d.rev += 1;
}

pub fn publish_endpoints(
    data: &Arc<Mutex<DashboardData>>,
    list: Vec<crate::collect::ports::Endpoint>,
) {
    let mut d = data.lock().unwrap();
    d.endpoints = list;
    d.rev += 1;
}

pub fn publish_tools(data: &Arc<Mutex<DashboardData>>, list: Vec<crate::collect::tools::ToolStat>) {
    let mut d = data.lock().unwrap();
    d.tools = list;
    d.rev += 1;
}

pub fn publish_repo(data: &Arc<Mutex<DashboardData>>, h: crate::collect::git::RepoHealth) {
    let mut d = data.lock().unwrap();
    d.repo = Some(h);
    d.rev += 1;
}

/// Gather everything except the network `git fetch` and publish it. This is the
/// per-tick work of the slow loop; `git fetch` is throttled separately.
fn gather_all(root: &str, data: &Arc<Mutex<DashboardData>>) {
    publish_worktrees(data, git::gather_worktrees(root));
    publish_loc(data, loc::loc_rows(root));
    publish_jobs(data, jobs::gather_jobs());
    publish_activity(data, crate::collect::activity::read_history());
    publish_containers(data, crate::collect::docker::gather_containers());
    publish_endpoints(
        data,
        crate::collect::ports::gather_endpoints(&crate::config::load(), root),
    );
    publish_repo(data, git::repo_health(root));
}

/// One full gather + publish, including a `git fetch` (used at startup and on
/// the manual `r` refresh, where the user expects freshly fetched remotes).
pub fn refresh_now(root: &str, data: &Arc<Mutex<DashboardData>>) {
    git::fetch_origin(root);
    gather_all(root, data);
}

/// Spawn the slow (10 s) refresh loop and a fast (2 s) jobs loop.
/// Both threads share the same returned stop flag.
pub fn spawn(root: String, data: Arc<Mutex<DashboardData>>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));

    // Slow thread: worktrees/loc/jobs/activity/docker/endpoints/repo every 10 s.
    // `git fetch` is network-bound, so it's throttled to every 6th tick (~60 s)
    // rather than run every 10 s.
    let s = stop.clone();
    let root2 = root.clone();
    let data2 = data.clone();
    thread::spawn(move || {
        let mut tick: u32 = 0;
        while !s.load(Ordering::Relaxed) {
            if tick % 6 == 0 {
                git::fetch_origin(&root2);
            }
            gather_all(&root2, &data2);
            tick = tick.wrapping_add(1);
            // Sleep in 100 ms slices so the stop flag is checked promptly.
            for _ in 0..100 {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    // Fast thread: jobs every 2 s (20 × 100 ms ticks) — the one widget that
    // genuinely benefits from a tight cadence (live agent state).
    let s = stop.clone();
    let data3 = data.clone();
    thread::spawn(move || {
        while !s.load(Ordering::Relaxed) {
            publish_jobs(&data3, jobs::gather_jobs());
            for _ in 0..20 {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    // Cost thread: usage scan + tool-use scan every 10 s (100 × 100 ms ticks).
    let s = stop.clone();
    thread::spawn(move || {
        // Best-effort live price refresh at startup; ignore result.
        let _ = crate::collect::pricing::fetch_litellm();
        let mut cache = crate::collect::usage::UsageCache::new();
        let mut tool_cache = crate::collect::tools::ToolCache::new();
        while !s.load(Ordering::Relaxed) {
            publish_usage(&data, crate::collect::usage::scan_all(&mut cache));
            let now_ms = chrono::Utc::now().timestamp_millis();
            publish_tools(
                &data,
                crate::collect::tools::scan_tools(&mut tool_cache, now_ms, 7),
            );
            for _ in 0..100 {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    stop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_swaps_only_its_slice() {
        let data = Arc::new(Mutex::new(DashboardData::default()));
        // Seed the loc slice so we can prove worktrees publish doesn't clobber it.
        data.lock().unwrap().loc.push(LocRow {
            language: "Rust".into(),
            files: 1,
            lines: 13,
            code: 10,
        });

        publish_worktrees(
            &data,
            vec![Worktree {
                name: "x".into(),
                path: String::new(),
                branch: String::new(),
                ahead: 0,
                dirty: 0,
                committed: (0, 0),
                uncommitted: (0, 0),
                age: String::new(),
            }],
        );

        let d = data.lock().unwrap();
        assert_eq!(d.worktrees.len(), 1);
        assert_eq!(d.loc.len(), 1); // untouched
    }
}
