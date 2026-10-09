//! Background data refresh: gather collectors off the UI thread, publish into shared state.
//!
//! Three threads (see [`spawn`]): **slow** (10 s: worktrees, LOC, docker,
//! endpoints…), **fast** (2 s: jobs) and **cost** (10 s: transcript scans).
//!
//! Robustness rules:
//! - Every collector call runs under [`std::panic::catch_unwind`]; a panic
//!   keeps the slice's previous data, is recorded in [`health`], and the
//!   thread carries on (its message is kept off the terminal so it can't
//!   tear down the TUI).
//! - Locks are poison-tolerant, so one panic can't wedge every later publish.
//! - A manual refresh ([`refresh_now`] / [`request_now`]) *wakes the slow
//!   thread* rather than running a second, racing `gather_all`.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::app::DashboardData;
use crate::collect::docker::{self, Container, Scope};
use crate::collect::git::{self, Worktree};
use crate::collect::jobs::{self, Job};
use crate::collect::loc::{self, LocRow};
use crate::proc::{output_with_timeout, stdout_of, DEFAULT_TIMEOUT};

/// Slow-loop cadence.
const SLOW_EVERY: Duration = Duration::from_secs(10);
/// `git fetch` is network-bound: at most this often in the background.
pub const FETCH_EVERY: Duration = Duration::from_secs(60);
/// Hard cap on one `git fetch`.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a manual refresh waits for the slow thread before giving up.
const MANUAL_WAIT: Duration = Duration::from_secs(120);

/// Lock shared state, recovering the data if a previous holder panicked.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn publish_repo(data: &Arc<Mutex<DashboardData>>, info: crate::collect::git::RepoInfo) {
    let mut d = lock(data);
    d.repo = info;
    d.rev += 1;
}

pub fn publish_worktrees(data: &Arc<Mutex<DashboardData>>, wts: Vec<Worktree>) {
    let mut d = lock(data);
    d.worktrees = wts;
    d.rev += 1;
}

pub fn publish_loc(data: &Arc<Mutex<DashboardData>>, loc: Vec<LocRow>) {
    let mut d = lock(data);
    d.loc = loc;
    d.rev += 1;
}

pub fn publish_jobs(data: &Arc<Mutex<DashboardData>>, jobs: Vec<Job>) {
    let mut d = lock(data);
    d.jobs = jobs;
    d.rev += 1;
}

pub fn publish_usage(data: &Arc<Mutex<DashboardData>>, totals: crate::collect::usage::UsageTotals) {
    let mut d = lock(data);
    d.usage = Some(totals);
    d.rev += 1;
}

pub fn publish_activity(data: &Arc<Mutex<DashboardData>>, timestamps_ms: Vec<i64>) {
    // Daily prompt cadence drives the full-history detail view. (The dashboard
    // Rate widget gets its token-rate trend from the usage scan, not here.)
    let counts = crate::collect::activity::daily_counts(&timestamps_ms);
    let mut d = lock(data);
    d.activity = counts;
    d.rev += 1;
}

pub fn publish_containers(data: &Arc<Mutex<DashboardData>>, list: Vec<Container>) {
    let mut d = lock(data);
    d.containers = list;
    d.rev += 1;
}

pub fn publish_endpoints(
    data: &Arc<Mutex<DashboardData>>,
    list: Vec<crate::collect::ports::Endpoint>,
) {
    let mut d = lock(data);
    d.endpoints = list;
    d.rev += 1;
}

pub fn publish_tools(data: &Arc<Mutex<DashboardData>>, list: Vec<crate::collect::tools::ToolStat>) {
    let mut d = lock(data);
    d.tools = list;
    d.rev += 1;
}

pub fn publish_skills(data: &Arc<Mutex<DashboardData>>, list: Vec<crate::collect::skills::Skill>) {
    let mut d = lock(data);
    d.skills = list;
    d.rev += 1;
}

/// Last failure of one data slice (cleared when the slice next succeeds).
#[derive(Debug, Clone, PartialEq)]
pub struct SliceError {
    pub slice: &'static str,
    pub message: String,
    pub at: SystemTime,
}

static HEALTH: Mutex<BTreeMap<&'static str, SliceError>> = Mutex::new(BTreeMap::new());

