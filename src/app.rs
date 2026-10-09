//! Application state and view stack.

use ratatui::layout::Rect;
use ratatui::widgets::TableState;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::anim::{Clock, Flashes};
use crate::collect::git::Worktree;
use crate::collect::git_detail::WorktreeDetail;
use crate::collect::jobs::JobEvent;
use crate::collect::loc::LocRow;
use crate::fuzzy::Fuzzy;
use crate::layout::{band, widgets_for, FrameRects};
use crate::theme::Theme;
use crate::view;
use crate::widget::WidgetKind;

/// All cached datasets shown on the dashboard. Extended per phase.
#[derive(Default)]
pub struct DashboardData {
    /// Repo name and branch shown in the header bar.
    pub repo: crate::collect::git::RepoInfo,
    pub worktrees: Vec<Worktree>,
    pub loc: Vec<LocRow>,
    pub jobs: Vec<crate::collect::jobs::Job>,
    pub usage: Option<crate::collect::usage::UsageTotals>,
    pub activity: Vec<(String, u32)>,
    pub containers: Vec<crate::collect::docker::Container>,
    pub endpoints: Vec<crate::collect::ports::Endpoint>,
    pub tools: Vec<crate::collect::tools::ToolStat>,
    pub skills: Vec<crate::collect::skills::Skill>,
    /// One row per registered project and suggestion, for the project picker.
    pub project_rows: Vec<crate::projects::overview::ProjectRow>,
    /// Monotonic revision, bumped by every `publish_*`. The render loop redraws
    /// only when this changes (or on input), so an idle dashboard costs no CPU.
    pub rev: u64,
}

/// A scrollable in-app diff/log view. Scroll position lives in `App::detail_scroll`.
#[derive(Clone, Default)]
pub struct DiffView {
    pub title: String,
    pub lines: Vec<String>,
    /// True until the background `git diff` has answered.
    pub loading: bool,
}

/// A drill-in detail target.
///
/// Row-backed details carry the row's stable key (see `rows::TableRow::key`),
/// never a data index: a refresh may reorder or drop rows at any time, and an
/// index would then silently point at a different item. The key is resolved
/// against the live data on every render, which is also how "no longer
/// present" gets noticed.
pub enum Detail {
    /// Keys off `App::wt_path`/`wt_detail`; the file diff needs the cached detail.
    Worktree,
    /// Job id.
    Job(String),
    /// Container id.
    Container(String),
    Diff(DiffView),
    Cost,
    /// Model name.
    CostModel(String),
    Activity,
    Code,
    /// Endpoint key (`host:port`).
    Ports(String),
    /// Skill key (`source/name`). The Repo slot hosts the Skills widget.
    Repo(String),
}

/// The current screen.
pub enum View {
    Dashboard,
    Expanded(WidgetKind),
    Detail(Detail),
}

/// What keystrokes currently mean.
///
/// `Normal` is the navigation vocabulary; the other two capture text. Keeping
/// this explicit is what lets `/` and `:` type freely without every letter also
/// firing its navigation binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Incremental filter over the focused widget's rows.
    Filter,
    /// Command palette.
    Palette,
}

/// Per-widget UI state: selection, live filter, sort.
#[derive(Default)]
pub struct WidgetUiState {
    pub table: TableState,
    /// Fuzzy filter applied to this widget's rows. Empty means no filter.
    pub filter: String,
    /// Index into `view::sort_columns(kind)`; 0 keeps the collector's order.
    pub sort_col: usize,
    pub sort_desc: bool,
}

impl WidgetUiState {
    /// The sort to apply, or `None` while on the collector's own order.
    pub fn sort(&self) -> Option<(usize, bool)> {
        (self.sort_col != 0).then_some((self.sort_col, self.sort_desc))
    }
}

/// The result of a drill-in that ran off the UI thread.
///
/// Each carries the token of the request that produced it; anything older than
/// `App::load_token` is a superseded request and is dropped.
pub enum Loaded {
    Worktree { token: u64, detail: WorktreeDetail },
    Logs { token: u64, lines: Vec<String> },
    Diff { token: u64, view: DiffView },
}

/// Parsed job timeline, valid while the file's mtime and length are unchanged.
pub struct TimelineCache {
    pub job_id: String,
    pub stamp: (Option<std::time::SystemTime>, u64),
    pub events: Vec<JobEvent>,
}

