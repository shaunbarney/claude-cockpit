//! Glue between the project picker and the running cockpit: opening it,
//! routing its keys, switching the active project, and keeping the overview
//! rows fresh in the background.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::overview::{self, JobRef};
use super::paths::{canonical, registry_file, state_file};
use super::picker::{Files, Outcome, Picker};
use super::store::{load_registry, load_state, save_state};
use crate::active::ActiveProject;
use crate::app::{App, DashboardData};

/// How often the overview rows are regathered while nobody asks for them.
const OVERVIEW_EVERY: Duration = Duration::from_secs(30);

/// Set to make the overview thread regather now (picker opened, registry edited).
static OVERVIEW_WAKE: AtomicBool = AtomicBool::new(true);

pub fn wake_overview() {
    OVERVIEW_WAKE.store(true, Ordering::Release);
}

/// A registered project as the dashboard needs it, without a disk read per key.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedProject {
    pub name: String,
    pub root: PathBuf,
    pub pinned: bool,
}

fn listed(reg: &super::Registry) -> Vec<ListedProject> {
    reg.ordered()
        .into_iter()
        .map(|p| ListedProject {
            name: p.name.clone(),
            root: p.path.clone(),
            pinned: p.pinned,
        })
        .collect()
}

/// Every registered project, pinned first.
pub fn project_list() -> Vec<ListedProject> {
    load_registry().map(|r| listed(&r)).unwrap_or_default()
}

/// Root of the `n`th pinned project (`0` is hotkey `1`).
pub fn pinned_root(list: &[ListedProject], n: usize) -> Option<PathBuf> {
    list.iter()
        .filter(|p| p.pinned)
        .nth(n)
        .map(|p| p.root.clone())
}

/// The registered alias for `root`, if any.
pub fn alias_for(root: &Path) -> Option<String> {
    let root = canonical(root);
    project_list()
        .into_iter()
        .find(|p| canonical(&p.root) == root)
        .map(|p| p.name)
}

/// Open the picker over the dashboard.
pub fn open(app: &mut App, current: &str) {
    let (Some(registry_path), Some(state_path)) = (registry_file(), state_file()) else {
        return;
    };
    let files = Files {
        registry: registry_path,
        state: state_path,
    };
    let current = (!current.is_empty()).then(|| PathBuf::from(current));
    let mut picker = match load_registry() {
        Ok(reg) => Picker::new(reg, load_state(), files, current),
        Err(e) => {
            let mut p = Picker::new(Default::default(), load_state(), files, current);
            p.message = Some((
                format!("{e} — fix the file; edits are disabled to protect it"),
                true,
            ));
            p.read_only = true;
            p
        }
    };
    let rows = app
        .data
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .project_rows
        .clone();
    if !rows.is_empty() {
        picker.set_rows(rows, &mut app.fuzzy);
    }
    app.picker = Some(picker);
    wake_overview();
}

/// Feed one key to the open picker.
pub fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    let Some(picker) = app.picker.as_mut() else {
        return;
    };
    match picker.handle_key(code, mods, &mut app.fuzzy) {
        Outcome::None => {}
        Outcome::Close => app.picker = None,
        Outcome::Switch(root) => {
            app.picker = None;
            app.pending_switch = Some(root);
        }
        Outcome::Changed => {
            app.project_list = listed(&picker.registry);
            wake_overview();
        }
    }
}

/// Fresh overview rows arrived: hand them to the picker if it is open.
pub fn sync(app: &mut App) {
    let Some(picker) = app.picker.as_mut() else {
        return;
    };
    let rows = app
        .data
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .project_rows
        .clone();
    if !rows.is_empty() && rows != picker.rows {
        picker.set_rows(rows, &mut app.fuzzy);
    }
}

/// Make `root` the project on screen. Everything scoped to the old project is
/// dropped at once, so nothing from it lingers while the new one loads.
pub fn switch(app: &mut App, active: &ActiveProject, root: &Path) {
    let root_s = root.to_string_lossy().into_owned();
    if canonical(root) == canonical(Path::new(&active.root())) {
        return;
    }
    active.set(&root_s, &alias_for(root).unwrap_or_default());
    app.reset_for_project();
    {
        let mut d = app.data.lock().unwrap_or_else(|e| e.into_inner());
        d.repo = crate::collect::git::RepoInfo {
            name: active.name(),
            branch: String::new(),
        };
        d.worktrees.clear();
        d.loc.clear();
        d.jobs.clear();
        d.containers.clear();
        d.endpoints.clear();
        d.skills.clear();
        d.rev += 1;
    }
    remember(root);
}

/// Record `root` as the last-opened project.
pub fn remember(root: &Path) {
    let mut st = load_state();
    st.touch(root, chrono::Utc::now().timestamp());
    let _ = save_state(&st);
}

/// Every job as the overview sees it.
fn job_refs() -> Vec<JobRef> {
    crate::collect::jobs::gather_jobs()
        .into_iter()
        .map(|j| {
            let paths = [
                j.worktree_path.as_deref(),
                Some(j.cwd.as_str()),
                j.origin_cwd.as_deref(),
            ]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty() && Some(Path::new(p)) != dirs::home_dir().as_deref())
            .map(PathBuf::from)
            .collect();
            JobRef {
                paths,
                active: j.state == "working",
                blocked: j.state == "blocked",
            }
        })
        .collect()
}

/// Regather overview rows every `OVERVIEW_EVERY`, or at once when woken.
pub fn spawn_overview(data: Arc<Mutex<DashboardData>>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut waited = Duration::ZERO;
        while !stop.load(Ordering::Relaxed) {
            if OVERVIEW_WAKE.swap(false, Ordering::AcqRel) || waited >= OVERVIEW_EVERY {
                waited = Duration::ZERO;
                let reg = load_registry().unwrap_or_default();
                let rows = overview::gather(&reg, &load_state(), &job_refs());
                let mut d = data.lock().unwrap_or_else(|e| e.into_inner());
                d.project_rows = rows;
                d.rev += 1;
            }
            thread::sleep(Duration::from_millis(100));
            waited += Duration::from_millis(100);
        }
    });
}
