//! Input handling: crossterm key/mouse -> Action -> applied to App.
//!
//! Keys are mapped in three layers, revealed as the user reaches for them:
//! arrows/Enter/Esc/q work with no learning at all; the vim motions (j/k, g/G,
//! `/`, `?`) sit on top for anyone who already types that way; and `:` opens
//! the palette, which lists everything without needing a binding at all.
//!
//! [`map_key`] stays a pure function of (key, modifiers, mode) so the whole
//! vocabulary is testable without a terminal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub(crate) static REFRESH_INFLIGHT: AtomicBool = AtomicBool::new(false);

use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Borders};

use crate::app::{App, Detail, DiffView, Mode, View};
use crate::collect::git_detail::{self, DiffMode};
use crate::palette::{self, Command};
use crate::refresh;
use crate::view;
use crate::widget::WidgetKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Refresh,
    FocusNext,
    FocusPrev,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    Up,
    Down,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Expand,
    Drill,
    Back,
    Help,
    BeginFilter,
    BeginPalette,
    SortCycle,
    SortReverse,
    /// A character typed into the filter or palette.
    Input(char),
    Backspace,
    /// Enter while capturing text: commit the filter, or run the command.
    Accept,
    None,
}

/// Pure key -> action map. `mode` decides whether letters navigate or type.
pub fn map_key(code: KeyCode, mods: KeyModifiers, mode: Mode) -> Action {
    if mods.contains(KeyModifiers::CONTROL) {
        return match code {
            KeyCode::Char('c') => Action::Quit,
            KeyCode::Char('k') | KeyCode::Char('p') => Action::BeginPalette,
            KeyCode::Char('d') => Action::PageDown,
            KeyCode::Char('u') => Action::PageUp,
            _ => Action::None,
        };
    }

    // While capturing text, printable characters belong to the buffer.
    if mode != Mode::Normal {
        return match code {
            KeyCode::Esc => Action::Back,
            KeyCode::Enter => Action::Accept,
            KeyCode::Backspace => Action::Backspace,
            KeyCode::Up => Action::Up,
            KeyCode::Down => Action::Down,
            KeyCode::Char(c) => Action::Input(c),
            _ => Action::None,
        };
    }

    match code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Esc => Action::Back,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Tab => Action::FocusNext,
        KeyCode::BackTab => Action::FocusPrev,
        KeyCode::Right | KeyCode::Char('l') => Action::FocusRight,
        KeyCode::Left | KeyCode::Char('h') => Action::FocusLeft,
        KeyCode::Up => Action::FocusUp,
        KeyCode::Down => Action::FocusDown,
        KeyCode::Char('k') => Action::Up,
        KeyCode::Char('j') => Action::Down,
        KeyCode::Char('g') | KeyCode::Home => Action::Top,
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('e') => Action::Expand,
        KeyCode::Enter => Action::Drill,
        KeyCode::Char('/') => Action::BeginFilter,
        KeyCode::Char(':') => Action::BeginPalette,
        KeyCode::Char('s') => Action::SortCycle,
        KeyCode::Char('S') => Action::SortReverse,
        KeyCode::Char('?') => Action::Help,
        _ => Action::None,
    }
}

/// Pure: which table row does a click at terminal row `y` hit, given the table's
/// inner rect (header at inner.y, data rows from inner.y+1) and current scroll offset.
pub fn row_at(inner: Rect, offset: usize, y: u16) -> Option<usize> {
    if y <= inner.y {
        return None;
    }
    Some(offset + (y - inner.y - 1) as usize)
}

/// Rows currently on screen for `kind` — the filter-aware row count.
fn row_count(app: &App, kind: WidgetKind) -> usize {
    app.visible_count(kind)
}

fn move_selection(app: &mut App, delta: isize) {
    let n = row_count(app, app.focus);
    if n == 0 {
        return;
    }
    let st = app.focus_ui_mut();
    let cur = st.table.selected().unwrap_or(0) as isize;
    let next = (cur + delta).clamp(0, n as isize - 1) as usize;
    st.table.select(Some(next));
}

