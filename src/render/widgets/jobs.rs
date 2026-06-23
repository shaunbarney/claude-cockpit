//! Jobs widget: renders a stateful ratatui Table of Claude Code background jobs.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::jobs::Job;
use crate::layout::Band;
use crate::theme::Theme;
use crate::util::{human_duration, worktree_label};

/// Truncate `s` to at most `max` chars, appending `…` if it was cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Render the Jobs table into `area`.
///
/// Columns (wide/medium): `● | Name | State | Tasks | Worktree | Age`
/// Compact band: drops `Tasks` and truncates Worktree column.
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    jobs: &[Job],
    theme: &Theme,
    focused: bool,
    band: Band,
    state: &mut TableState,
    now: i64,
) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Jobs ")
        .border_style(border_style);

    let compact = band == Band::Compact;

    let full_widths: &[Constraint] = &[
        Constraint::Length(2), // dot
        Constraint::Min(16),   // name
        Constraint::Length(9), // state
        Constraint::Length(8), // tasks
        Constraint::Min(20),   // worktree
        Constraint::Length(9), // age
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2), // dot
        Constraint::Min(14),   // name
        Constraint::Length(9), // state
        Constraint::Min(16),   // worktree (truncated)
        Constraint::Length(9), // age
    ];

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
            // Dot/state colour logic.
            let dot_color = if j.tempo == "active" {
                theme.accent
            } else if j.state == "working" {
                theme.ok
            } else if j.state == "blocked" {
                theme.warn
            } else {
                theme.dim
            };

            // Stuck row: blocked for more than 5 minutes.
            let stuck =
                j.state == "blocked" && j.updated_at.map(|u| now - u > 300).unwrap_or(false);

            let row_style = if stuck {
                Style::new().fg(theme.warn)
            } else {
                Style::new()
            };

            let dot_cell = Cell::from(Span::styled("●", Style::new().fg(dot_color)));
            let name_cell = Cell::from(Span::styled(
                j.name.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            let state_cell = Cell::from(Span::styled(j.state.clone(), Style::new().fg(dot_color)));

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
            let wt_str = if compact {
                truncate(&wt_full, 20)
            } else {
                truncate(&wt_full, 40)
            };
            let wt_cell = Cell::from(wt_str);

            // Age: "Xs ago" or dim dash.
            let age_cell = match j.updated_at {
                Some(u) => {
                    let secs = (now - u).max(0) as u64;
                    Cell::from(Span::styled(
                        format!("{} ago", human_duration(secs)),
                        theme.dim_style(),
                    ))
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

            Row::new(cells).style(row_style)
        })
        .collect();

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(2)
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
                &theme,
                true,
                crate::layout::Band::Wide,
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
