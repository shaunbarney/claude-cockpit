//! Compose the screen for the current view: header, widget grid, footer, overlays.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, TableState};
use ratatui::Frame;

use crate::app::{App, Mode, View};
use crate::layout::{band, cols_for, place, place_with, widgets_for, Band, FrameRects};
use crate::render::widgets::{self, TableCtx};
use crate::render::{footer, header, overlay};
use crate::rows::TableRow;
use crate::theme::Theme;
use crate::widget::WidgetKind;

/// Rows a chartless widget wants, so the grid can weigh it against the tables.
const CHART_ROWS: usize = 10;

pub fn render(f: &mut Frame, app: &mut App) {
    app.area = f.area();
    app.anim_visible = false;
    app.rects = FrameRects::default();
    draw_content(f, app);

    if app.mode == Mode::Palette {
        let theme = app.theme.clone();
        let input = app.input.clone();
        let sel = app.palette_sel;
        let (entries, hits) = crate::event::palette_state(app);
        overlay::palette(f, f.area(), &input, &entries, &hits, sel, &theme);
    }
    if app.show_help {
        overlay::help(f, f.area(), &app.theme.clone());
    }
    if let Some(picker) = app.picker.as_ref() {
        crate::render::projects::render(f, f.area(), picker, &app.theme);
    }
}

/// What a widget says when it has nothing to show.
fn empty_msg(kind: WidgetKind, filter: &str) -> String {
    if !filter.is_empty() {
        return format!("nothing matches “{filter}”");
    }
    match kind {
        WidgetKind::Worktrees => "no linked worktrees — git worktree add …".into(),
        WidgetKind::Jobs => "no background agents".into(),
        WidgetKind::Docker => "no containers — is the docker daemon up?".into(),
        WidgetKind::Ports => "no dev endpoints found".into(),
        WidgetKind::Repo => "no skills in ~/.claude/skills or this repo".into(),
        WidgetKind::Procs => "no tool usage recorded yet".into(),
        WidgetKind::Code => "nothing to count here".into(),
        WidgetKind::Cost | WidgetKind::Activity => "no usage data".into(),
    }
}

/// Take the rows a widget should display, in display order.
fn pick<T: Clone>(all: &[T], vis: &[usize]) -> Vec<T> {
    vis.iter().filter_map(|&i| all.get(i).cloned()).collect()
}

/// Change-highlight style per displayed row, parallel to [`pick`]'s output.
///
/// Also notes on `app` whether any drawn row is still fading: the run loop
/// only pays for a fast tick while motion is actually on screen.
fn flash_for<T: TableRow>(
    all: &[T],
    vis: &[usize],
    kind: WidgetKind,
    app: &mut App,
    theme: &Theme,
) -> Vec<Option<Style>> {
    let out: Vec<Option<Style>> = vis
        .iter()
        .filter_map(|&i| all.get(i))
        .map(|r| {
            let key = App::flash_key(kind, &r.key());
            theme.flash_style(app.flashes.intensity(&key))
        })
        .collect();
    app.anim_visible |= out.iter().any(Option::is_some);
    out
}

/// Rows a widget will actually draw — what the grid weighs it by.
fn content_rows(app: &App, kind: WidgetKind) -> usize {
    match kind {
        WidgetKind::Cost | WidgetKind::Activity => CHART_ROWS,
        // Code draws its TOTAL in the border, so its rows are its languages.
        _ => app.visible_count(kind).max(1),
    }
}

fn too_small(f: &mut Frame, area: Rect) {
    let p = Paragraph::new("Resize terminal (need >= 70x18)").alignment(Alignment::Center);
    let v = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .flex(Flex::Center)
    .split(area);
    f.render_widget(p, v[1]);
}