fn jump_selection(app: &mut App, to_end: bool) {
    let n = row_count(app, app.focus);
    if n == 0 {
        return;
    }
    let st = app.focus_ui_mut();
    st.table.select(Some(if to_end { n - 1 } else { 0 }));
}

/// Combined file list in render order: uncommitted (staged then unstaged) THEN committed.
fn combined_files(d: &git_detail::WorktreeDetail) -> Vec<(String, DiffMode)> {
    let mut v: Vec<(String, DiffMode)> = Vec::new();
    for f in &d.uncommitted_files {
        v.push((
            f.path.clone(),
            if f.staged {
                DiffMode::Staged
            } else {
                DiffMode::Unstaged
            },
        ));
    }
    for f in &d.committed_files {
        v.push((f.path.clone(), DiffMode::Committed));
    }
    v
}

fn wt_file_count(app: &App) -> usize {
    app.wt_detail
        .as_ref()
        .map(|d| d.uncommitted_files.len() + d.committed_files.len())
        .unwrap_or(0)
}

fn open_worktree_detail(app: &mut App) {
    let Some(idx) = app.selected_data_index(WidgetKind::Worktrees) else {
        return;
    };
    let wt = app.data.lock().unwrap().worktrees.get(idx).cloned();
    let Some(wt) = wt else { return };
    let detail = git_detail::worktree_detail(&wt.path, &wt.name, &wt.branch);
    app.wt_detail = Some(detail);
    app.last_wt_idx = Some(idx);
    app.detail_table = ratatui::widgets::TableState::default();
    app.detail_table.select(Some(0));
    app.view = View::Detail(Detail::Worktree);
}

fn open_container_detail(app: &mut App) {
    let Some(idx) = app.selected_data_index(WidgetKind::Docker) else {
        return;
    };
    let c = app.data.lock().unwrap().containers.get(idx).cloned();
    let Some(c) = c else { return };
    app.container_logs = crate::collect::docker::container_logs(&c.id, 300);
    app.view = View::Detail(Detail::Container(idx));
    app.detail_scroll = 0;
}

fn open_cost_detail(app: &mut App) {
    if app.data.lock().unwrap().usage.is_none() {
        return;
    }
    app.detail_table = ratatui::widgets::TableState::default();
    app.detail_table.select(Some(0));
    app.detail_scroll = 0;
    app.view = View::Detail(Detail::Cost);
}

fn open_cost_model(app: &mut App) {
    let Some(sel) = app.detail_table.selected() else {
        return;
    };
    let n = app
        .data
        .lock()
        .unwrap()
        .usage
        .as_ref()
        .map(|u| u.by_model.len())
        .unwrap_or(0);
    if sel < n {
        app.view = View::Detail(Detail::CostModel(sel));
        app.detail_scroll = 0;
    }
}

fn open_file_diff(app: &mut App) {
    let Some(detail) = app.wt_detail.clone() else {
        return;
    };
    let Some(sel) = app.detail_table.selected() else {
        return;
    };
    let files = combined_files(&detail);
    let Some((path, mode)) = files.get(sel) else {
        return;
    };
    let lines = git_detail::file_diff(&detail.path, path, *mode);
    let label = match mode {
        DiffMode::Staged => "staged",
        DiffMode::Unstaged => "unstaged",
        DiffMode::Committed => "committed",
    };
    app.view = View::Detail(Detail::Diff(DiffView {
        title: format!("{} · {}", path, label),
        lines,
    }));
    app.detail_scroll = 0;
}

