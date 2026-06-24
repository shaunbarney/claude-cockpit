//! Input handling: crossterm key/mouse -> Action -> applied to App, plus the run loop.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static REFRESH_INFLIGHT: AtomicBool = AtomicBool::new(false);

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Borders};

use crate::app::{App, Detail, DiffView, View};
use crate::collect::git_detail::{self, DiffMode};
use crate::refresh;
use crate::theme::Theme;
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
    Expand,
    Drill,
    Back,
    Help,
    None,
}

/// Pure key -> action map.
pub fn map_key(code: KeyCode, mods: KeyModifiers) -> Action {
    if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
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
        KeyCode::Char('e') => Action::Expand,
        KeyCode::Enter => Action::Drill,
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

fn row_count(app: &App, kind: WidgetKind) -> usize {
    match kind {
        WidgetKind::Worktrees => app.data.lock().unwrap().worktrees.len(),
        WidgetKind::Jobs => app.data.lock().unwrap().jobs.len(),
        WidgetKind::Docker => app.data.lock().unwrap().containers.len(),
        WidgetKind::Ports => app.data.lock().unwrap().endpoints.len(),
        _ => 0, // extended per phase as more widgets become selectable
    }
}

fn is_table_widget(kind: WidgetKind) -> bool {
    matches!(
        kind,
        WidgetKind::Worktrees
            | WidgetKind::Jobs
            | WidgetKind::Docker
            | WidgetKind::Ports
    )
}

fn move_selection(app: &mut App, down: bool) {
    let n = row_count(app, app.focus);
    if n == 0 {
        return;
    }
    let st = app.focus_ui_mut();
    let cur = st.table.selected().unwrap_or(0);
    let next = if down {
        (cur + 1).min(n - 1)
    } else {
        cur.saturating_sub(1)
    };
    st.table.select(Some(next));
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
    let Some(idx) = app
        .ui
        .get(&WidgetKind::Worktrees)
        .and_then(|u| u.table.selected())
    else {
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
    let Some(idx) = app
        .ui
        .get(&WidgetKind::Docker)
        .and_then(|u| u.table.selected())
    else {
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
/// Driven by `k`, and by the arrow keys when not on the dashboard grid.
fn select_up(app: &mut App) {
    match &app.view {
        View::Detail(Detail::Worktree) | View::Detail(Detail::Cost) => {
            let s = app.detail_table.selected().unwrap_or(0);
            app.detail_table.select(Some(s.saturating_sub(1)));
        }
        View::Detail(_) => {
            app.detail_scroll = app.detail_scroll.saturating_sub(1);
        }
        _ => move_selection(app, false),
    }
}

/// In-widget "down": counterpart to [`select_up`].
fn select_down(app: &mut App) {
    match &app.view {
        View::Detail(Detail::Worktree) => {
            let n = wt_file_count(app);
            if n > 0 {
                let s = app.detail_table.selected().unwrap_or(0);
                app.detail_table.select(Some((s + 1).min(n - 1)));
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
                app.detail_table.select(Some((s + 1).min(n - 1)));
            }
        }
        View::Detail(_) => {
            app.detail_scroll = app.detail_scroll.saturating_add(1);
        }
        _ => move_selection(app, true),
    }
}

fn apply(app: &mut App, action: Action, root: &str) {
    match action {
        Action::Quit => app.should_quit = true,
        Action::Refresh => {
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
        Action::FocusNext => app.focus_next(),
        Action::FocusPrev => app.focus_prev(),
        Action::FocusUp => {
            if matches!(app.view, View::Dashboard) {
                focus_dir(app, Dir::Up);
            } else {
                select_up(app);
            }
        }
        Action::FocusDown => {
            if matches!(app.view, View::Dashboard) {
                focus_dir(app, Dir::Down);
            } else {
                select_down(app);
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
        Action::Up => select_up(app),
        Action::Down => select_down(app),
        Action::Expand => app.expand_focused(),
        Action::Help => app.show_help = !app.show_help,
        Action::Back => {
            if app.show_help {
                app.show_help = false;
            } else {
                app.back();
            }
        }
        Action::Drill => match &app.view {
            View::Dashboard | View::Expanded(_) => match app.focus {
                WidgetKind::Worktrees => open_worktree_detail(app),
                WidgetKind::Jobs => {
                    if let Some(idx) = app
                        .ui
                        .get(&WidgetKind::Jobs)
                        .and_then(|u| u.table.selected())
                    {
                        let n = app.data.lock().unwrap().jobs.len();
                        if idx < n {
                            app.view = View::Detail(Detail::Job(idx));
                            app.detail_scroll = 0;
                        }
                    }
                }
                WidgetKind::Docker => open_container_detail(app),
                WidgetKind::Cost => open_cost_detail(app),
                WidgetKind::Activity => {
                    if app.data.lock().unwrap().usage.is_some()
                        || !app.data.lock().unwrap().activity.is_empty()
                    {
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
                WidgetKind::Ports => {
                    if let Some(idx) = app
                        .ui
                        .get(&WidgetKind::Ports)
                        .and_then(|u| u.table.selected())
                    {
                        let n = app.data.lock().unwrap().endpoints.len();
                        if idx < n {
                            app.view = View::Detail(Detail::Ports(idx));
                            app.detail_scroll = 0;
                        }
                    }
                }
                WidgetKind::Repo => {
                    if app.data.lock().unwrap().repo.is_some() {
                        app.view = View::Detail(Detail::Repo);
                        app.detail_scroll = 0;
                    }
                }
                WidgetKind::Procs => {} // Tools widget — not drillable
            },
            View::Detail(Detail::Worktree) => open_file_diff(app),
            View::Detail(Detail::Cost) => open_cost_model(app),
            _ => {}
        },
        Action::None => {}
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent, root: &str) {
    let pos = Position {
        x: m.column,
        y: m.row,
    };
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(kind) = app.rects.widget_at(pos) {
                app.focus = kind;
                if is_table_widget(kind) {
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

/// Enter alt-screen + mouse capture, run the loop, restore on exit (incl. panic via ratatui hook).
pub fn run(root: &str) -> io::Result<()> {
    let mut term = ratatui::init();
    if let Err(e) = execute!(io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(e);
    }
    let mut app = App::new(Theme::default());
    let stop = refresh::spawn(root.to_string(), app.data.clone());
    let res = event_loop(&mut term, &mut app, root);
    stop.store(true, Ordering::Relaxed);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

fn event_loop<B: ratatui::backend::Backend>(
    term: &mut ratatui::Terminal<B>,
    app: &mut App,
    root: &str,
) -> io::Result<()> {
    // Redraw only when the data revision changes or input arrives, so an idle
    // dashboard does no work. `u64::MAX` forces the first frame. Background
    // refreshes bump `rev` every 2–10 s, which keeps time-based fields (ages,
    // reset countdowns) ticking without a busy redraw loop.
    let mut last_rev = u64::MAX;
    loop {
        let rev = { app.data.lock().unwrap().rev };
        if rev != last_rev {
            term.draw(|f| crate::render::dashboard::render(f, app))?;
            last_rev = rev;
        }
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    let a = map_key(k.code, k.modifiers);
                    apply(app, a, root);
                }
                Event::Mouse(m) => handle_mouse(app, m, root),
                Event::Resize(_, _) => last_rev = u64::MAX, // force a redraw next iteration
                _ => {}
            }
            // Input may have changed view/scroll state — redraw regardless of rev.
            if !app.should_quit {
                term.draw(|f| crate::render::dashboard::render(f, app))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_keys() {
        assert!(matches!(
            map_key(KeyCode::Char('q'), KeyModifiers::NONE),
            Action::Quit
        ));
        assert!(matches!(
            map_key(KeyCode::Char('e'), KeyModifiers::NONE),
            Action::Expand
        ));
        assert!(matches!(
            map_key(KeyCode::Tab, KeyModifiers::NONE),
            Action::FocusNext
        ));
        assert!(matches!(
            map_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit
        ));
        assert!(matches!(
            map_key(KeyCode::Enter, KeyModifiers::NONE),
            Action::Drill
        ));
        // Arrows move focus between widgets; j/k select rows within one.
        assert!(matches!(
            map_key(KeyCode::Down, KeyModifiers::NONE),
            Action::FocusDown
        ));
        assert!(matches!(
            map_key(KeyCode::Left, KeyModifiers::NONE),
            Action::FocusLeft
        ));
        assert!(matches!(
            map_key(KeyCode::Char('j'), KeyModifiers::NONE),
            Action::Down
        ));
        assert!(matches!(
            map_key(KeyCode::Char('k'), KeyModifiers::NONE),
            Action::Up
        ));
    }

    #[test]
    fn arrows_move_focus_spatially() {
        let mut app = App::new(Theme::default());
        // Two widgets stacked vertically: Worktrees on top, Jobs below.
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
    fn cost_drill_and_back() {
        let mut app = App::new(Theme::default());
        // Seed minimal usage so the drill is allowed.
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
        // Enter on the selected model drills one level deeper.
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::CostModel(0))));
        // Back pops CostModel -> Cost, then Cost -> Dashboard.
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Detail(Detail::Cost)));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }

    #[test]
    fn repo_drill_opens_detail() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.repo = Some(crate::collect::git::RepoHealth {
                branch: "main".into(),
                ahead: 0,
                behind: 0,
                stash: 0,
                dirty: 0,
                last_fetch_secs: None,
            });
        }
        app.focus = WidgetKind::Repo;
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::Repo)));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }
}
