//! Compose the widget grid for the current size band; dispatch to widget renderers.

use ratatui::layout::{Alignment, Constraint, Flex, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Detail, View};
use crate::layout::{band, cols_for, place, widgets_for, Band, FrameRects};
use crate::render::widgets;
use crate::widget::WidgetKind;

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let now = chrono::Utc::now().timestamp();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    if band(area) == Band::TooSmall {
        let p = Paragraph::new("Resize terminal (need >= 70x18)")
            .alignment(Alignment::Center);
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

    // Detail routing: classify the view without holding a borrow across mutable calls.
    enum DetailRoute { Job(usize), Worktree, Diff, Container(usize), None }
    let route = match &app.view {
        View::Detail(Detail::Job(i)) => DetailRoute::Job(*i),
        View::Detail(Detail::Worktree(_)) => DetailRoute::Worktree,
        View::Detail(Detail::Diff(_)) => DetailRoute::Diff,
        View::Detail(Detail::Container(i)) => DetailRoute::Container(*i),
        _ => DetailRoute::None,
    };

    match route {
        DetailRoute::Job(idx) => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let job_opt = { app.data.lock().unwrap().jobs.get(idx).cloned() };
            match job_opt {
                Some(job) => {
                    let events = crate::collect::jobs::read_timeline(&job.id, 200);
                    crate::render::detail::job::render(
                        f, outer[0], &job, &events, &app.theme, now, app.detail_scroll,
                    );
                }
                None => {
                    let p = Paragraph::new("job no longer present — Esc to go back")
                        .style(app.theme.dim_style());
                    f.render_widget(p, outer[0]);
                }
            }
            let line = Line::from("  Esc back · ↑/↓ scroll · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
        DetailRoute::Worktree => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let theme = app.theme.clone();
            let jobs = { app.data.lock().unwrap().jobs.clone() };
            if let Some(detail) = app.wt_detail.clone() {
                crate::render::detail::worktree::render(
                    f, outer[0], &detail, &jobs, &theme, &mut app.detail_table, now,
                );
            } else {
                f.render_widget(
                    Paragraph::new("Loading…").style(app.theme.dim_style()),
                    outer[0],
                );
            }
            f.render_widget(
                Paragraph::new(Line::from(
                    "  Esc back · ↑/↓ select · Enter diff · q quit",
                ))
                .style(theme.dim_style()),
                outer[1],
            );
            app.rects = FrameRects::default();
            return;
        }
        DetailRoute::Diff => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let theme = app.theme.clone();
            let scroll = app.detail_scroll;
            if let View::Detail(Detail::Diff(dv)) = &app.view {
                crate::render::detail::diff::render(f, outer[0], dv, &theme, scroll);
            }
            f.render_widget(
                Paragraph::new(Line::from("  Esc back · ↑/↓ scroll · q quit"))
                    .style(theme.dim_style()),
                outer[1],
            );
            app.rects = FrameRects::default();
            return;
        }
        DetailRoute::Container(idx) => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
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
                outer[0],
                &name,
                &app.container_logs,
                &app.theme,
                app.detail_scroll,
            );
            let line = Line::from("  Esc back · ↑/↓ scroll · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
        DetailRoute::None => {}
    }

    let outer =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let body = outer[0];
    let b = band(body);

    let (kinds, cols) = match app.view {
        View::Expanded(k) => (vec![k], 1usize),
        _ => (widgets_for(b), cols_for(b)),
    };

    let placed = place(body, &kinds, cols);
    app.rects = FrameRects {
        widgets: placed.clone(),
        table_inner: None,
        table_offset: 0,
    };

    let theme = app.theme.clone();
    let focus = app.focus;

    let data = app.data.lock().unwrap();
    let wts = data.worktrees.clone();
    let loc = data.loc.clone();
    let jobs = data.jobs.clone();
    let usage = data.usage.clone();
    let activity = data.activity.clone();
    let containers = data.containers.clone();
    let endpoints = data.endpoints.clone();
    let procs = data.procs.clone();
    drop(data);

    for (kind, rect) in &placed {
        let focused = *kind == focus;
        match kind {
            WidgetKind::Worktrees => {
                let offset = {
                    let st = app.ui.entry(WidgetKind::Worktrees).or_default();
                    widgets::worktrees::render(f, *rect, &wts, &theme, focused, b, &mut st.table);
                    st.table.offset()
                };
                if focused {
                    app.rects.table_inner = Some(Block::default().borders(Borders::ALL).inner(*rect));
                    // Fix-7 note: table_offset reflects the PREVIOUS frame's offset (set here,
                    // before render_stateful_widget advances it), so a click in the same frame
                    // as a scroll resolves against last frame's offset and self-corrects next draw.
                    // This is intentional — do not "fix" row_at to compensate.
                    app.rects.table_offset = offset;
                }
            }
            WidgetKind::Code => {
                widgets::code::render(f, *rect, &loc, &theme, focused);
            }
            WidgetKind::Jobs => {
                let offset = {
                    let st = app.ui.entry(WidgetKind::Jobs).or_default();
                    widgets::jobs::render(f, *rect, &jobs, &theme, focused, b, &mut st.table, now);
                    st.table.offset()
                };
                if focused {
                    app.rects.table_inner =
                        Some(Block::default().borders(Borders::ALL).inner(*rect));
                    app.rects.table_offset = offset;
                }
            }
            WidgetKind::Cost => {
                widgets::cost::render(f, *rect, usage.as_ref(), &theme, focused, b, &today);
            }
            WidgetKind::Activity => {
                widgets::activity::render(
                    f, *rect, usage.as_ref(), &activity, &theme, focused, b,
                );
            }
            WidgetKind::Docker => {
                let offset = {
                    let st = app.ui.entry(WidgetKind::Docker).or_default();
                    widgets::docker::render(f, *rect, &containers, &theme, focused, b, &mut st.table);
                    st.table.offset()
                };
                if focused {
                    app.rects.table_inner =
                        Some(Block::default().borders(Borders::ALL).inner(*rect));
                    app.rects.table_offset = offset;
                }
            }
            WidgetKind::Ports => {
                let offset = {
                    let st = app.ui.entry(WidgetKind::Ports).or_default();
                    widgets::ports::render(f, *rect, &endpoints, &theme, focused, b, &mut st.table);
                    st.table.offset()
                };
                if focused {
                    app.rects.table_inner =
                        Some(Block::default().borders(Borders::ALL).inner(*rect));
                    app.rects.table_offset = offset;
                }
            }
            WidgetKind::Procs => {
                let offset = {
                    let st = app.ui.entry(WidgetKind::Procs).or_default();
                    widgets::procs::render(f, *rect, &procs, &theme, focused, b, &mut st.table);
                    st.table.offset()
                };
                if focused {
                    app.rects.table_inner =
                        Some(Block::default().borders(Borders::ALL).inner(*rect));
                    app.rects.table_offset = offset;
                }
            }
            other => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", other.title()));
                let p =
                    Paragraph::new(Line::from("…")).block(block).style(theme.dim_style());
                f.render_widget(p, *rect);
            }
        }
    }

    footer(f, outer[1], app);
}

fn footer(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let n = { app.data.lock().unwrap().worktrees.len() };
    let line = Line::from(format!(
        "  {} worktrees · Tab focus · e expand · r refresh · q quit",
        n
    ));
    f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn wide_shows_widget_titles() {
        let mut app = App::new(Theme::default());
        let mut term = Terminal::new(TestBackend::new(200, 60)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Worktrees"));
        assert!(s.contains("Code"));
    }

    #[test]
    fn too_small_shows_guard() {
        let mut app = App::new(Theme::default());
        let mut term = Terminal::new(TestBackend::new(50, 10)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let s = buffer_text(term.backend().buffer()).to_lowercase();
        assert!(s.contains("resize"));
    }
}