#[derive(Clone, Copy)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Move focus to the nearest widget in `dir`, using the current frame's widget rects
/// (`app.rects.widgets`). No-op if there is no widget in that direction.
fn focus_dir(app: &mut App, dir: Dir) {
    let cur = app.focus;
    let target = {
        let rects = &app.rects.widgets;
        let Some(cur_rect) = rects.iter().find(|(k, _)| *k == cur).map(|(_, r)| *r) else {
            return;
        };
        let cx = cur_rect.x as i32 + cur_rect.width as i32 / 2;
        let cy = cur_rect.y as i32 + cur_rect.height as i32 / 2;
        let mut best: Option<WidgetKind> = None;
        let mut best_score = i32::MAX;
        for (k, r) in rects {
            if *k == cur {
                continue;
            }
            let rx = r.x as i32 + r.width as i32 / 2;
            let ry = r.y as i32 + r.height as i32 / 2;
            // primary = distance along the travel axis, secondary = off-axis offset.
            let (primary, secondary, valid) = match dir {
                Dir::Down => (ry - cy, (rx - cx).abs(), ry > cy),
                Dir::Up => (cy - ry, (rx - cx).abs(), ry < cy),
                Dir::Right => (rx - cx, (ry - cy).abs(), rx > cx),
                Dir::Left => (cx - rx, (ry - cy).abs(), rx < cx),
            };
            if !valid {
                continue;
            }
            let score = primary * 4 + secondary;
            if score < best_score {
                best_score = score;
                best = Some(*k);
            }
        }
        best
    };
    if let Some(k) = target {
        app.focus = k;
    }
}

/// In-widget "up": detail row-select / scroll, or dashboard table row-select.
fn select_up(app: &mut App, step: isize) {
    match &app.view {
        View::Detail(Detail::Worktree) | View::Detail(Detail::Cost) => {
            let s = app.detail_table.selected().unwrap_or(0);
            app.detail_table
                .select(Some(s.saturating_sub(step as usize)));
        }
        View::Detail(_) => {
            app.detail_scroll = app.detail_scroll.saturating_sub(step as u16);
        }
        _ => move_selection(app, -step),
    }
}

/// In-widget "down": counterpart to [`select_up`].
fn select_down(app: &mut App, step: isize) {
    match &app.view {
        View::Detail(Detail::Worktree) => {
            let n = wt_file_count(app);
            if n > 0 {
                let s = app.detail_table.selected().unwrap_or(0);
                app.detail_table
                    .select(Some((s + step as usize).min(n - 1)));
            }
        }
        View::Detail(Detail::Cost) => {
            let n = app
                .data
                .lock()
                .unwrap()
                .usage
                .as_ref()
                .map(|u| u.by_model.len())
                .unwrap_or(0);
            if n > 0 {
                let s = app.detail_table.selected().unwrap_or(0);
                app.detail_table
                    .select(Some((s + step as usize).min(n - 1)));
            }
        }
        View::Detail(_) => {
            app.detail_scroll = app.detail_scroll.saturating_add(step as u16);
        }
        _ => move_selection(app, step),
    }
}

/// Start a refresh unless one is already running.
fn kick_refresh(app: &App, root: &str) {
    if REFRESH_INFLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let d = app.data.clone();
        let r = root.to_string();
        thread::spawn(move || {
            refresh::refresh_now(&r, &d);
            REFRESH_INFLIGHT.store(false, Ordering::Release);
        });
    }
}

/// True while a manual refresh is running — drives the header spinner.
pub fn refreshing() -> bool {
    REFRESH_INFLIGHT.load(Ordering::Relaxed)
}

/// Advance the focused widget's sort to the next column, wrapping to default.
fn cycle_sort(app: &mut App) {
    let n = view::sort_columns(app.focus).len();
    let st = app.focus_ui_mut();
    st.sort_col = (st.sort_col + 1) % n.max(1);
    app.refilter();
}

fn reverse_sort(app: &mut App) {
    let st = app.focus_ui_mut();
    // Reversing while on the collector's own order is meaningless; step onto
    // the first real column so the key always does something visible.
    if st.sort_col == 0 {
        st.sort_col = 1;
    } else {
        st.sort_desc = !st.sort_desc;
    }
    app.refilter();
}

/// The palette entries plus the indices matching the current input.
pub fn palette_state(app: &mut App) -> (Vec<palette::Entry>, Vec<usize>) {
    let entries = palette::registry(app.focus);
    let input = app.input.clone();
    let hits = palette::search(&entries, &input, &mut app.fuzzy);
    (entries, hits)
}

