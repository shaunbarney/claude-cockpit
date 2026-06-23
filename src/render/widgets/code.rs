//! Code widget: renders the LOC table (language, files, code lines).

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;

use crate::collect::loc::LocRow;
use crate::theme::Theme;
use crate::util::thousands;

/// Render the Code (LOC) table into `area`.
pub fn render(f: &mut Frame, area: Rect, rows: &[LocRow], theme: &Theme, focused: bool) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Code ")
        .border_style(border_style);

    let widths: &[Constraint] = &[
        Constraint::Min(12),
        Constraint::Length(10),
        Constraint::Length(12),
    ];

    let header =
        Row::new(["Language", "Files", "Code"]).style(Style::new().add_modifier(Modifier::BOLD));

    let mut body: Vec<Row> = rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(Span::styled(
                    r.language.clone(),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    thousands(r.files as u64),
                    Style::new().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    thousands(r.code as u64),
                    Style::new().fg(Color::Cyan),
                )),
            ])
        })
        .collect();

    // TOTAL row in accent colour.
    let tot = crate::collect::loc::totals(rows);
    body.push(Row::new(vec![
        Cell::from(Span::styled(
            "TOTAL",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            thousands(tot.files as u64),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            thousands(tot.code as u64),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
    ]));

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(2)
        .block(block);

    f.render_widget(table, area);
}
