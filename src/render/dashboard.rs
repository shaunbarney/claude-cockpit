//! Compose the screen for the current view: header, widget grid, footer, overlays.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Detail, Mode, View};
use crate::collect::usage::DayUsage;
use crate::layout::{band, cols_for, place, place_with, widgets_for, Band, FrameRects};
use crate::render::widgets::{self, TableCtx};
use crate::render::{footer, header, overlay};
use crate::rows::TableRow;
use crate::theme::Theme;
use crate::widget::WidgetKind;

/// Rows a chartless widget wants, so the grid can weigh it against the tables.
const CHART_ROWS: usize = 10;

pub fn render(f: &mut Frame, app: &mut App) {
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
fn flash_for<T: TableRow>(
    all: &[T],
    vis: &[usize],
    kind: WidgetKind,
    app: &App,
    theme: &Theme,
) -> Vec<Option<Style>> {
    vis.iter()
        .filter_map(|&i| all.get(i))
        .map(|r| {
            let key = App::flash_key(kind, &r.key());
            theme.flash_style(app.flashes.intensity(&key))
        })
        .collect()
}

/// Rows a widget will actually draw — what the grid weighs it by.
fn content_rows(app: &App, kind: WidgetKind) -> usize {
    match kind {
        WidgetKind::Cost | WidgetKind::Activity => CHART_ROWS,
        _ => app.visible_count(kind).max(1),
    }
}

fn draw_content(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let now = chrono::Utc::now().timestamp();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    if band(area) == Band::TooSmall {
        let p = Paragraph::new("Resize terminal (need >= 70x18)").alignment(Alignment::Center);
        let v = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .flex(Flex::Center)
        .split(area);
        f.render_widget(p, v[1]);
        app.rects = FrameRects::default();
        return;
    }

    if draw_detail(f, app, area, now, &today) {
        return;
    }

    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    let (head, body, foot) = (outer[0], outer[1], outer[2]);
    let b = band(body);

    let (kinds, cols) = match app.view {
        View::Expanded(k) => (vec![k], 1usize),
        _ => (widgets_for(b), cols_for(b)),
    };

    let placed = if matches!(app.view, View::Expanded(_)) {
        place(body, &kinds, cols)
    } else {
        place_with(body, &kinds, cols, |k| content_rows(app, k))
    };
    app.rects = FrameRects {
        widgets: placed.clone(),
        table_inner: None,
        table_offset: 0,
    };

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

    for (kind, rect) in &placed {
        draw_widget(
            f,
            app,
            *kind,
            *rect,
            &theme,
            b,
            focus,
            now,
            &today,
            &WidgetData {
                wts: &wts,
                loc: &loc,
                jobs: &jobs,
                usage: usage.as_ref(),
                containers: &containers,
                endpoints: &endpoints,
                tools: &tools,
                skills: &skills,
            },
        );
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

/// Remember where the focused table is, for mouse row hit-testing.
fn note_table_rect(app: &mut App, kind: WidgetKind, rect: Rect, offset: usize) {
    if app.focus != kind {
        return;
    }
    app.rects.table_inner = Some(
        ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .inner(rect),
    );
    // Fix-7 note: table_offset reflects the PREVIOUS frame's offset (set here,
    // before render_stateful_widget advances it), so a click in the same frame
    // as a scroll resolves against last frame's offset and self-corrects next
    // draw. This is intentional — do not "fix" row_at to compensate.
    app.rects.table_offset = offset;
}

#[allow(clippy::too_many_arguments)]
fn draw_widget(
    f: &mut Frame,
    app: &mut App,
    kind: WidgetKind,
    rect: Rect,
    theme: &Theme,
    b: Band,
    focus: WidgetKind,
    now: i64,
    today: &str,
    d: &WidgetData<'_>,
) {
    let focused = kind == focus;
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
            let st = app.ui.entry(kind).or_default();
            widgets::worktrees::render(f, rect, &rows, ctx!(&flash), &mut st.table);
            let off = st.table.offset();
            note_table_rect(app, kind, rect, off);
        }
        WidgetKind::Jobs => {
            let rows = pick(d.jobs, &vis);
            let flash = flash_for(d.jobs, &vis, kind, app, theme);
            let st = app.ui.entry(kind).or_default();
            widgets::jobs::render(f, rect, &rows, ctx!(&flash), &mut st.table, now);
            let off = st.table.offset();
            note_table_rect(app, kind, rect, off);
        }
        WidgetKind::Docker => {
            let rows = pick(d.containers, &vis);
            let flash = flash_for(d.containers, &vis, kind, app, theme);
            let st = app.ui.entry(kind).or_default();
            widgets::docker::render(f, rect, &rows, ctx!(&flash), &mut st.table);
            let off = st.table.offset();
            note_table_rect(app, kind, rect, off);
        }
        WidgetKind::Ports => {
            let rows = pick(d.endpoints, &vis);
            let flash = flash_for(d.endpoints, &vis, kind, app, theme);
            let st = app.ui.entry(kind).or_default();
            widgets::ports::render(f, rect, &rows, ctx!(&flash), &mut st.table);
            let off = st.table.offset();
            note_table_rect(app, kind, rect, off);
        }
        WidgetKind::Repo => {
            let rows = pick(d.skills, &vis);
            let flash = flash_for(d.skills, &vis, kind, app, theme);
            let st = app.ui.entry(kind).or_default();
            widgets::skills::render(f, rect, &rows, ctx!(&flash), &mut st.table);
            let off = st.table.offset();
            note_table_rect(app, kind, rect, off);
        }
        WidgetKind::Code => {
            let rows = pick(d.loc, &vis);
            widgets::code::render(f, rect, &rows, theme, focused);
        }
        WidgetKind::Procs => {
            let rows = pick(d.tools, &vis);
            widgets::tools::render(f, rect, &rows, theme, focused, b);
        }
        WidgetKind::Cost => {
            widgets::cost::render(f, rect, d.usage, theme, focused, b, today);
        }
        WidgetKind::Activity => {
            widgets::activity::render(f, rect, d.usage, theme, focused, b);
        }
    }
}

/// Render a detail screen if one is open. Returns true when it drew.
fn draw_detail(f: &mut Frame, app: &mut App, area: Rect, now: i64, today: &str) -> bool {
    enum Route {
        Job(usize),
        Worktree,
        Diff,
        Container(usize),
        Cost,
        CostModel(usize),
        Activity,
        Code,
        Ports(usize),
        Repo(usize),
        None,
    }
    let route = match &app.view {
        View::Detail(Detail::Job(i)) => Route::Job(*i),
        View::Detail(Detail::Worktree) => Route::Worktree,
        View::Detail(Detail::Diff(_)) => Route::Diff,
        View::Detail(Detail::Container(i)) => Route::Container(*i),
        View::Detail(Detail::Cost) => Route::Cost,
        View::Detail(Detail::CostModel(i)) => Route::CostModel(*i),
        View::Detail(Detail::Activity) => Route::Activity,
        View::Detail(Detail::Code) => Route::Code,
        View::Detail(Detail::Ports(i)) => Route::Ports(*i),
        View::Detail(Detail::Repo(i)) => Route::Repo(*i),
        _ => Route::None,
    };
    if matches!(route, Route::None) {
        return false;
    }

    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let (body, foot) = (outer[0], outer[1]);
    let theme = app.theme.clone();
    let gone = |f: &mut Frame, what: &str| {
        f.render_widget(
            Paragraph::new(format!("{what} is no longer present — Esc to go back"))
                .style(theme.dim_style()),
            body,
        );
    };

    match route {
        Route::Job(idx) => {
            let job_opt = { app.data.lock().unwrap().jobs.get(idx).cloned() };
            match job_opt {
                Some(job) => {
                    let events = crate::collect::jobs::read_timeline(&job.id, 200);
                    crate::render::detail::job::render(
                        f,
                        body,
                        &job,
                        &events,
                        &theme,
                        now,
                        app.detail_scroll,
                    );
                }
                None => gone(f, "that job"),
            }
        }
        Route::Worktree => {
            let jobs = { app.data.lock().unwrap().jobs.clone() };
            if let Some(detail) = app.wt_detail.clone() {
                crate::render::detail::worktree::render(
                    f,
                    body,
                    &detail,
                    &jobs,
                    &theme,
                    &mut app.detail_table,
                    now,
                );
            } else {
                f.render_widget(Paragraph::new("Loading…").style(theme.dim_style()), body);
            }
        }
        Route::Diff => {
            let scroll = app.detail_scroll;
            if let View::Detail(Detail::Diff(dv)) = &app.view {
                crate::render::detail::diff::render(f, body, dv, &theme, scroll);
            }
        }
        Route::Container(idx) => {
            let name = app
                .data
                .lock()
                .unwrap()
                .containers
                .get(idx)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            crate::render::detail::container::render(
                f,
                body,
                &name,
                &app.container_logs,
                &theme,
                app.detail_scroll,
            );
        }
        Route::Cost => {
            let usage = { app.data.lock().unwrap().usage.clone() };
            match usage {
                Some(u) => crate::render::detail::cost::render(
                    f,
                    body,
                    &u,
                    &theme,
                    &mut app.detail_table,
                    today,
                ),
                None => gone(f, "usage data"),
            }
        }
        Route::CostModel(idx) => {
            let usage = { app.data.lock().unwrap().usage.clone() };
            match usage.as_ref().and_then(|u| u.by_model.get(idx)) {
                Some(m) => {
                    let empty: Vec<DayUsage> = Vec::new();
                    let days = usage
                        .as_ref()
                        .and_then(|u| u.by_model_day.get(&m.model))
                        .map(|v| v.as_slice())
                        .unwrap_or(&empty);
                    crate::render::detail::cost::render_model(
                        f,
                        body,
                        m,
                        days,
                        &theme,
                        app.detail_scroll,
                    );
                }
                None => gone(f, "that model"),
            }
        }
        Route::Activity => {
            let (usage, activity) = {
                let d = app.data.lock().unwrap();
                (d.usage.clone(), d.activity.clone())
            };
            crate::render::detail::activity::render(
                f,
                body,
                usage.as_ref(),
                &activity,
                &theme,
                app.detail_scroll,
            );
        }
        Route::Code => {
            let loc = { app.data.lock().unwrap().loc.clone() };
            crate::render::detail::code::render(f, body, &loc, &theme);
        }
        Route::Ports(idx) => {
            let ep = { app.data.lock().unwrap().endpoints.get(idx).cloned() };
            match ep {
                Some(ep) => crate::render::detail::ports::render(f, body, &ep, &theme),
                None => gone(f, "that endpoint"),
            }
        }
        Route::Repo(idx) => {
            let skill = { app.data.lock().unwrap().skills.get(idx).cloned() };
            crate::render::detail::skills::render(
                f,
                body,
                skill.as_ref(),
                &theme,
                app.detail_scroll,
            );
        }
        Route::None => unreachable!("handled above"),
    }

    footer::render(f, foot, app);
    app.rects = FrameRects::default();
    true
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
}