fn run_command(app: &mut App, cmd: Command, root: &str) {
    match cmd {
        Command::Focus(k) => {
            app.focus = k;
            app.view = View::Dashboard;
        }
        Command::Expand => app.expand_focused(),
        Command::Drill => apply(app, Action::Drill, root),
        Command::Filter => {
            app.mode = Mode::Filter;
            app.input = app.filter_of(app.focus).to_string();
        }
        Command::ClearFilter => {
            app.focus_ui_mut().filter.clear();
            app.refilter();
        }
        Command::Sort(col) => {
            app.focus_ui_mut().sort_col = col;
            app.refilter();
        }
        Command::Refresh => kick_refresh(app, root),
        Command::Help => app.show_help = !app.show_help,
        Command::Quit => app.should_quit = true,
    }
}

fn accept_input(app: &mut App, root: &str) {
    match app.mode {
        // The filter is already live; Enter just hands the keyboard back.
        Mode::Filter => app.mode = Mode::Normal,
        Mode::Palette => {
            let (entries, hits) = palette_state(app);
            let cmd = hits.get(app.palette_sel).map(|&i| entries[i].cmd);
            app.mode = Mode::Normal;
            app.input.clear();
            app.palette_sel = 0;
            if let Some(c) = cmd {
                run_command(app, c, root);
            }
        }
        Mode::Normal => {}
    }
}

/// Esc: unwind exactly one layer of state.
fn escape(app: &mut App) {
    match app.mode {
        Mode::Filter => {
            // Cancel restores the unfiltered view; Enter is how you keep it.
            app.mode = Mode::Normal;
            app.input.clear();
            app.focus_ui_mut().filter.clear();
            app.refilter();
        }
        Mode::Palette => {
            app.mode = Mode::Normal;
            app.input.clear();
            app.palette_sel = 0;
        }
        Mode::Normal => {
            if app.show_help {
                app.show_help = false;
            } else if !app.filter_of(app.focus).is_empty() && matches!(app.view, View::Dashboard) {
                app.focus_ui_mut().filter.clear();
                app.refilter();
            } else {
                app.back();
            }
        }
    }
}

pub fn apply(app: &mut App, action: Action, root: &str) {
    match action {
        Action::Quit => app.should_quit = true,
        Action::Refresh => kick_refresh(app, root),
        Action::FocusNext => app.focus_next(),
        Action::FocusPrev => app.focus_prev(),
        Action::FocusUp => {
            if matches!(app.view, View::Dashboard) && app.mode == Mode::Normal {
                focus_dir(app, Dir::Up);
            } else {
                select_up(app, 1);
            }
        }
        Action::FocusDown => {
            if matches!(app.view, View::Dashboard) && app.mode == Mode::Normal {
                focus_dir(app, Dir::Down);
            } else {
                select_down(app, 1);
            }
        }
        Action::FocusLeft => {
            if matches!(app.view, View::Dashboard) {
                focus_dir(app, Dir::Left);
            }
        }
        Action::FocusRight => {
            if matches!(app.view, View::Dashboard) {
                focus_dir(app, Dir::Right);
            }
        }
        Action::Up => {
            if app.mode == Mode::Palette {
                app.palette_sel = app.palette_sel.saturating_sub(1);
            } else {
                select_up(app, 1)
            }
        }
        Action::Down => {
            if app.mode == Mode::Palette {
                let (_, hits) = palette_state(app);
                app.palette_sel = (app.palette_sel + 1).min(hits.len().saturating_sub(1));
            } else {
                select_down(app, 1)
            }
        }
        Action::Top => jump_selection(app, false),
        Action::Bottom => jump_selection(app, true),
        Action::PageUp => select_up(app, 10),
        Action::PageDown => select_down(app, 10),
        Action::Expand => app.expand_focused(),
        Action::Help => app.show_help = !app.show_help,
        Action::Back => escape(app),
        Action::BeginFilter => {
            if view::is_table(app.focus) {
                app.mode = Mode::Filter;
                app.input = app.filter_of(app.focus).to_string();
            }
        }
        Action::BeginPalette => {
            app.mode = Mode::Palette;
            app.input.clear();
            app.palette_sel = 0;
        }
        Action::SortCycle => cycle_sort(app),
        Action::SortReverse => reverse_sort(app),
        Action::Input(c) => {
            app.input.push(c);
            match app.mode {
                Mode::Filter => {
                    let s = app.input.clone();
                    app.focus_ui_mut().filter = s;
                    app.refilter();
                }
                Mode::Palette => app.palette_sel = 0,
                Mode::Normal => app.input.clear(),
            }
        }
        Action::Backspace => {
            app.input.pop();
            match app.mode {
                Mode::Filter => {
                    let s = app.input.clone();
                    app.focus_ui_mut().filter = s;
                    app.refilter();
                }
                Mode::Palette => app.palette_sel = 0,
                Mode::Normal => {}
            }
        }
        Action::Accept => accept_input(app, root),
        Action::Drill => match &app.view {
            View::Dashboard | View::Expanded(_) => drill(app),
            View::Detail(Detail::Worktree) => open_file_diff(app),
            View::Detail(Detail::Cost) => open_cost_model(app),
            _ => {}
        },
        Action::None => {}
    }
}