pub struct App {
    pub data: Arc<Mutex<DashboardData>>,
    pub view: View,
    pub focus: WidgetKind,
    pub ui: HashMap<WidgetKind, WidgetUiState>,
    pub rects: FrameRects,
    /// Size of the last drawn frame — decides which widgets the dashboard shows.
    pub area: Rect,
    pub theme: Theme,
    pub should_quit: bool,
    pub show_help: bool,
    pub detail_scroll: u16,
    /// Largest useful `detail_scroll` for the current detail, written by the
    /// renderer. Input clamps against it so overscroll can never pile up.
    pub detail_max: u16,
    pub detail_table: TableState,
    /// The Expanded widget a detail was opened from, so Back returns there.
    pub origin: Option<WidgetKind>,
    /// Path of the worktree whose detail is open.
    pub wt_path: Option<String>,
    pub wt_detail: Option<WorktreeDetail>,
    /// When the open worktree detail was last (re)requested.
    pub wt_requested: Option<Instant>,
    pub container_logs: Vec<String>,
    pub timeline: Option<TimelineCache>,

    /// Drives spinners, pulses, and flash fades.
    pub clock: Clock,
    /// Rows that changed recently, by `WidgetKind`-scoped row key.
    pub flashes: Flashes,
    /// True when the last frame drew a row that is still fading. Only visible
    /// motion is worth a fast tick.
    pub anim_visible: bool,
    /// Reused fuzzy matcher — constructing one per keystroke would allocate.
    pub fuzzy: Fuzzy,
    pub mode: Mode,
    /// Text buffer shared by filter and palette entry.
    pub input: String,
    /// Selected row in the command palette.
    pub palette_sel: usize,
    /// Original row indices currently displayed, per widget. Recomputed when
    /// data, filter, or sort changes — every selection bound reads this, so
    /// filtered-away rows can never be selected or drilled into.
    pub visible: HashMap<WidgetKind, Vec<usize>>,
    /// Row keys parallel to `visible`, so a selection can follow its row
    /// across a refresh that reorders the data.
    visible_keys: HashMap<WidgetKind, Vec<String>>,
    /// Last-seen row signatures, per widget, for change detection.
    sigs: HashMap<WidgetKind, HashMap<String, String>>,
    /// Background drill-in results.
    load_tx: Sender<Loaded>,
    load_rx: Receiver<Loaded>,
    pub load_token: u64,
    /// A background drill-in is in flight.
    pub loading: bool,

    /// The project picker, while it is open. Modal: it takes every key.
    pub picker: Option<crate::projects::picker::Picker>,
    /// Registered projects, pinned first — feeds the palette's "Switch to …"
    /// entries and the `1`–`9` hotkeys without a disk read per key.
    pub project_list: Vec<crate::projects::host::ListedProject>,
    /// A project switch requested by a key or command, applied by the run loop
    /// (which owns the refresh threads).
    pub pending_switch: Option<std::path::PathBuf>,
}

impl App {
    pub fn new(theme: Theme) -> Self {
        let (load_tx, load_rx) = channel();
        App {
            data: Arc::new(Mutex::new(DashboardData::default())),
            view: View::Dashboard,
            focus: WidgetKind::Worktrees,
            ui: HashMap::new(),
            rects: FrameRects::default(),
            area: Rect::default(),
            theme,
            should_quit: false,
            show_help: false,
            detail_scroll: 0,
            detail_max: 0,
            detail_table: TableState::default(),
            origin: None,
            wt_path: None,
            wt_detail: None,
            wt_requested: None,
            container_logs: Vec::new(),
            timeline: None,
            clock: Clock::default(),
            flashes: Flashes::default(),
            anim_visible: false,
            fuzzy: Fuzzy::default(),
            mode: Mode::Normal,
            input: String::new(),
            palette_sel: 0,
            visible: HashMap::new(),
            visible_keys: HashMap::new(),
            sigs: HashMap::new(),
            load_tx,
            load_rx,
            load_token: 0,
            loading: false,
            picker: None,
            project_list: Vec::new(),
            pending_switch: None,
        }
    }

    /// Forget everything tied to the previous project: open details, cached
    /// drill-ins, selections, filters, and change-flash history (otherwise
    /// every row of the new project would flash as "changed").
    pub fn reset_for_project(&mut self) {
        self.view = View::Dashboard;
        self.origin = None;
        self.mode = Mode::Normal;
        self.input.clear();
        self.detail_scroll = 0;
        self.wt_path = None;
        self.wt_detail = None;
        self.wt_requested = None;
        self.container_logs.clear();
        self.timeline = None;
        self.ui.clear();
        self.visible.clear();
        self.visible_keys.clear();
        self.sigs.clear();
        self.flashes = Flashes::default();
        self.load_token = self.load_token.wrapping_add(1);
        self.loading = false;
    }

