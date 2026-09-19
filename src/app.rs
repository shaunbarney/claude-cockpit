//! Application state and view stack.

use ratatui::widgets::TableState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::anim::{Clock, Flashes};
use crate::collect::git::Worktree;
use crate::collect::loc::LocRow;
use crate::fuzzy::Fuzzy;
use crate::layout::FrameRects;
use crate::theme::Theme;
use crate::view;
use crate::widget::WidgetKind;

/// All cached datasets shown on the dashboard. Extended per phase.
#[derive(Default)]
pub struct DashboardData {
    pub worktrees: Vec<Worktree>,
    pub loc: Vec<LocRow>,
    pub jobs: Vec<crate::collect::jobs::Job>,
    pub usage: Option<crate::collect::usage::UsageTotals>,
    pub activity: Vec<(String, u32)>,
    pub containers: Vec<crate::collect::docker::Container>,
    pub endpoints: Vec<crate::collect::ports::Endpoint>,
    pub tools: Vec<crate::collect::tools::ToolStat>,
    pub skills: Vec<crate::collect::skills::Skill>,
    /// Monotonic revision, bumped by every `publish_*`. The render loop redraws
    /// only when this changes (or on input), so an idle dashboard costs no CPU.
    pub rev: u64,
}

/// A scrollable in-app diff/log view. Scroll position lives in `App::detail_scroll`.
#[derive(Clone, Default)]
pub struct DiffView {
    pub title: String,
    pub lines: Vec<String>,
}

/// A drill-in detail target. The worktree detail keys off `App::last_wt_idx`/`wt_detail`,
/// so it carries no index; job/container detail render directly by index.
pub enum Detail {
    Worktree,
    Job(usize),
    Container(usize),
    Diff(DiffView),
    Cost,
    CostModel(usize),
    Activity,
    Code,
    Ports(usize),
    Repo(usize), // the Repo slot now hosts the Skills widget; carries the skill index
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

pub struct App {
    pub data: Arc<Mutex<DashboardData>>,
    pub view: View,
    pub focus: WidgetKind,
    pub ui: HashMap<WidgetKind, WidgetUiState>,
    pub rects: FrameRects,
    pub theme: Theme,
    pub should_quit: bool,
    pub show_help: bool,
    pub detail_scroll: u16,
    pub detail_table: TableState,
    pub last_wt_idx: Option<usize>,
    pub wt_detail: Option<crate::collect::git_detail::WorktreeDetail>,
    pub container_logs: Vec<String>,

    /// Drives spinners, pulses, and flash fades.
    pub clock: Clock,
    /// Rows that changed recently, by `WidgetKind`-scoped row key.
    pub flashes: Flashes,
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
    /// Last-seen row signatures, per widget, for change detection.
    sigs: HashMap<WidgetKind, HashMap<String, String>>,
}

impl App {
    pub fn new(theme: Theme) -> Self {
        App {
            data: Arc::new(Mutex::new(DashboardData::default())),
            view: View::Dashboard,
            focus: WidgetKind::Worktrees,
            ui: HashMap::new(),
            rects: FrameRects::default(),
            theme,
            should_quit: false,
            show_help: false,
            detail_scroll: 0,
            detail_table: TableState::default(),
            last_wt_idx: None,
            wt_detail: None,
            container_logs: Vec::new(),
            clock: Clock::default(),
            flashes: Flashes::default(),
            fuzzy: Fuzzy::default(),
            mode: Mode::Normal,
            input: String::new(),
            palette_sel: 0,
            visible: HashMap::new(),
            sigs: HashMap::new(),
        }
    }

    pub fn focus_next(&mut self) {
        self.focus = self.focus.next();
    }
    pub fn focus_prev(&mut self) {
        self.focus = self.focus.prev();
    }
    pub fn expand_focused(&mut self) {
        self.view = View::Expanded(self.focus);
    }

    /// Pop one level: Diff -> Worktree detail -> Dashboard; everything else -> Dashboard.
    pub fn back(&mut self) {
        let cur = std::mem::replace(&mut self.view, View::Dashboard);
        self.view = match cur {
            View::Detail(Detail::Diff(_)) if self.last_wt_idx.is_some() => {
                View::Detail(Detail::Worktree)
            }
            View::Detail(Detail::CostModel(_)) => View::Detail(Detail::Cost),
            _ => View::Dashboard,
        };
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
    /// Everything that drills in goes through here: with a filter or sort
    /// active, row 0 on screen is rarely row 0 in the collector's vector.
    pub fn data_index(&self, k: WidgetKind, visible_pos: usize) -> Option<usize> {
        self.visible.get(&k)?.get(visible_pos).copied()
    }

    /// The data index under the selection for widget `k`.
    pub fn selected_data_index(&self, k: WidgetKind) -> Option<usize> {
        let sel = self.ui.get(&k)?.table.selected()?;
        self.data_index(k, sel)
    }

    /// Flash key for a row, namespaced so two widgets cannot collide.
    pub fn flash_key(k: WidgetKind, row_key: &str) -> String {
        format!("{k:?}/{row_key}")
    }

    /// Recompute the visible index lists for every table widget.
    ///
    /// Also clamps each selection into range, so narrowing a filter never
    /// leaves the cursor pointing past the end of the list.
    pub fn refilter(&mut self) {
        let data = self.data.lock().unwrap();
        for &k in WidgetKind::all() {
            if !view::is_table(k) {
                continue;
            }
            let st = self.ui.entry(k).or_default();
            let (filter, sort) = (st.filter.clone(), st.sort());
            let idx = view::visible(k, &data, &filter, sort, &mut self.fuzzy);
            let st = self.ui.entry(k).or_default();
            match (st.table.selected(), idx.len()) {
                (Some(_), 0) => st.table.select(None),
                (Some(s), n) if s >= n => st.table.select(Some(n - 1)),
                (None, n) if n > 0 => st.table.select(Some(0)),
                _ => {}
            }
            self.visible.insert(k, idx);
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
        assert_eq!(app.flashes.intensity(&App::flash_key(WidgetKind::Jobs, "1")), 0.0);

        // A later change does flash.
        app.data.lock().unwrap().jobs[0].state = "blocked".into();
        app.sync();
        assert!(app.flashes.intensity(&App::flash_key(WidgetKind::Jobs, "1")) > 0.0);
        // Untouched rows stay quiet.
        assert_eq!(app.flashes.intensity(&App::flash_key(WidgetKind::Jobs, "2")), 0.0);
    }

    #[test]
    fn flash_keys_are_namespaced_per_widget() {
        assert_ne!(
            App::flash_key(WidgetKind::Jobs, "1"),
            App::flash_key(WidgetKind::Docker, "1")
        );
    }
}
