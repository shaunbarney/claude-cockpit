//! Worktrees widget: renders a stateful ratatui Table with per-row health colours.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::git::Worktree;
use crate::layout::Band;
use crate::theme::Theme;

/// A `+A/-D` churn span (green) or a dim em-dash when not shown.
fn churn(pair: (u32, u32), show: bool) -> Span<'static> {
    if !show {
        return Span::styled("—", Style::new().fg(Color::DarkGray));
    }
    Span::styled(format!("+{}/-{}", pair.0, pair.1), Style::new().fg(Color::Green))
}

/// Render the Worktrees table into `area`.
///
/// Columns adapt to `band`: `Compact` drops the two churn columns.
pub fn render(
    f: &mut Frame,
    area: Rect,
    rows: &[Worktree],
    theme: &Theme,
    focused: bool,
    band: Band,
    state: &mut TableState,
) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Worktrees ")
        .border_style(border_style);

    let compact = band == Band::Compact;

    // Column widths differ between compact and full.
    let full_widths: &[Constraint] = &[
        Constraint::Length(2),
        Constraint::Min(16),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(13),
        Constraint::Length(13),
        Constraint::Length(16),
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2),
        Constraint::Min(16),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(16),
    ];

    let widths = if compact { compact_widths } else { full_widths };

    let header = if compact {
        Row::new(["", "Worktree", "Ahead", "Dirty", "Age"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        Row::new(["", "Worktree", "Ahead", "Dirty", "Committed", "Uncommitted", "Age"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    };

    let body: Vec<Row> = rows
        .iter()
        .map(|r| {
            let (dot_color, ahead_span) = if r.ahead > 0 {
                (Color::Red, Span::styled(r.ahead.to_string(), Style::new().fg(Color::Red)))
            } else if r.dirty > 0 {
                (Color::Yellow, Span::styled("0", Style::new().fg(Color::DarkGray)))
            } else {
                (Color::Green, Span::styled("0", Style::new().fg(Color::DarkGray)))
            };

            let dirty_span = if r.dirty > 0 {
                Span::styled(r.dirty.to_string(), Style::new().fg(Color::Yellow))
            } else {
                Span::styled("0", Style::new().fg(Color::DarkGray))
            };

            let age_span = Span::styled(r.age.clone(), Style::new().fg(Color::DarkGray));
            let name_span =
                Span::styled(r.name.clone(), Style::new().add_modifier(Modifier::BOLD));

            if compact {
                Row::new(vec![
                    Cell::from(Span::styled("●", Style::new().fg(dot_color))),
                    Cell::from(name_span),
                    Cell::from(ahead_span),
                    Cell::from(dirty_span),
                    Cell::from(age_span),
                ])
            } else {
                Row::new(vec![
                    Cell::from(Span::styled("●", Style::new().fg(dot_color))),
                    Cell::from(name_span),
                    Cell::from(ahead_span),
                    Cell::from(dirty_span),
                    Cell::from(churn(r.committed, r.ahead > 0)),
                    Cell::from(churn(r.uncommitted, r.dirty > 0)),
                    Cell::from(age_span),
                ])
            }
        })
        .collect();

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(2)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, state);
}