thread_local! {
    static LAST_PANIC: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Slices whose most recent collection panicked, sorted by slice name. Empty
/// when everything is healthy. (Intended for a status line in the UI.)
pub fn health() -> Vec<SliceError> {
    lock(&HEALTH).values().cloned().collect()
}

/// Thread-name prefix for refresh threads; panics on them are captured.
const THREAD_PREFIX: &str = "cockpit-";

/// Route panics on refresh threads into [`health`] instead of the previous
/// hook (ratatui's hook restores the terminal, which would wreck a running
/// TUI over a recoverable collector bug). Other threads are unaffected.
fn install_panic_capture() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let ours = thread::current()
                .name()
                .is_some_and(|n| n.starts_with(THREAD_PREFIX));
            if !ours {
                return prev(info);
            }
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            let at = info
                .location()
                .map(|l| format!(" at {}:{}", l.file(), l.line()))
                .unwrap_or_default();
            LAST_PANIC.with(|p| *p.borrow_mut() = Some(format!("{msg}{at}")));
        }));
    });
}

/// Run one collector, catching a panic. `None` means it panicked (recorded in
/// [`health`]); the caller then leaves the slice's previous data in place.
pub fn guarded<T>(slice: &'static str, f: impl FnOnce() -> T) -> Option<T> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => {
            lock(&HEALTH).remove(slice);
            Some(v)
        }
        Err(payload) => {
            let message = LAST_PANIC
                .with(|p| p.borrow_mut().take())
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            lock(&HEALTH).insert(
                slice,
                SliceError {
                    slice,
                    message,
                    at: SystemTime::now(),
                },
            );
            None
        }
    }
}

/// Full host container list from the last slow tick (all projects, exited
/// included). `DashboardData::containers` holds only the scoped view.
static ALL_CONTAINERS: Mutex<Vec<Container>> = Mutex::new(Vec::new());
/// Current [`Scope`] for the Docker widget (0 = Repo, 1 = All).
static DOCKER_SCOPE: AtomicU8 = AtomicU8::new(0);

/// Every container seen on the host at the last refresh.
pub fn all_containers() -> Vec<Container> {
    lock(&ALL_CONTAINERS).clone()
}

/// The Docker widget's current scope.
pub fn docker_scope() -> Scope {
    match DOCKER_SCOPE.load(Ordering::Relaxed) {
        1 => Scope::All,
        _ => Scope::Repo,
    }
}

/// Switch the Docker widget between this repo's containers and all of them,
/// republishing immediately from the last snapshot (no docker call).
pub fn set_docker_scope(data: &Arc<Mutex<DashboardData>>, scope: Scope) {
    DOCKER_SCOPE.store(matches!(scope, Scope::All) as u8, Ordering::Relaxed);
    let all = all_containers();
    publish_containers(data, docker::scoped(&all, scope));
}

/// `git fetch origin --quiet`, bounded by a timeout and never prompting.
pub fn fetch_origin_bounded(root: &str) {
    if root.is_empty() {
        return;
    }
    let _ = output_with_timeout(
        Command::new("git")
            .args(["-C", root, "fetch", "origin", "--quiet"])
            .env("GIT_TERMINAL_PROMPT", "0"),
        FETCH_TIMEOUT,
    );
}

/// LOC is a full tokei pass over every tracked file; recompute it only when
/// `HEAD` or the index changed.
#[derive(Default)]
struct LocCache {
    key: Option<(String, Option<SystemTime>)>,
}

impl LocCache {
    fn key(root: &str) -> Option<(String, Option<SystemTime>)> {
        let out = stdout_of(
            "git",
            &["-C", root, "rev-parse", "HEAD", "--git-path", "index"],
            DEFAULT_TIMEOUT,
        );
        let mut lines = out.lines();
        let head = lines.next()?.trim().to_string();
        let index = PathBuf::from(lines.next()?.trim());
        let index = if index.is_absolute() {
            index
        } else {
            PathBuf::from(root).join(index)
        };
        let mtime = std::fs::metadata(index).and_then(|m| m.modified()).ok();
        Some((head, mtime))
    }
}

/// Per-thread memory for the slow loop.
#[derive(Default)]
struct SlowState {
    loc: LocCache,
}

/// Gather everything except the network `git fetch` and publish it. This is the
/// per-tick work of the slow loop; `git fetch` is throttled separately.
/// (Skill usage is scanned on the cost thread, alongside the transcript walk.)
/// With no repo (`root` empty) the git-backed slices are skipped.
fn gather_all(root: &str, data: &Arc<Mutex<DashboardData>>, st: &mut SlowState) {
    let has_repo = !root.is_empty();
    if has_repo {
        if let Some(info) = guarded("repo", || git::repo_info(root)) {
            publish_repo(data, info);
        }
    }
    let wts = if has_repo {
        guarded("worktrees", || git::gather_worktrees(root))
    } else {
        Some(Vec::new())
    };
    let wts_for_jobs = wts.clone().unwrap_or_else(|| lock(data).worktrees.clone());
    // Jobs list includes a synthetic "no agent" row for each worktree no job owns.
    if let Some(j) = guarded("jobs", || {
        jobs::merge_orphan_worktrees(jobs::gather_jobs(), &wts_for_jobs)
    }) {
        publish_jobs(data, j);
    }
    if let Some(w) = wts {
        publish_worktrees(data, w);
    }
    if has_repo {
        gather_loc(root, data, st);
    }
    if let Some(h) = guarded("activity", crate::collect::activity::read_history) {
        publish_activity(data, h);
    }
    gather_docker_and_endpoints(root, data);
}

