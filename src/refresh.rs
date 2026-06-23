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
    data.lock().unwrap().worktrees = wts;
}

pub fn publish_loc(data: &Arc<Mutex<DashboardData>>, loc: Vec<LocRow>) {
    data.lock().unwrap().loc = loc;
}

pub fn publish_jobs(data: &Arc<Mutex<DashboardData>>, jobs: Vec<Job>) {
    data.lock().unwrap().jobs = jobs;
}

pub fn publish_usage(
    data: &Arc<Mutex<DashboardData>>,
    totals: crate::collect::usage::UsageTotals,
) {
    data.lock().unwrap().usage = Some(totals);
}

pub fn publish_activity(data: &Arc<Mutex<DashboardData>>, counts: Vec<(String, u32)>) {
    data.lock().unwrap().activity = counts;
}

pub fn publish_containers(
    data: &Arc<Mutex<DashboardData>>,
    list: Vec<crate::collect::docker::Container>,
) {
    data.lock().unwrap().containers = list;
}

pub fn publish_endpoints(
    data: &Arc<Mutex<DashboardData>>,
    list: Vec<crate::collect::ports::Endpoint>,
) {
    data.lock().unwrap().endpoints = list;
}

/// One full gather + publish (used at startup and on `r`).
pub fn refresh_now(root: &str, data: &Arc<Mutex<DashboardData>>) {
    git::fetch_origin(root);
    publish_worktrees(data, git::gather_worktrees(root));
    publish_loc(data, loc::loc_rows(root));
    publish_jobs(data, jobs::gather_jobs());
    publish_activity(
        data,
        crate::collect::activity::daily_counts(&crate::collect::activity::read_history()),
    );
    publish_containers(data, crate::collect::docker::gather_containers());
    publish_endpoints(
        data,
        crate::collect::ports::gather_endpoints(&crate::config::load()),
    );
}

/// Spawn the slow (10 s) refresh loop and a fast (2 s) jobs loop.
/// Both threads share the same returned stop flag.
pub fn spawn(root: String, data: Arc<Mutex<DashboardData>>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));

    // Slow thread: full refresh every 10 s (includes docker).
    let s = stop.clone();
    let root2 = root.clone();
    let data2 = data.clone();
    thread::spawn(move || {
        while !s.load(Ordering::Relaxed) {
            refresh_now(&root2, &data2);
            // Sleep in 100 ms slices so the stop flag is checked promptly.
            for _ in 0..100 {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    // Fast thread: jobs + endpoints refresh every 2 s (20 × 100 ms ticks).
    let s = stop.clone();
    let data3 = data.clone();
    thread::spawn(move || {
        while !s.load(Ordering::Relaxed) {
            publish_jobs(&data3, jobs::gather_jobs());
            publish_endpoints(
                &data3,
                crate::collect::ports::gather_endpoints(&crate::config::load()),
            );
            for _ in 0..20 {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    // Cost thread: usage scan every 10 s (100 × 100 ms ticks).
    let s = stop.clone();
    thread::spawn(move || {
        // Best-effort live price refresh at startup; ignore result.
        let _ = crate::collect::pricing::fetch_litellm();
        let mut cache = crate::collect::usage::UsageCache::new();
        while !s.load(Ordering::Relaxed) {
            publish_usage(&data, crate::collect::usage::scan_all(&mut cache));
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
        data.lock().unwrap().loc.push(LocRow { language: "Rust".into(), files: 1, code: 10 });

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
