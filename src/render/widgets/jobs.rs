//! Jobs widget: renders a stateful ratatui Table of Claude Code background jobs.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use super::TableCtx;
use crate::collect::jobs::Job;
use crate::theme::{Level, Theme};
use crate::util::{human_duration, worktree_label};

/// Truncate `s` to at most `max` chars, appending `…` if it was cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// A working job silent for longer than this has probably stalled.
const STALLED_SECS: i64 = 10 * 60;
/// A job blocked on input for longer than this has been walked away from.
const ABANDONED_SECS: i64 = 24 * 60 * 60;
/// Worktree column width; the name column takes everything left over.
const WT_W: u16 = 18;
const WT_W_COMPACT: u16 = 12;
/// Gap between columns — kept to one so the name keeps the space.
const SPACING: u16 = 1;

/// What the State column says, and at what level.
///
/// A blocked job is waiting for *you* — that is its own state, not a fault.
/// What deserves a warning is a job that claims to be working but has gone
/// quiet, and one left blocked so long it has been forgotten.
pub fn status(j: &Job, now: i64) -> (String, Level) {
    let silent = j.updated_at.map(|u| now - u).unwrap_or(0);
    match j.state.as_str() {
        "failed" | "error" => (j.state.clone(), Level::Err),
        "done" | "completed" | "cancelled" => (j.state.clone(), Level::Idle),
        "blocked" if silent > ABANDONED_SECS => ("abandoned".into(), Level::Idle),
        "blocked" => ("needs input".into(), Level::Warn),
        "working" if silent > STALLED_SECS => ("stalled".into(), Level::Warn),
        _ if j.tempo == "active" && silent > STALLED_SECS => ("stalled".into(), Level::Warn),
        _ if j.tempo == "active" => (j.state.clone(), Level::Active),
        "working" => (j.state.clone(), Level::Ok),
        _ => (j.state.clone(), Level::Idle),
    }
}

fn state_cell(label: String, level: Level, theme: &Theme) -> Cell<'static> {
    Cell::from(Span::styled(
        label,
        Style::new().fg(theme.level_color(level)),
    ))
}

/// Render the Jobs table into `area`.
///
/// Columns (wide/medium): `● | Name | State | Tasks | Worktree | Age`
/// Compact band: drops `Tasks` and truncates Worktree column.
pub fn render(
    f: &mut Frame,
    area: Rect,
    jobs: &[Job],
    ctx: TableCtx<'_>,
    state: &mut TableState,
    now: i64,
) {
    let TableCtx {
        theme,
        focused,
        band,
        flash,
        empty,
    } = ctx;
    if jobs.is_empty() {
        let ctx = TableCtx {
            theme,
            focused,
            band,
            flash,
            empty,
        };
        return super::empty_state(f, area, " Jobs ", empty, &ctx);
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Jobs ")
        .border_style(theme.border_style(focused));

    // The name is what identifies a job, so it gets every spare column; the
    // worktree is fixed-width and truncated.
    let full_widths: &[Constraint] = &[
        Constraint::Length(2),    // dot
        Constraint::Min(26),      // name: drop Tasks before squeezing this
        Constraint::Length(11),   // state ("needs input")
        Constraint::Length(5),    // tasks
        Constraint::Length(WT_W), // worktree
        Constraint::Length(5),    // age
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2),            // dot
        Constraint::Min(12),              // name
        Constraint::Length(11),           // state
        Constraint::Length(WT_W_COMPACT), // worktree (truncated)
        Constraint::Length(5),            // age
    ];

    let compact = super::tight(area, band, full_widths, SPACING);
    let widths = if compact { compact_widths } else { full_widths };

    let header = if compact {
        Row::new(["", "Name", "State", "Worktree", "Age"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        Row::new(["", "Name", "State", "Tasks", "Worktree", "Age"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    };

    let body: Vec<Row> = jobs
        .iter()
        .map(|j| {
            let (label, level) = status(j, now);
            let dot_cell = Cell::from(theme.level_span(level));
            let name_cell = Cell::from(Span::styled(
                j.name.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            let state_cell = state_cell(label, level, theme);

            // Tasks: "N" or "N+Q" dim when zero.
            let tasks_text = if j.queued > 0 {
                format!("{}+{}", j.tasks, j.queued)
            } else {
                j.tasks.to_string()
            };
            let tasks_cell = if j.tasks == 0 && j.queued == 0 {
                Cell::from(Span::styled(tasks_text, theme.dim_style()))
            } else {
                Cell::from(tasks_text)
            };

            // Worktree: branch (or folder name), truncated to fit.
            let wt_full = worktree_label(j.worktree_branch.as_deref(), j.worktree_path.as_deref());
            let wt_str = truncate(
                &wt_full,
                usize::from(if compact { WT_W_COMPACT } else { WT_W }),
            );
            let wt_cell = Cell::from(wt_str);

            // Age since last update ("22m"), or a dim dash.
            let age_cell = match j.updated_at {
                Some(u) => {
                    let secs = (now - u).max(0) as u64;
                    Cell::from(Span::styled(human_duration(secs), theme.dim_style()))
                }
                None => Cell::from(Span::styled("—", theme.dim_style())),
            };

            let cells: Vec<Cell> = if compact {
                vec![dot_cell, name_cell, state_cell, wt_cell, age_cell]
            } else {
                vec![
                    dot_cell, name_cell, state_cell, tasks_cell, wt_cell, age_cell,
                ]
            };

            Row::new(cells)
        })
        .collect();
    let body = super::flashed(body, flash);

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(SPACING)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::jobs::Job;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, layout::Rect, widgets::TableState, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn job(name: &str, state: &str) -> Job {
        Job {
            id: "i".into(),
            name: name.into(),
            state: state.into(),
            tempo: "active".into(),
            intent: "doing things".into(),
            tasks: 1,
            queued: 0,
            cwd: String::new(),
            worktree_path: None,
            worktree_branch: Some("feat/demo".into()),
            created_at: Some(0),
            updated_at: Some(100),
        }
    }

    #[test]
    fn status_names_what_needs_attention() {
        let mut j = job("a", "blocked");
        j.updated_at = Some(0);
        assert_eq!(status(&j, 60), ("needs input".into(), Level::Warn));
        assert_eq!(status(&j, ABANDONED_SECS + 1).0, "abandoned");
        let mut w = job("b", "working");
        w.updated_at = Some(0);
        assert_eq!(status(&w, 30).1, Level::Active);
        assert_eq!(
            status(&w, STALLED_SECS + 1),
            ("stalled".into(), Level::Warn)
        );
        let f = job("c", "failed");
        assert_eq!(status(&f, 0).1, Level::Err);
    }

    #[test]
    fn renders_job_row() {
        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let mut st = TableState::default();
        let theme = Theme::default();
        let jobs = vec![job("MyAgent", "working")];
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 12,
                },
                &jobs,
                super::TableCtx {
                    theme: &theme,
                    focused: true,
                    band: crate::layout::Band::Wide,
                    flash: &[],
                    empty: "nothing here",
                },
                &mut st,
                200,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("MyAgent"), "expected 'MyAgent' in buffer");
        assert!(s.contains("Jobs"), "expected 'Jobs' block title in buffer");
        assert!(
            s.contains("Worktree"),
            "expected 'Worktree' header in buffer"
        );
        assert!(
            s.contains("feat/demo"),
            "expected worktree branch value in buffer"
        );
    }
}