fn draw_content(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let now = chrono::Utc::now().timestamp();
    let today = crate::collect::usage::today_local();

    // One band for the whole frame. Deriving the guard from the full area but
    // the grid from the body (two rows shorter) left heights 18-19 passing the
    // guard and then placing no widgets at all.
    let b = band(area);
    if b == Band::TooSmall {
        return too_small(f, area);
    }

    if crate::render::detail::screen::draw(f, app, area, now, &today) {
        return;
    }

    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    let (head, body, foot) = (outer[0], outer[1], outer[2]);

    let (kinds, cols) = match app.view {
        View::Expanded(k) => (vec![k], 1usize),
        _ => (widgets_for(b), cols_for(b)),
    };
    // A resize can push the focused widget off the grid; keep the keyboard on
    // something visible rather than stranded on a hidden cell.
    if matches!(app.view, View::Dashboard) && !kinds.contains(&app.focus) {
        if let Some(&first) = kinds.first() {
            app.focus = first;
        }
    }

    let placed = if matches!(app.view, View::Expanded(_)) {
        place(body, &kinds, cols)
    } else {
        place_with(body, &kinds, cols, |k| content_rows(app, k))
    };
    app.rects.widgets = placed.clone();

    let theme = app.theme.clone();
    let focus = app.focus;

    // Snapshot the datasets, then drop the lock before rendering.
    let data = app.data.lock().unwrap();
    let repo = data.repo.clone();
    let kpis = header::kpis(&data, &today);
    let wts = data.worktrees.clone();
    let loc = data.loc.clone();
    let jobs = data.jobs.clone();
    let usage = data.usage.clone();
    let containers = data.containers.clone();
    let endpoints = data.endpoints.clone();
    let tools = data.tools.clone();
    let skills = data.skills.clone();
    drop(data);

    let busy = crate::event::refreshing();
    let pulse = if busy {
        app.clock.spinner()
    } else {
        app.clock.pulse()
    };
    header::render(f, head, &repo, &kpis, &theme, pulse, busy);

    let d = WidgetData {
        wts: &wts,
        loc: &loc,
        jobs: &jobs,
        usage: usage.as_ref(),
        containers: &containers,
        endpoints: &endpoints,
        tools: &tools,
        skills: &skills,
    };
    for (kind, rect) in &placed {
        let cx = Cx {
            theme: &theme,
            band: b,
            focused: *kind == focus,
            now,
            today: &today,
        };
        draw_widget(f, app, *kind, *rect, &cx, &d);
    }

    footer::render(f, foot, app);
}

/// Borrowed snapshot of every dataset, so the widget loop takes one parameter
/// instead of eight.
struct WidgetData<'a> {
    wts: &'a [crate::collect::git::Worktree],
    loc: &'a [crate::collect::loc::LocRow],
    jobs: &'a [crate::collect::jobs::Job],
    usage: Option<&'a crate::collect::usage::UsageTotals>,
    containers: &'a [crate::collect::docker::Container],
    endpoints: &'a [crate::collect::ports::Endpoint],
    tools: &'a [crate::collect::tools::ToolStat],
    skills: &'a [crate::collect::skills::Skill],
}

/// Per-widget presentation context.
struct Cx<'a> {
    theme: &'a Theme,
    band: Band,
    focused: bool,
    now: i64,
    today: &'a str,
}

/// Draw a table with `kind`'s selection — but only while it has focus.
///
/// An unfocused table showing a reversed row looks exactly like a focused one,
/// so the eye cannot tell where the keyboard is. Unfocused tables render with
/// their scroll offset and no selection; the real state is left untouched.
fn with_state(app: &mut App, kind: WidgetKind, focused: bool, draw: impl FnOnce(&mut TableState)) {
    let st = &mut app.ui.entry(kind).or_default().table;
    if focused {
        draw(st);
    } else {
        let mut tmp = TableState::new().with_offset(st.offset());
        draw(&mut tmp);
    }
}