    /// Widgets the dashboard currently places. Empty before the first frame,
    /// which callers treat as "everything is reachable".
    pub fn placed(&self) -> Vec<WidgetKind> {
        widgets_for(band(self.area))
    }

    /// True when `k` has a cell on the dashboard at the current size.
    pub fn is_placed(&self, k: WidgetKind) -> bool {
        let p = self.placed();
        p.is_empty() || p.contains(&k)
    }

    /// How many widgets the current size band leaves off the dashboard.
    pub fn hidden_count(&self) -> usize {
        let p = self.placed();
        if p.is_empty() {
            0
        } else {
            WidgetKind::all().len() - p.len()
        }
    }

    /// Cycle focus. On the dashboard, widgets the size band hides are skipped
    /// (focusing something invisible strands the keyboard); in the Expanded
    /// view every widget is reachable and the view follows the focus.
    fn step_focus(&mut self, forward: bool) {
        let step = |k: WidgetKind| if forward { k.next() } else { k.prev() };
        match self.view {
            View::Expanded(_) => {
                self.focus = step(self.focus);
                self.view = View::Expanded(self.focus);
            }
            View::Dashboard => {
                let mut k = self.focus;
                for _ in 0..WidgetKind::all().len() {
                    k = step(k);
                    if self.is_placed(k) {
                        break;
                    }
                }
                self.focus = k;
            }
            // Focus is a dashboard concept; a detail screen has nothing to cycle.
            View::Detail(_) => {}
        }
    }

    pub fn focus_next(&mut self) {
        self.step_focus(true);
    }
    pub fn focus_prev(&mut self) {
        self.step_focus(false);
    }
    pub fn expand_focused(&mut self) {
        self.view = View::Expanded(self.focus);
    }

    /// Focus `k`, expanding it when the dashboard has no room to show it.
    pub fn focus_widget(&mut self, k: WidgetKind) {
        self.focus = k;
        self.origin = None;
        self.view = if self.is_placed(k) {
            View::Dashboard
        } else {
            View::Expanded(k)
        };
    }

    /// True on the screens where the focused widget's rows are on show — the
    /// only places filter and sort mean anything.
    pub fn table_view(&self) -> bool {
        matches!(self.view, View::Dashboard | View::Expanded(_))
    }

    /// Pop one level: Diff -> Worktree detail -> wherever the detail was opened
    /// from (Expanded or Dashboard); CostModel -> Cost; Expanded -> Dashboard.
    pub fn back(&mut self) {
        let cur = std::mem::replace(&mut self.view, View::Dashboard);
        self.view = match cur {
            View::Detail(Detail::Diff(_)) if self.wt_path.is_some() => {
                View::Detail(Detail::Worktree)
            }
            View::Detail(Detail::CostModel(_)) => View::Detail(Detail::Cost),
            View::Detail(_) => match self.origin.take() {
                Some(k) => View::Expanded(k),
                None => View::Dashboard,
            },
            _ => View::Dashboard,
        };
        if !matches!(self.view, View::Detail(_)) {
            self.wt_path = None;
            self.wt_detail = None;
            self.loading = false;
        }
    }

    /// Open a detail screen, remembering an Expanded origin for Back.
    pub fn open_detail(&mut self, d: Detail) {
        if let View::Expanded(k) = self.view {
            self.origin = Some(k);
        } else if !matches!(self.view, View::Detail(_)) {
            self.origin = None;
        }
        self.view = View::Detail(d);
        self.detail_scroll = 0;
    }

    /// Clamp a scroll request against the last rendered maximum.
    pub fn set_detail_scroll(&mut self, v: u16) {
        self.detail_scroll = v.min(self.detail_max);
    }

    pub fn ui_mut(&mut self, k: WidgetKind) -> &mut WidgetUiState {
        self.ui.entry(k).or_default()
    }
    pub fn focus_ui_mut(&mut self) -> &mut WidgetUiState {
        let k = self.focus;
        self.ui.entry(k).or_default()
    }
    pub fn ui_offset(&self, k: WidgetKind) -> usize {
        self.ui.get(&k).map(|u| u.table.offset()).unwrap_or(0)
    }
    /// The focused widget's filter, or `""`.
    pub fn filter_of(&self, k: WidgetKind) -> &str {
        self.ui.get(&k).map(|u| u.filter.as_str()).unwrap_or("")
    }

    /// How many rows `k` is currently showing.
    pub fn visible_count(&self, k: WidgetKind) -> usize {
        self.visible.get(&k).map(Vec::len).unwrap_or(0)
    }