fn gather_loc(root: &str, data: &Arc<Mutex<DashboardData>>, st: &mut SlowState) {
    let key = LocCache::key(root);
    // Unchanged: skip both the tokei pass and the publish (no `rev` bump).
    if key.is_some() && key == st.loc.key {
        return;
    }
    if let Some(rows) = guarded("loc", || loc::loc_rows(root)) {
        st.loc.key = key;
        publish_loc(data, rows);
    }
}

/// One `docker ps` (+ stats) feeds both the Docker and Ports widgets.
fn gather_docker_and_endpoints(root: &str, data: &Arc<Mutex<DashboardData>>) {
    let all = guarded("docker", || {
        docker::gather_containers(&docker::repo_scope_roots(root))
    });
    if let Some(all) = &all {
        *lock(&ALL_CONTAINERS) = all.clone();
        publish_containers(data, docker::scoped(all, docker_scope()));
    }
    let all = all.unwrap_or_else(all_containers);
    let cfg = crate::config::load_for(root);
    if let Some(eps) = guarded("endpoints", || {
        crate::collect::ports::gather_endpoints(&cfg, root, &all)
    }) {
        publish_endpoints(data, eps);
    }
}

/// Manual-refresh handshake between the UI and the slow thread.
#[derive(Default)]
struct Wake {
    /// Bumped by every manual request.
    requested: u64,
    /// Highest request number a finished slow tick has covered.
    completed: u64,
    /// Number of live slow threads (0 = run manual refreshes inline).
    slow_threads: u32,
}

static WAKE: Mutex<Wake> = Mutex::new(Wake {
    requested: 0,
    completed: 0,
    slow_threads: 0,
});
static WAKE_CV: Condvar = Condvar::new();

/// Ask the slow thread for an immediate tick including `git fetch`, without
/// waiting. No-op if no slow thread is running.
pub fn request_now() -> u64 {
    let mut w = lock(&WAKE);
    w.requested += 1;
    WAKE_CV.notify_all();
    w.requested
}

