//! Route the open detail view to its renderer.
//!
//! Row-backed details carry a stable row key (job id, container id, …) and are
//! resolved against the live data here on every frame, so a refresh that
//! reorders rows can never swap the item on screen; a row that has gone says
//! so. This is also where each scrolling detail reports its maximum scroll, so
//! input can clamp against it.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Detail, TimelineCache, View};
use crate::collect::jobs::JobEvent;
use crate::collect::usage::DayUsage;
use crate::render::footer;
use crate::rows::TableRow;

/// Timeline events kept for the job detail.
const TIMELINE_TAIL: usize = 200;

/// Largest scroll that still shows content: `content` lines in a `viewport`.
pub fn max_scroll(content: usize, viewport: u16) -> u16 {
    content
        .saturating_sub(viewport as usize)
        .min(u16::MAX as usize) as u16
}

/// Record the detail's scroll bound and clamp the current scroll into it.
fn bound(app: &mut App, content: usize, viewport: u16) -> u16 {
    app.detail_max = max_scroll(content, viewport);
    app.detail_scroll = app.detail_scroll.min(app.detail_max);
    app.detail_scroll
}

/// Lines `text` occupies when word-wrapped to `width` — an estimate (it wraps
/// by characters), close enough to bound a scroll.
fn wrapped_lines(text: &str, width: u16) -> usize {
    let w = (width as usize).max(1);
    text.lines()
        .map(|l| l.chars().count().div_ceil(w).max(1))
        .sum()
}

/// A job's timeline, re-parsed only when the file's mtime or length changes.
fn timeline(app: &mut App, job_id: &str) -> Vec<JobEvent> {
    let path =
        crate::util::claude_home().map(|h| h.join("jobs").join(job_id).join("timeline.jsonl"));
    let stamp = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| (m.modified().ok(), m.len()))
        .unwrap_or((None, 0));
    if let Some(c) = &app.timeline {
        if c.job_id == job_id && c.stamp == stamp {
            return c.events.clone();
        }
    }
    let events = crate::collect::jobs::read_timeline(job_id, TIMELINE_TAIL);
    app.timeline = Some(TimelineCache {
        job_id: job_id.to_string(),
        stamp,
        events: events.clone(),
    });
    events
}

enum Route {
    Job(String),
    Worktree,
    Diff,
    Container(String),
    Cost,
    CostModel(String),
    Activity,
    Code,
    Ports(String),
    Repo(String),
}

fn route(view: &View) -> Option<Route> {
    let View::Detail(d) = view else {
        return None;
    };
    Some(match d {
        Detail::Job(k) => Route::Job(k.clone()),
        Detail::Worktree => Route::Worktree,
        Detail::Diff(_) => Route::Diff,
        Detail::Container(k) => Route::Container(k.clone()),
        Detail::Cost => Route::Cost,
        Detail::CostModel(k) => Route::CostModel(k.clone()),
        Detail::Activity => Route::Activity,
        Detail::Code => Route::Code,
        Detail::Ports(k) => Route::Ports(k.clone()),
        Detail::Repo(k) => Route::Repo(k.clone()),
    })
}

fn gone(f: &mut Frame, app: &App, body: Rect, what: &str) {
    f.render_widget(
        Paragraph::new(format!("{what} is no longer present — Esc to go back"))
            .style(app.theme.dim_style()),
        body,
    );
}

fn loading(f: &mut Frame, app: &App, body: Rect, what: &str) {
    let msg = format!("{} loading {what}…", app.clock.spinner());
    f.render_widget(Paragraph::new(msg).style(app.theme.dim_style()), body);
}