/// Open the detail view for the focused widget's selection.
fn drill(app: &mut App) {
    match app.focus {
        WidgetKind::Worktrees => open_worktree_detail(app),
        WidgetKind::Docker => open_container_detail(app),
        WidgetKind::Cost => open_cost_detail(app),
        WidgetKind::Jobs => {
            if let Some(idx) = app.selected_data_index(WidgetKind::Jobs) {
                app.view = View::Detail(Detail::Job(idx));
                app.detail_scroll = 0;
            }
        }
        WidgetKind::Ports => {
            if let Some(idx) = app.selected_data_index(WidgetKind::Ports) {
                app.view = View::Detail(Detail::Ports(idx));
                app.detail_scroll = 0;
            }
        }
        WidgetKind::Repo => {
            // The Repo slot hosts the Skills table — drill into a skill.
            if let Some(idx) = app.selected_data_index(WidgetKind::Repo) {
                app.view = View::Detail(Detail::Repo(idx));
                app.detail_scroll = 0;
            }
        }
        WidgetKind::Activity => {
            let has = {
                let d = app.data.lock().unwrap();
                d.usage.is_some() || !d.activity.is_empty()
            };
            if has {
                app.view = View::Detail(Detail::Activity);
                app.detail_scroll = 0;
            }
        }
        WidgetKind::Code => {
            if !app.data.lock().unwrap().loc.is_empty() {
                app.view = View::Detail(Detail::Code);
                app.detail_scroll = 0;
            }
        }
        WidgetKind::Procs => {} // Tools widget — nothing deeper to show
    }
}