fn draw_widget(
    f: &mut Frame,
    app: &mut App,
    kind: WidgetKind,
    rect: Rect,
    cx: &Cx<'_>,
    d: &WidgetData<'_>,
) {
    let (theme, focused, b) = (cx.theme, cx.focused, cx.band);
    let vis = app.visible.get(&kind).cloned().unwrap_or_default();
    let filter = app.filter_of(kind).to_string();
    let empty = empty_msg(kind, &filter);

    macro_rules! ctx {
        ($flash:expr) => {
            TableCtx {
                theme,
                focused,
                band: b,
                flash: $flash,
                empty: &empty,
            }
        };
    }

    match kind {
        WidgetKind::Worktrees => {
            let rows = pick(d.wts, &vis);
            let flash = flash_for(d.wts, &vis, kind, app, theme);
            with_state(app, kind, focused, |st| {
                widgets::worktrees::render(f, rect, &rows, ctx!(&flash), st)
            });
        }
        WidgetKind::Jobs => {
            let rows = pick(d.jobs, &vis);
            let flash = flash_for(d.jobs, &vis, kind, app, theme);
            with_state(app, kind, focused, |st| {
                let title = crate::scope::jobs_title(crate::refresh::jobs_all());
                widgets::jobs::render(f, rect, &rows, ctx!(&flash), st, cx.now, title)
            });
        }
        WidgetKind::Docker => {
            let rows = pick(d.containers, &vis);
            let flash = flash_for(d.containers, &vis, kind, app, theme);
            with_state(app, kind, focused, |st| {
                let title = crate::scope::docker_title(crate::scope::docker_all());
                widgets::docker::render(f, rect, &rows, ctx!(&flash), st, title)
            });
        }
        WidgetKind::Ports => {
            let rows = pick(d.endpoints, &vis);
            let flash = flash_for(d.endpoints, &vis, kind, app, theme);
            with_state(app, kind, focused, |st| {
                widgets::ports::render(f, rect, &rows, ctx!(&flash), st)
            });
        }
        WidgetKind::Repo => {
            let rows = pick(d.skills, &vis);
            let flash = flash_for(d.skills, &vis, kind, app, theme);
            with_state(app, kind, focused, |st| {
                widgets::skills::render(f, rect, &rows, ctx!(&flash), st)
            });
        }
        WidgetKind::Code => {
            let rows = pick(d.loc, &vis);
            with_state(app, kind, focused, |st| {
                widgets::code::render(f, rect, &rows, theme, focused, st)
            });
        }
        WidgetKind::Procs => {
            let rows = pick(d.tools, &vis);
            draw_tools(f, app, rect, &rows, cx);
        }
        WidgetKind::Cost => {
            widgets::cost::render(f, rect, d.usage, theme, focused, b, cx.today);
        }
        WidgetKind::Activity => {
            widgets::activity::render(f, rect, d.usage, theme, focused, b);
        }
    }
}