/// Render a detail screen if one is open. Returns true when it drew.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect, now: i64, today: &str) -> bool {
    let Some(r) = route(&app.view) else {
        return false;
    };
    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let (body, foot) = (outer[0], outer[1]);
    app.detail_max = 0;

    match r {
        Route::Job(id) => draw_job(f, app, body, &id, now),
        Route::Worktree => draw_worktree(f, app, body, now),
        Route::Diff => draw_diff(f, app, body),
        Route::Container(id) => draw_container(f, app, body, &id),
        Route::Cost => {
            let usage = { app.data.lock().unwrap().usage.clone() };
            let theme = app.theme.clone();
            match usage {
                Some(u) => super::cost::render(f, body, &u, &theme, &mut app.detail_table, today),
                None => gone(f, app, body, "usage data"),
            }
        }
        Route::CostModel(model) => draw_cost_model(f, app, body, &model),
        Route::Activity => {
            let (usage, activity) = {
                let d = app.data.lock().unwrap();
                (d.usage.clone(), d.activity.clone())
            };
            // Header block is 5 rows, plus the body's own borders.
            let scroll = bound(app, activity.len(), body.height.saturating_sub(7));
            let theme = app.theme.clone();
            super::activity::render(f, body, usage.as_ref(), &activity, &theme, scroll);
        }
        Route::Code => {
            let loc = { app.data.lock().unwrap().loc.clone() };
            super::code::render(f, body, &loc, &app.theme);
        }
        Route::Ports(key) => {
            let ep = {
                let d = app.data.lock().unwrap();
                d.endpoints.iter().find(|e| e.key() == key).cloned()
            };
            match ep {
                Some(ep) => super::ports::render(f, body, &ep, &app.theme),
                None => gone(f, app, body, "that endpoint"),
            }
        }
        Route::Repo(key) => {
            let skill = {
                let d = app.data.lock().unwrap();
                d.skills.iter().find(|s| s.key() == key).cloned()
            };
            let lines = skill
                .as_ref()
                .map(|s| wrapped_lines(&s.description, body.width.saturating_sub(2)))
                .unwrap_or(0);
            // Header block is 6 rows, plus the description's borders.
            let scroll = bound(app, lines, body.height.saturating_sub(8));
            let theme = app.theme.clone();
            super::skills::render(f, body, skill.as_ref(), &theme, scroll);
        }
    }

    footer::render(f, foot, app);
    true
}

fn draw_job(f: &mut Frame, app: &mut App, body: Rect, id: &str, now: i64) {
    let job = {
        let d = app.data.lock().unwrap();
        d.jobs.iter().find(|j| j.id == id).cloned()
    };
    let Some(job) = job else {
        return gone(f, app, body, "that job");
    };
    let events = timeline(app, id);
    let viewport = body
        .height
        .saturating_sub(super::job::header_height(&job) + 2);
    let scroll = bound(app, events.len(), viewport);
    let theme = app.theme.clone();
    super::job::render(f, body, &job, &events, &theme, now, scroll);
}

fn draw_worktree(f: &mut Frame, app: &mut App, body: Rect, now: i64) {
    let jobs = { app.data.lock().unwrap().jobs.clone() };
    let Some(detail) = app.wt_detail.clone() else {
        return loading(f, app, body, "worktree");
    };
    let theme = app.theme.clone();
    let files =
        super::worktree::render(f, body, &detail, &jobs, &theme, &mut app.detail_table, now);
    app.rects.detail_table = Some(files);
}

fn draw_diff(f: &mut Frame, app: &mut App, body: Rect) {
    let (n, is_loading) = match &app.view {
        View::Detail(Detail::Diff(dv)) => (dv.lines.len(), dv.loading),
        _ => return,
    };
    if is_loading {
        return loading(f, app, body, "diff");
    }
    let scroll = bound(app, n, body.height.saturating_sub(2));
    let theme = app.theme.clone();
    if let View::Detail(Detail::Diff(dv)) = &app.view {
        super::diff::render(f, body, dv, &theme, scroll);
    }
}

fn draw_container(f: &mut Frame, app: &mut App, body: Rect, id: &str) {
    let name = {
        let d = app.data.lock().unwrap();
        d.containers
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
    };
    let Some(name) = name else {
        return gone(f, app, body, "that container");
    };
    if app.loading {
        return loading(f, app, body, "logs");
    }
    let scroll = bound(app, app.container_logs.len(), body.height.saturating_sub(2));
    let theme = app.theme.clone();
    super::container::render(f, body, &name, &app.container_logs, &theme, scroll);
}

fn draw_cost_model(f: &mut Frame, app: &mut App, body: Rect, model: &str) {
    let usage = { app.data.lock().unwrap().usage.clone() };
    let Some(m) = usage
        .as_ref()
        .and_then(|u| u.by_model.iter().find(|m| m.model == model))
    else {
        return gone(f, app, body, "that model");
    };
    let empty: Vec<DayUsage> = Vec::new();
    let days = usage
        .as_ref()
        .and_then(|u| u.by_model_day.get(&m.model))
        .map(|v| v.as_slice())
        .unwrap_or(&empty);
    // Header block is 7 rows, plus the daily list's borders.
    let scroll = bound(app, days.len(), body.height.saturating_sub(9));
    let theme = app.theme.clone();
    super::cost::render_model(f, body, m, days, &theme, scroll);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_scroll_stops_at_the_last_page() {
        assert_eq!(max_scroll(5, 20), 0);
        assert_eq!(max_scroll(50, 20), 30);
        assert_eq!(max_scroll(usize::MAX, 0), u16::MAX);
    }

    #[test]
    fn wrapped_lines_counts_wraps_and_blank_lines() {
        assert_eq!(wrapped_lines("abcdef", 3), 2);
        assert_eq!(wrapped_lines("a\n\nb", 10), 3);
        assert_eq!(wrapped_lines("", 10), 0);
    }
}