pub fn handle_mouse(app: &mut App, m: MouseEvent, root: &str) {
    let pos = Position {
        x: m.column,
        y: m.row,
    };
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(kind) = app.rects.widget_at(pos) {
                app.focus = kind;
                if view::is_table(kind) {
                    if let Some((_, outer)) =
                        app.rects.widgets.iter().find(|(k, _)| *k == kind).copied()
                    {
                        let inner = Block::default().borders(Borders::ALL).inner(outer);
                        if inner.contains(pos) {
                            let offset = app.ui_offset(kind);
                            if let Some(row) = row_at(inner, offset, pos.y) {
                                if row < row_count(app, kind) {
                                    app.ui_mut(kind).table.select(Some(row));
                                }
                            }
                        }
                    }
                }
            }
        }
        MouseEventKind::ScrollDown => apply(app, Action::Down, root),
        MouseEventKind::ScrollUp => apply(app, Action::Up, root),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::jobs::Job;
    use crate::theme::Theme;

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
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.jobs = vec![
                job("1", "alpha", "working"),
                job("2", "beta", "done"),
                job("3", "gamma", "done"),
            ];
        }
        app.focus = WidgetKind::Jobs;
        app.sync();
        app
    }

    #[test]
    fn maps_keys() {
        let n = Mode::Normal;
        assert_eq!(
            map_key(KeyCode::Char('q'), KeyModifiers::NONE, n),
            Action::Quit
        );
        assert_eq!(
            map_key(KeyCode::Char('e'), KeyModifiers::NONE, n),
            Action::Expand
        );
        assert_eq!(
            map_key(KeyCode::Tab, KeyModifiers::NONE, n),
            Action::FocusNext
        );
        assert_eq!(
            map_key(KeyCode::Char('c'), KeyModifiers::CONTROL, n),
            Action::Quit
        );
        assert_eq!(
            map_key(KeyCode::Enter, KeyModifiers::NONE, n),
            Action::Drill
        );
        // Arrows move focus between widgets; j/k select rows within one.
        assert_eq!(
            map_key(KeyCode::Down, KeyModifiers::NONE, n),
            Action::FocusDown
        );
        assert_eq!(
            map_key(KeyCode::Left, KeyModifiers::NONE, n),
            Action::FocusLeft
        );
        assert_eq!(
            map_key(KeyCode::Char('j'), KeyModifiers::NONE, n),
            Action::Down
        );
        assert_eq!(
            map_key(KeyCode::Char('k'), KeyModifiers::NONE, n),
            Action::Up
        );
        // New vocabulary.
        assert_eq!(
            map_key(KeyCode::Char('/'), KeyModifiers::NONE, n),
            Action::BeginFilter
        );
        assert_eq!(
            map_key(KeyCode::Char(':'), KeyModifiers::NONE, n),
            Action::BeginPalette
        );
        assert_eq!(
            map_key(KeyCode::Char('k'), KeyModifiers::CONTROL, n),
            Action::BeginPalette
        );
        assert_eq!(
            map_key(KeyCode::Char('g'), KeyModifiers::NONE, n),
            Action::Top
        );
        assert_eq!(
            map_key(KeyCode::Char('G'), KeyModifiers::NONE, n),
            Action::Bottom
        );
        assert_eq!(
            map_key(KeyCode::Char('s'), KeyModifiers::NONE, n),
            Action::SortCycle
        );
    }

    #[test]
    fn typing_mode_captures_letters_instead_of_navigating() {
        // 'q' quits in Normal but types in Filter — otherwise you could not
        // search for anything containing a bound letter.
        assert_eq!(
            map_key(KeyCode::Char('q'), KeyModifiers::NONE, Mode::Filter),
            Action::Input('q')
        );
        assert_eq!(
            map_key(KeyCode::Char('j'), KeyModifiers::NONE, Mode::Palette),
            Action::Input('j')
        );
        // Ctrl-C still quits from anywhere.
        assert_eq!(
            map_key(KeyCode::Char('c'), KeyModifiers::CONTROL, Mode::Filter),
            Action::Quit
        );
        assert_eq!(
            map_key(KeyCode::Esc, KeyModifiers::NONE, Mode::Filter),
            Action::Back
        );
    }

    #[test]
    fn arrows_move_focus_spatially() {
        let mut app = App::new(Theme::default());
        app.rects.widgets = vec![
            (
                WidgetKind::Worktrees,
                Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 10,
                },
            ),
            (
                WidgetKind::Jobs,
                Rect {
                    x: 0,
                    y: 10,
                    width: 40,
                    height: 10,
                },
            ),
        ];
        app.focus = WidgetKind::Worktrees;
        apply(&mut app, Action::FocusDown, ".");
        assert_eq!(app.focus, WidgetKind::Jobs);
        apply(&mut app, Action::FocusUp, ".");
        assert_eq!(app.focus, WidgetKind::Worktrees);
        // Nothing to the left → focus stays put.
        apply(&mut app, Action::FocusLeft, ".");
        assert_eq!(app.focus, WidgetKind::Worktrees);
    }

    #[test]
    fn row_at_maps_clicks() {
        let inner = Rect {
            x: 0,
            y: 5,
            width: 20,
            height: 10,
        };
        assert_eq!(row_at(inner, 0, 5), None); // header row
        assert_eq!(row_at(inner, 0, 6), Some(0)); // first data row
        assert_eq!(row_at(inner, 3, 7), Some(4)); // offset 3 + 2nd visible row
    }

    #[test]
    fn filter_flow_narrows_then_cancels() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::BeginFilter, ".");
        assert_eq!(app.mode, Mode::Filter);
        for c in "gam".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        assert_eq!(app.visible_count(WidgetKind::Jobs), 1);
        // Enter keeps the filter and returns to navigation.
        apply(&mut app, Action::Accept, ".");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.visible_count(WidgetKind::Jobs), 1);
        // Esc from Normal clears a live filter.
        apply(&mut app, Action::Back, ".");
        assert_eq!(app.visible_count(WidgetKind::Jobs), 3);
    }

    #[test]
    fn escape_during_filter_restores_everything() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::BeginFilter, ".");
        apply(&mut app, Action::Input('z'), ".");
        assert_eq!(app.visible_count(WidgetKind::Jobs), 0);
        apply(&mut app, Action::Back, ".");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.visible_count(WidgetKind::Jobs), 3);
        assert!(app.input.is_empty());
    }

    #[test]
    fn backspace_widens_the_filter_again() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::BeginFilter, ".");
        for c in "gamma".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        assert_eq!(app.visible_count(WidgetKind::Jobs), 1);
        for _ in 0..5 {
            apply(&mut app, Action::Backspace, ".");
        }
        assert_eq!(app.visible_count(WidgetKind::Jobs), 3);
    }

    #[test]
    fn drill_follows_the_filtered_row_not_the_raw_index() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::BeginFilter, ".");
        for c in "gamma".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        apply(&mut app, Action::Accept, ".");
        apply(&mut app, Action::Drill, ".");
        // gamma is data row 2, displayed at row 0.
        match app.view {
            View::Detail(Detail::Job(i)) => assert_eq!(i, 2),
            _ => panic!("expected the job detail for gamma"),
        }
    }

    #[test]
    fn palette_runs_the_selected_command() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::BeginPalette, ".");
        assert_eq!(app.mode, Mode::Palette);
        for c in "dock".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        apply(&mut app, Action::Accept, ".");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.focus, WidgetKind::Docker);
        assert!(app.input.is_empty());
    }

    #[test]
    fn palette_escape_runs_nothing() {
        let mut app = app_with_jobs();
        let before = app.focus;
        apply(&mut app, Action::BeginPalette, ".");
        for c in "dock".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        apply(&mut app, Action::Back, ".");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.focus, before);
    }

    #[test]
    fn sort_cycles_through_columns_and_wraps() {
        let mut app = app_with_jobs();
        let n = view::sort_columns(WidgetKind::Jobs).len();
        for i in 1..n {
            apply(&mut app, Action::SortCycle, ".");
            assert_eq!(app.ui[&WidgetKind::Jobs].sort_col, i);
        }
        apply(&mut app, Action::SortCycle, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].sort_col, 0, "sort must wrap home");
    }

    #[test]
    fn reverse_from_default_order_picks_a_real_column() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::SortReverse, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].sort_col, 1);
        apply(&mut app, Action::SortReverse, ".");
        assert!(app.ui[&WidgetKind::Jobs].sort_desc);
    }

    #[test]
    fn g_and_shift_g_jump_to_the_ends() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::Bottom, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(2));
        apply(&mut app, Action::Top, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(0));
    }

    #[test]
    fn paging_clamps_at_both_ends() {
        let mut app = app_with_jobs();
        apply(&mut app, Action::PageDown, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(2));
        apply(&mut app, Action::PageUp, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(0));
    }

    #[test]
    fn cost_drill_and_back() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.usage = Some(crate::collect::usage::UsageTotals {
                by_model: vec![crate::collect::usage::ModelUsage {
                    model: "claude-x".into(),
                    cost_usd: 1.0,
                    input: 1,
                    output: 1,
                    cache_write: 0,
                    cache_read: 0,
                }],
                ..Default::default()
            });
        }
        app.focus = WidgetKind::Cost;
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::Cost)));
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::CostModel(0))));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Detail(Detail::Cost)));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }

    #[test]
    fn skills_drill_opens_detail() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.skills.push(crate::collect::skills::Skill {
                name: "deploy".into(),
                description: "Ship it.".into(),
                source: "personal".into(),
                path: None,
                uses: 3,
                last_used: None,
            });
        }
        app.focus = WidgetKind::Repo;
        app.sync();
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::Repo(0))));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }
}