/// The Tools widget draws a plain (stateless) table, so the selection is
/// driven from here: keep the selected row in view by passing the renderer a
/// window of rows starting at the scroll offset, then paint the cursor row.
fn draw_tools(
    f: &mut Frame,
    app: &mut App,
    rect: Rect,
    rows: &[crate::collect::tools::ToolStat],
    cx: &Cx<'_>,
) {
    // Borders (2) + header (1).
    let page = rect.height.saturating_sub(3).max(1) as usize;
    let st = &mut app.ui.entry(WidgetKind::Procs).or_default().table;
    let sel = st.selected().filter(|&s| s < rows.len());
    let mut off = st.offset().min(rows.len().saturating_sub(1));
    if let Some(s) = sel {
        if s < off {
            off = s;
        } else if s >= off + page {
            off = s + 1 - page;
        }
    }
    *st.offset_mut() = off;
    let window = rows.get(off..).unwrap_or(&[]);
    widgets::tools::render(f, rect, window, cx.theme, cx.focused, cx.band);

    let inner = Block::default().borders(Borders::ALL).inner(rect);
    if let (true, Some(s)) = (cx.focused, sel) {
        let y = inner.y + 1 + (s - off) as u16;
        if y < inner.bottom() {
            let line = Rect {
                y,
                height: 1,
                ..inner
            };
            f.buffer_mut()
                .set_style(line, Style::new().add_modifier(Modifier::REVERSED));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::jobs::Job;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn draw(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        buffer_text(term.backend().buffer())
    }

    fn job(id: &str, name: &str) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            state: "working".into(),
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

    #[test]
    fn wide_shows_widget_titles() {
        let mut app = App::new(Theme::default());
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("Worktrees"));
        assert!(s.contains("Code"));
    }

    #[test]
    fn too_small_shows_guard() {
        let mut app = App::new(Theme::default());
        let s = draw(&mut app, 50, 10).to_lowercase();
        assert!(s.contains("resize"));
    }

    #[test]
    fn empty_widgets_explain_themselves() {
        let mut app = App::new(Theme::default());
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("no linked worktrees"), "worktrees empty state");
        assert!(s.contains("no background agents"), "jobs empty state");
    }

    #[test]
    fn header_and_footer_are_present() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.repo = crate::collect::git::RepoInfo {
                name: "claude-cockpit".into(),
                branch: "main".into(),
            };
            d.jobs = vec![job("1", "alpha")];
        }
        app.sync();
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("claude-cockpit"), "header repo name");
        assert!(s.contains("agents working"), "header KPI");
        assert!(s.contains("commands"), "footer hint for the palette");
    }

    #[test]
    fn filter_narrows_the_rendered_table() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.jobs = vec![job("1", "alpha"), job("2", "zulu")];
        }
        app.focus = WidgetKind::Jobs;
        app.sync();
        assert!(draw(&mut app, 200, 60).contains("zulu"));

        app.ui_mut(WidgetKind::Jobs).filter = "alpha".into();
        app.refilter();
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("alpha"));
        assert!(!s.contains("zulu"), "filtered-out row must not render");
        assert!(s.contains("filter:alpha"), "footer must show the filter");
    }

    #[test]
    fn filter_with_no_matches_says_so() {
        let mut app = App::new(Theme::default());
        {
            app.data.lock().unwrap().jobs = vec![job("1", "alpha")];
        }
        app.focus = WidgetKind::Jobs;
        app.sync();
        app.ui_mut(WidgetKind::Jobs).filter = "qqqq".into();
        app.refilter();
        assert!(draw(&mut app, 200, 60).contains("nothing matches"));
    }

    #[test]
    fn palette_overlay_draws_over_the_dashboard() {
        let mut app = App::new(Theme::default());
        app.mode = Mode::Palette;
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("Command"));
        assert!(s.contains("Go to"));
    }

    #[test]
    fn help_overlay_draws_over_the_dashboard() {
        let mut app = App::new(Theme::default());
        app.show_help = true;
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("Keys"));
        assert!(s.contains("command palette"));
    }

    #[test]
    fn dense_widget_gets_more_rows_than_an_empty_one() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.jobs = (0..14)
                .map(|i| job(&i.to_string(), &format!("job{i}")))
                .collect();
        }
        app.sync();
        let _ = draw(&mut app, 200, 60);
        let h = |k: WidgetKind| {
            app.rects
                .widgets
                .iter()
                .find(|(kk, _)| *kk == k)
                .map(|(_, r)| r.height)
                .unwrap_or(0)
        };
        // Jobs shares a grid row with Worktrees and Cost, so compare rows:
        // the row holding 14 jobs must be taller than the bottom row of
        // empty Ports/Tools/Skills widgets.
        assert!(
            h(WidgetKind::Jobs) > h(WidgetKind::Ports),
            "{} vs {}",
            h(WidgetKind::Jobs),
            h(WidgetKind::Ports)
        );
    }

    // ---- Regression tests for the interactive-layer audit ----------------

    use crate::app::{Detail, DiffView};
    use crate::event::{apply, Action};

    fn three_jobs() -> App {
        let mut app = App::new(Theme::default());
        app.data.lock().unwrap().jobs =
            vec![job("1", "alpha"), job("2", "beta"), job("3", "gamma")];
        app.focus = WidgetKind::Jobs;
        app.sync();
        app
    }

    fn publish_jobs(app: &mut App, jobs: Vec<Job>) {
        {
            let mut d = app.data.lock().unwrap();
            d.jobs = jobs;
            d.rev += 1;
        }
        app.sync();
    }

    #[test]
    fn detail_follows_the_job_not_its_index_across_a_reorder() {
        let mut app = three_jobs();
        apply(&mut app, Action::Drill, ".");
        assert!(draw(&mut app, 120, 30).contains("Job · alpha"));

        // A refresh reorders the jobs: the detail must still show alpha.
        publish_jobs(
            &mut app,
            vec![job("3", "gamma"), job("2", "beta"), job("1", "alpha")],
        );
        let s = draw(&mut app, 120, 30);
        assert!(s.contains("Job · alpha"), "detail swapped to another job");
        assert!(!s.contains("Job · gamma"));

        // And once alpha is gone, say so instead of showing a neighbour.
        publish_jobs(&mut app, vec![job("2", "beta")]);
        let s = draw(&mut app, 120, 30);
        assert!(s.contains("no longer present"), "{s}");
        assert!(!s.contains("Job · beta"));
    }

    #[test]
    fn selection_follows_its_row_when_rows_above_disappear() {
        let mut app = three_jobs();
        apply(&mut app, Action::Down, ".");
        assert_eq!(app.selected_key(WidgetKind::Jobs).as_deref(), Some("2"));
        publish_jobs(&mut app, vec![job("2", "beta"), job("3", "gamma")]);
        assert_eq!(app.selected_key(WidgetKind::Jobs).as_deref(), Some("2"));
        publish_jobs(
            &mut app,
            vec![job("3", "gamma"), job("1", "alpha"), job("2", "beta")],
        );
        assert_eq!(app.selected_key(WidgetKind::Jobs).as_deref(), Some("2"));
        // A vanished row falls back to clamping the old position.
        publish_jobs(&mut app, vec![job("3", "gamma")]);
        assert_eq!(app.selected_key(WidgetKind::Jobs).as_deref(), Some("3"));
    }

    #[test]
    fn heights_just_above_the_guard_still_draw_the_grid() {
        for h in [18, 19] {
            let mut app = App::new(Theme::default());
            let s = draw(&mut app, 100, h);
            assert!(!s.to_lowercase().contains("resize"), "h={h} hit the guard");
            assert!(!app.rects.widgets.is_empty(), "h={h} placed no widgets");
            assert!(s.contains("Worktrees"), "h={h}");
        }
        let mut app = App::new(Theme::default());
        assert!(draw(&mut app, 100, 17).to_lowercase().contains("resize"));
    }

    #[test]
    fn tab_skips_widgets_the_band_hides_and_palette_expands_them() {
        let mut app = App::new(Theme::default());
        app.focus = WidgetKind::Docker; // last widget placed in the Medium band
        let s = draw(&mut app, 140, 40);
        assert!(s.contains("+3 hidden"), "footer must admit hidden widgets");
        apply(&mut app, Action::FocusNext, ".");
        assert_eq!(
            app.focus,
            WidgetKind::Worktrees,
            "Tab landed on a hidden widget"
        );
        apply(&mut app, Action::FocusPrev, ".");
        assert_eq!(app.focus, WidgetKind::Docker);

        // The palette can still reach a hidden widget — by expanding it.
        apply(&mut app, Action::BeginPalette, ".");
        for c in "go to ports".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        apply(&mut app, Action::Accept, ".");
        assert_eq!(app.focus, WidgetKind::Ports);
        assert!(matches!(app.view, View::Expanded(WidgetKind::Ports)));
        assert!(draw(&mut app, 140, 40).contains("Ports"));
    }

    #[test]
    fn tab_in_expanded_moves_the_view_with_the_focus() {
        let mut app = three_jobs();
        app.focus = WidgetKind::Worktrees;
        apply(&mut app, Action::Expand, ".");
        apply(&mut app, Action::FocusNext, ".");
        assert_eq!(app.focus, WidgetKind::Jobs);
        assert!(matches!(app.view, View::Expanded(WidgetKind::Jobs)));
        assert!(draw(&mut app, 120, 30).contains("alpha"));

        // Back from a detail opened in Expanded returns to Expanded.
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::Job(_))));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Expanded(WidgetKind::Jobs)));
    }

    #[test]
    fn detail_keys_do_not_touch_the_hidden_dashboard() {
        let mut app = three_jobs();
        apply(&mut app, Action::Drill, ".");
        let _ = draw(&mut app, 120, 30);

        apply(&mut app, Action::BeginFilter, ".");
        assert_eq!(
            app.mode,
            Mode::Normal,
            "/ opened a filter on an unseen table"
        );
        apply(&mut app, Action::SortCycle, ".");
        apply(&mut app, Action::SortReverse, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].sort_col, 0);
        apply(&mut app, Action::Bottom, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(0));

        // The palette offers no filter/sort here, and its commands are inert.
        let (entries, _) = crate::event::palette_state(&mut app);
        assert!(!entries.iter().any(|e| e.label == "Filter rows"));
        apply(&mut app, Action::BeginPalette, ".");
        for c in "filter".chars() {
            apply(&mut app, Action::Input(c), ".");
        }
        apply(&mut app, Action::Accept, ".");
        assert_eq!(app.mode, Mode::Normal);
        assert!(matches!(app.view, View::Detail(Detail::Job(_))));
    }

    #[test]
    fn detail_scroll_is_bounded_so_paging_back_works_at_once() {
        let mut app = App::new(Theme::default());
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        app.view = View::Detail(Detail::Diff(DiffView {
            title: "t".into(),
            lines,
            loading: false,
        }));
        let _ = draw(&mut app, 100, 30);
        let max = app.detail_max;
        assert!(max > 0);
        for _ in 0..50 {
            apply(&mut app, Action::PageDown, ".");
        }
        assert_eq!(app.detail_scroll, max, "overscroll piled up");
        apply(&mut app, Action::PageUp, ".");
        assert_eq!(
            app.detail_scroll,
            max - 10,
            "PgUp after overscroll was dead"
        );
        apply(&mut app, Action::Bottom, ".");
        assert_eq!(app.detail_scroll, max);
        apply(&mut app, Action::Top, ".");
        assert_eq!(app.detail_scroll, 0);

        // Content that fits never scrolls at all.
        app.view = View::Detail(Detail::Diff(DiffView {
            title: "t".into(),
            lines: vec!["x".into(); 5],
            loading: false,
        }));
        let _ = draw(&mut app, 100, 30);
        apply(&mut app, Action::PageDown, ".");
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn palette_hides_filter_and_sort_on_chart_widgets() {
        let mut app = App::new(Theme::default());
        app.focus = WidgetKind::Cost;
        app.mode = Mode::Palette;
        let s = draw(&mut app, 200, 50);
        assert!(!s.contains("Filter rows"), "Cost has no rows to filter");
        assert!(!s.contains("Sort Cost"));

        app.focus = WidgetKind::Jobs;
        let s = draw(&mut app, 200, 60);
        assert!(s.contains("Filter rows"));
    }

    #[test]
    fn escape_in_expanded_clears_the_filter_first() {
        let mut app = three_jobs();
        apply(&mut app, Action::Expand, ".");
        app.ui_mut(WidgetKind::Jobs).filter = "alpha".into();
        app.refilter();
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Expanded(WidgetKind::Jobs)));
        assert_eq!(app.filter_of(WidgetKind::Jobs), "");
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }

    #[test]
    fn help_overlay_is_modal() {
        let mut app = three_jobs();
        apply(&mut app, Action::Help, ".");
        apply(&mut app, Action::Down, ".");
        apply(&mut app, Action::Drill, ".");
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(0));
        assert!(matches!(app.view, View::Dashboard));
        apply(&mut app, Action::Back, ".");
        assert!(!app.show_help);
    }

    #[test]
    fn unfocused_tables_show_no_selection_highlight() {
        let mut app = three_jobs();
        app.focus = WidgetKind::Worktrees;
        let mut term = Terminal::new(TestBackend::new(200, 60)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let reversed = buf
            .content
            .iter()
            .any(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED));
        assert!(!reversed, "an unfocused table drew a reversed cursor row");
        // Its real selection is untouched.
        assert_eq!(app.ui[&WidgetKind::Jobs].table.selected(), Some(0));
    }

    #[test]
    fn tools_cursor_is_visible_and_reaches_the_last_row() {
        let mut app = App::new(Theme::default());
        app.data.lock().unwrap().tools = (0..40)
            .map(|i| crate::collect::tools::ToolStat {
                name: format!("Tool{i:02}"),
                count: 100 - i,
            })
            .collect();
        app.focus = WidgetKind::Procs;
        app.sync();
        apply(&mut app, Action::Expand, ".");
        apply(&mut app, Action::Bottom, ".");
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = buffer_text(&buf);
        assert!(text.contains("Tool39"), "last tool must scroll into view");
        let row_of_last = (0..30u16)
            .find(|&y| {
                (0..120u16)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .contains("Tool39")
            })
            .unwrap();
        assert!(
            buf[(5, row_of_last)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "selected tool row must be highlighted"
        );
    }
}