    /// Map a displayed row position to its index in the underlying data.
    ///
    /// With a filter or sort active, row 0 on screen is rarely row 0 in the
    /// collector's vector.
    pub fn data_index(&self, k: WidgetKind, visible_pos: usize) -> Option<usize> {
        self.visible.get(&k)?.get(visible_pos).copied()
    }

    /// The data index under the selection for widget `k`.
    pub fn selected_data_index(&self, k: WidgetKind) -> Option<usize> {
        let sel = self.ui.get(&k)?.table.selected()?;
        self.data_index(k, sel)
    }

    /// The stable row key under the selection for widget `k`. Drill-ins carry
    /// this, never an index.
    pub fn selected_key(&self, k: WidgetKind) -> Option<String> {
        let sel = self.ui.get(&k)?.table.selected()?;
        self.visible_keys.get(&k)?.get(sel).cloned()
    }

    /// Flash key for a row, namespaced so two widgets cannot collide.
    pub fn flash_key(k: WidgetKind, row_key: &str) -> String {
        format!("{k:?}/{row_key}")
    }

    /// Recompute the visible index lists for every table widget.
    ///
    /// The selection follows its row by key, so a refresh that reorders or
    /// drops rows never silently moves the cursor onto a different item. Only
    /// when the selected row is gone does it fall back to clamping the old
    /// position into range.
    pub fn refilter(&mut self) {
        let data = self.data.lock().unwrap();
        for &k in WidgetKind::all() {
            if !view::is_table(k) {
                continue;
            }
            let st = self.ui.entry(k).or_default();
            let (filter, sort) = (st.filter.clone(), st.sort());
            let prev_sel = st.table.selected();
            let prev_key = prev_sel.and_then(|s| self.visible_keys.get(&k)?.get(s).cloned());
            let idx = view::visible(k, &data, &filter, sort, &mut self.fuzzy);
            let keys = view::keys(k, &data, &idx);
            let n = idx.len();
            let found = prev_key.and_then(|key| keys.iter().position(|x| *x == key));
            let next = match (found, prev_sel) {
                (Some(p), _) => Some(p),
                _ if n == 0 => None,
                (None, Some(s)) => Some(s.min(n - 1)),
                (None, None) => Some(0),
            };
            let st = self.ui.entry(k).or_default();
            if st.table.selected() != next {
                st.table.select(next);
            }
            self.visible.insert(k, idx);
            self.visible_keys.insert(k, keys);
        }
    }

    /// Absorb a new data revision: flag the rows that changed, then refilter.
    ///
    /// The first snapshot for a widget seeds signatures without flashing —
    /// otherwise every row would light up on launch, which says nothing.
    pub fn sync(&mut self) {
        let fresh: Vec<(WidgetKind, HashMap<String, String>)> = {
            let data = self.data.lock().unwrap();
            WidgetKind::all()
                .iter()
                .filter(|&&k| view::is_table(k))
                .map(|&k| (k, view::signatures(k, &data)))
                .collect()
        };
        for (k, next) in fresh {
            if let Some(prev) = self.sigs.get(&k) {
                if !prev.is_empty() {
                    for (key, sig) in &next {
                        if prev.get(key) != Some(sig) {
                            self.flashes.mark(&Self::flash_key(k, key));
                        }
                    }
                }
            }
            self.sigs.insert(k, next);
        }
        self.flashes.gc();
        self.refilter();
    }

    /// Run `job` on a worker thread; its result arrives via [`App::drain_loads`].
    ///
    /// Drill-ins shell out to `git` and `docker`, which can take seconds on a
    /// big repo or a busy daemon — long enough to freeze the UI if run inline.
    pub fn spawn_load(&mut self, job: impl FnOnce(u64) -> Loaded + Send + 'static) {
        self.load_token += 1;
        self.loading = true;
        let token = self.load_token;
        let tx = self.load_tx.clone();
        std::thread::spawn(move || {
            // The receiver only disappears when the app is shutting down.
            let _ = tx.send(job(token));
        });
    }

    /// Apply finished background loads. Returns true when anything changed.
    pub fn drain_loads(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.load_rx.try_recv() {
            changed |= self.apply_load(msg);
        }
        changed
    }

