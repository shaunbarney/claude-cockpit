//! Full-screen Code detail: every language with files, code lines, and % of total.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::collect::loc::{totals, LocRow};
use crate::theme::Theme;
use crate::util::thousands;

/// Whole percentages that sum to exactly 100 (largest-remainder method).
///
/// Plain truncation dropped every row's fraction, so a long tail of small
/// languages left the column summing to well under the TOTAL row's 100%.
pub fn percentages(values: &[u64]) -> Vec<u64> {
    let total: u64 = values.iter().sum();
    if total == 0 {
        return vec![0; values.len()];
    }
    let mut out: Vec<u64> = values.iter().map(|v| v * 100 / total).collect();
    let mut left = 100 - out.iter().sum::<u64>();
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(values[i] * 100 % total));
    for &i in order.iter().cycle().take(values.len().max(1) * 2) {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

pub fn render(f: &mut Frame, area: Rect, loc: &[LocRow], theme: &Theme) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    let sums = totals(loc);
    let header = Line::from(vec![
        Span::styled("files ", theme.dim_style()),
        Span::raw(thousands(sums.files as u64)),
        Span::styled("   lines ", theme.dim_style()),
        Span::styled(thousands(sums.lines as u64), Style::new().fg(theme.accent)),
        Span::styled("   languages ", theme.dim_style()),
        Span::raw(loc.len().to_string()),
    ]);
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(" Code ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(header).block(hblock), chunks[0]);

    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(" Languages ")
        .title_style(theme.title());
    if loc.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no code counted", theme.dim_style())).block(bblock),
            chunks[1],
        );
        return;
    }

    let pcts = percentages(&loc.iter().map(|r| r.lines as u64).collect::<Vec<_>>());
    let mut rows: Vec<Row> = loc
        .iter()
        .zip(&pcts)
        .map(|(r, pct)| {
            Row::new(vec![
                Cell::from(Span::styled(
                    r.language.clone(),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Cell::from(thousands(r.files as u64)),
                Cell::from(thousands(r.lines as u64)),
                Cell::from(format!("{pct}%")),
            ])
        })
        .collect();
    rows.push(
        Row::new(vec![
            Cell::from(Span::styled(
                "TOTAL",
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Cell::from(thousands(sums.files as u64)),
            Cell::from(thousands(sums.lines as u64)),
            Cell::from("100%".to_string()),
        ])
        .style(Style::new().add_modifier(Modifier::BOLD)),
    );

    let widths = [
        Constraint::Min(12),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(6),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new(["Language", "Files", "Lines", "%"]).style(theme.dim_style()))
        .column_spacing(1)
        .block(bblock);
    f.render_widget(table, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn percentages_sum_to_one_hundred() {
        let p = percentages(&[1, 1, 1]);
        assert_eq!(p.iter().sum::<u64>(), 100);
        assert_eq!(percentages(&[900, 50, 30, 20]).iter().sum::<u64>(), 100);
        assert_eq!(percentages(&[0, 0]), vec![0, 0]);
        assert_eq!(percentages(&[5]), vec![100]);
    }

    #[test]
    fn renders_language_table_and_total() {
        let loc = vec![
            LocRow {
                language: "Rust".into(),
                files: 10,
                lines: 1100,
                code: 900,
            },
            LocRow {
                language: "TOML".into(),
                files: 1,
                lines: 120,
                code: 100,
            },
        ];
        let mut term = Terminal::new(TestBackend::new(80, 16)).unwrap();
        term.draw(|f| render(f, f.area(), &loc, &Theme::default()))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("Rust"), "expected language row");
        assert!(s.contains("TOTAL"), "expected total row");
    }
}