/// One full gather + publish, including a `git fetch` (used on the manual `r`
/// refresh, where the user expects freshly fetched remotes). Blocks until it
/// is done, so callers can show a spinner meanwhile.
///
/// When the background threads are running this wakes the slow thread and
/// waits for its tick instead of gathering in parallel with it.
pub fn refresh_now(root: &str, data: &Arc<Mutex<DashboardData>>) {
    if lock(&WAKE).slow_threads == 0 {
        fetch_origin_bounded(root);
        gather_all(root, data, &mut SlowState::default());
        return;
    }
    let ticket = request_now();
    let deadline = Instant::now() + MANUAL_WAIT;
    let mut w = lock(&WAKE);
    while w.completed < ticket && w.slow_threads > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        w = WAKE_CV
            .wait_timeout(w, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// Sleep until the next slow tick is due, a manual request arrives, or `stop`.
/// Returns the pending manual request number, if one woke us.
fn wait_for_tick(stop: &AtomicBool, served: u64) -> Option<u64> {
    let due = Instant::now() + SLOW_EVERY;
    let mut w = lock(&WAKE);
    loop {
        if w.requested > served {
            return Some(w.requested);
        }
        let now = Instant::now();
        if stop.load(Ordering::Relaxed) || now >= due {
            return None;
        }
        // Short waits so the stop flag (an atomic, not condvar-signalled) is seen promptly.
        let nap = (due - now).min(Duration::from_millis(250));
        w = WAKE_CV
            .wait_timeout(w, nap)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

fn slow_loop(root: &str, data: &Arc<Mutex<DashboardData>>, stop: &AtomicBool) {
    let mut st = SlowState::default();
    let mut last_fetch: Option<Instant> = None;
    let mut served = lock(&WAKE).requested;
    let mut manual = false;
    while !stop.load(Ordering::Relaxed) {
        let target = lock(&WAKE).requested;
        if manual || last_fetch.is_none_or(|t| t.elapsed() >= FETCH_EVERY) {
            guarded("fetch", || fetch_origin_bounded(root));
            last_fetch = Some(Instant::now());
        }
        gather_all(root, data, &mut st);
        {
            let mut w = lock(&WAKE);
            w.completed = w.completed.max(target);
            WAKE_CV.notify_all();
        }
        served = served.max(target);
        manual = wait_for_tick(stop, served).is_some();
    }
}

/// Sleep `ticks` × 100 ms, returning early once `stop` is set.
fn nap(stop: &AtomicBool, ticks: u32) {
    for _ in 0..ticks {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = thread::Builder::new()
        .name(format!("{THREAD_PREFIX}{name}"))
        .spawn(f);
}

/// Spawn the slow (10 s) refresh loop, a fast (2 s) jobs loop and the cost
/// loop. All threads share the returned stop flag.
pub fn spawn(root: String, data: Arc<Mutex<DashboardData>>) -> Arc<AtomicBool> {
    install_panic_capture();
    let stop = Arc::new(AtomicBool::new(false));

    // Slow thread: worktrees/loc/jobs/activity/docker/endpoints/repo every
    // 10 s, `git fetch` at most every 60 s, plus manual wake-ups.
    let (s, root2, data2) = (stop.clone(), root.clone(), data.clone());
    lock(&WAKE).slow_threads += 1;
    spawn_named("slow", move || {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| slow_loop(&root2, &data2, &s)));
        let mut w = lock(&WAKE);
        w.slow_threads = w.slow_threads.saturating_sub(1);
        WAKE_CV.notify_all();
    });

    // Fast thread: jobs every 2 s — the one widget that genuinely benefits
    // from a tight cadence (live agent state).
    let (s, data3) = (stop.clone(), data.clone());
    spawn_named("fast", move || {
        while !s.load(Ordering::Relaxed) {
            // Merge against the last-published worktrees (refreshed by the slow
            // thread) so orphan-worktree rows stay present between slow ticks.
            let wts = { lock(&data3).worktrees.clone() };
            if let Some(j) = guarded("jobs", || {
                jobs::merge_orphan_worktrees(jobs::gather_jobs(), &wts)
            }) {
                publish_jobs(&data3, j);
            }
            nap(&s, 20);
        }
    });

    // Cost thread: usage + tool-use + skill scans every 10 s. All three walk
    // the transcripts, so they share this thread's cadence.
    let (s, root_cost) = (stop.clone(), root);
    spawn_named("cost", move || {
        // Best-effort live price refresh at startup; ignore result.
        let _ = guarded("pricing", crate::collect::pricing::fetch_litellm);
        let mut cache = crate::collect::usage::UsageCache::new();
        let mut tool_cache = crate::collect::tools::ToolCache::new();
        let mut skill_cache = crate::collect::skills::SkillCache::new();
        while !s.load(Ordering::Relaxed) {
            if let Some(u) = guarded("usage", || crate::collect::usage::scan_all(&mut cache)) {
                publish_usage(&data, u);
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            if let Some(t) = guarded("tools", || {
                crate::collect::tools::scan_tools(&mut tool_cache, now_ms, 7)
            }) {
                publish_tools(&data, t);
            }
            if let Some(sk) = guarded("skills", || {
                crate::collect::skills::scan_skills(&mut skill_cache, &root_cost)
            }) {
                publish_skills(&data, sk);
            }
            nap(&s, 100);
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

        let d = lock(&data);
        assert_eq!(d.worktrees.len(), 1);
        assert_eq!(d.loc.len(), 1); // untouched
    }

    #[test]
    fn guarded_catches_panics_and_records_health() {
        assert_eq!(guarded("test-ok", || 7), Some(7));
        let r: Option<u8> = guarded("test-boom", || panic!("collector exploded"));
        assert_eq!(r, None);
        let h = health();
        let e = h.iter().find(|e| e.slice == "test-boom").expect("recorded");
        assert!(e.message.contains("collector exploded"), "{}", e.message);
        // Success clears the slice.
        assert_eq!(guarded("test-boom", || 1), Some(1));
        assert!(!health().iter().any(|e| e.slice == "test-boom"));
    }

    #[test]
    fn poisoned_lock_is_recovered() {
        let data = Arc::new(Mutex::new(DashboardData::default()));
        let d2 = data.clone();
        let _ = thread::spawn(move || {
            let _g = d2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(data.is_poisoned());
        publish_loc(&data, Vec::new()); // must not panic
        assert!(lock(&data).rev > 0);
    }

    #[test]
    fn manual_request_wakes_the_slow_wait() {
        let stop = AtomicBool::new(false);
        let served = lock(&WAKE).requested;
        let t = thread::spawn(|| {
            thread::sleep(Duration::from_millis(50));
            request_now()
        });
        let start = Instant::now();
        let got = wait_for_tick(&stop, served);
        let ticket = t.join().unwrap();
        assert!(got.is_some_and(|g| g <= ticket));
        assert!(start.elapsed() < SLOW_EVERY);
    }
}