    fn apply_load(&mut self, msg: Loaded) -> bool {
        let token = match &msg {
            Loaded::Worktree { token, .. } | Loaded::Logs { token, .. } => *token,
            Loaded::Diff { token, .. } => *token,
        };
        if token != self.load_token {
            return false;
        }
        self.loading = false;
        match msg {
            Loaded::Worktree { detail, .. } => {
                if self.wt_path.as_deref() == Some(detail.path.as_str()) {
                    self.wt_detail = Some(detail);
                }
            }
            Loaded::Logs { lines, .. } => {
                if matches!(self.view, View::Detail(Detail::Container(_))) {
                    self.container_logs = lines;
                    // Logs read newest-last: open at the bottom, where the
                    // renderer clamps this to the real maximum.
                    self.detail_scroll = u16::MAX;
                }
            }
            Loaded::Diff { view, .. } => {
                if let View::Detail(Detail::Diff(dv)) = &mut self.view {
                    *dv = view;
                }
            }
        }
        true
    }

    /// Block until the in-flight load lands. Tests only — the app never waits.
    #[cfg(test)]
    pub fn wait_loads(&mut self) {
        while self.loading {
            if let Ok(msg) = self
                .load_rx
                .recv_timeout(std::time::Duration::from_secs(10))
            {
                self.apply_load(msg);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::jobs::Job;

    fn job(id: &str, name: &str, state: &str) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            state: state.into(),
            tempo: "active".into(),
            intent: String::new(),
            tasks: 0,
            queued: 0,
            cwd: String::new(),
            worktree_path: None,
            worktree_branch: None,
            created_at: Some(0),
            updated_at: Some(0),
            ..Default::default()
        }
    }

    fn app_with_jobs() -> App {
        let app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.jobs = vec![
                job("1", "alpha", "working"),
                job("2", "beta", "done"),
                job("3", "gamma", "done"),
            ];
        }
        app
    }

    #[test]
    fn expand_and_back() {
        let mut app = App::new(Theme::default());
        assert!(matches!(app.view, View::Dashboard));
        app.expand_focused();
        assert!(matches!(app.view, View::Expanded(_)));
        app.back();
        assert!(matches!(app.view, View::Dashboard));
    }

    #[test]
    fn focus_next_wraps() {
        let mut app = App::new(Theme::default());
        let start = app.focus;
        for _ in 0..WidgetKind::all().len() {
            app.focus_next();
        }
        assert_eq!(app.focus, start);
    }

    #[test]
    fn filter_narrows_visible_rows_and_maps_back_to_data() {
        let mut app = app_with_jobs();
        app.sync();
        assert_eq!(app.visible_count(WidgetKind::Jobs), 3);

        app.ui_mut(WidgetKind::Jobs).filter = "gamma".into();
        app.refilter();
        assert_eq!(app.visible_count(WidgetKind::Jobs), 1);
        // Displayed row 0 is data row 2 — drill-in must follow the mapping.
        assert_eq!(app.data_index(WidgetKind::Jobs, 0), Some(2));
    }

    #[test]
    fn selection_is_clamped_when_the_filter_narrows() {
        let mut app = app_with_jobs();
        app.sync();
        app.ui_mut(WidgetKind::Jobs).table.select(Some(2));

        app.ui_mut(WidgetKind::Jobs).filter = "alpha".into();
        app.refilter();
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(0));

        // A filter matching nothing clears the selection rather than pointing
        // at a row that is not on screen.
        app.ui_mut(WidgetKind::Jobs).filter = "zzzzz".into();
        app.refilter();
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), None);
        assert_eq!(app.selected_data_index(WidgetKind::Jobs), None);
    }

    #[test]
    fn sort_reorders_visible_rows() {
        let mut app = app_with_jobs();
        app.sync();
        let st = app.ui_mut(WidgetKind::Jobs);
        st.sort_col = 1; // name
        st.sort_desc = true;
        app.refilter();
        // gamma, beta, alpha
        assert_eq!(app.data_index(WidgetKind::Jobs, 0), Some(2));
        assert_eq!(app.data_index(WidgetKind::Jobs, 2), Some(0));
    }

    #[test]
    fn first_sync_seeds_without_flashing() {
        let mut app = app_with_jobs();
        app.sync();
        assert_eq!(
            app.flashes
                .intensity(&App::flash_key(WidgetKind::Jobs, "1")),
            0.0
        );

        // A later change does flash.
        app.data.lock().unwrap().jobs[0].state = "blocked".into();
        app.sync();
        assert!(
            app.flashes
                .intensity(&App::flash_key(WidgetKind::Jobs, "1"))
                > 0.0
        );
        // Untouched rows stay quiet.
        assert_eq!(
            app.flashes
                .intensity(&App::flash_key(WidgetKind::Jobs, "2")),
            0.0
        );
    }

    #[test]
    fn flash_keys_are_namespaced_per_widget() {
        assert_ne!(
            App::flash_key(WidgetKind::Jobs, "1"),
            App::flash_key(WidgetKind::Docker, "1")
        );
    }
}
