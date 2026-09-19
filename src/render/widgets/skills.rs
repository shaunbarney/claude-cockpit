//! Skills widget: Claude Code skills ranked by invocation count, with source
//! and a usage bar. A selectable table — drill into a row for the full skill.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use super::TableCtx;
use crate::collect::skills::Skill;
use crate::theme::Theme;
use crate::util::thousands;

/// A short tag + colour for a skill's source.
fn source_style(source: &str, theme: &Theme) -> (&'static str, Color) {
    match source {
        "personal" => ("personal", theme.accent),
        "project" => ("project", theme.ok),
        "plugin" => ("plugin", theme.warn),
        _ => ("used", theme.dim), // invoked but not found on disk
    }
}

/// Left-aligned eighth-block bar of `frac` (0..1) over `width` cells.
fn bar(frac: f64, width: usize) -> String {
    let frac = frac.clamp(0.0, 1.0);
    let eighths = (frac * width as f64 * 8.0).round() as usize;
    let full = (eighths / 8).min(width);
    let rem = eighths % 8;
    let mut s = "\u{2588}".repeat(full);
    if rem > 0 && full < width {
        s.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][rem - 1]);
    }
    s
}

/// Render the Skills table into `area`.
pub fn render(
    f: &mut Frame,
    area: Rect,
    skills: &[Skill],
    ctx: TableCtx<'_>,
    state: &mut TableState,
) {
    let TableCtx {
        theme,
        focused,
        band,
        flash,
        empty,
    } = ctx;
    if skills.is_empty() {
        let ctx = TableCtx {
            theme,
            focused,
            band,
            flash,
            empty,
        };
        return super::empty_state(f, area, " Skills ", empty, &ctx);
    }
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Skills ")
        .border_style(border_style);

    let max = skills.iter().map(|s| s.uses).max().unwrap_or(1).max(1) as f64;

    let full_widths: &[Constraint] = &[
        Constraint::Min(16),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Min(8),
    ];
    let compact = super::tight(area, band, full_widths, 2);

    let widths: &[Constraint] = if compact {
        &[Constraint::Min(12), Constraint::Length(6)]
    } else {
        &[
            Constraint::Min(16),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Min(8),
        ]
    };

    let rows: Vec<Row> = skills
        .iter()
        .map(|s| {
            let (tag, color) = source_style(&s.source, theme);
            let name = Cell::from(Span::styled(
                format!("\u{f0d0} {}", s.name), //
                Style::new().add_modifier(Modifier::BOLD),
            ));
            let uses = Cell::from(Span::styled(
                thousands(s.uses),
                Style::new().fg(theme.accent),
            ));
            if compact {
                Row::new(vec![name, uses])
            } else {
                Row::new(vec![
                    name,
                    Cell::from(Span::styled(tag, Style::new().fg(color))),
                    uses,
                    Cell::from(Span::styled(
                        bar(s.uses as f64 / max, 8),
                        Style::new().fg(theme.accent),
                    )),
                ])
            }
        })
        .collect();
    let rows = super::flashed(rows, flash);

    let header = if compact {
        Row::new(["Skill", "Uses"])
    } else {
        Row::new(["Skill", "Source", "Uses", ""])
    }
    .style(theme.dim_style().add_modifier(Modifier::BOLD));

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(2)
        .row_highlight_style(
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::REVERSED),
        )
        .block(block);
    f.render_stateful_widget(table, area, state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Band;
    use ratatui::{backend::TestBackend, Terminal};

    fn skill(name: &str, source: &str, uses: u64) -> Skill {
        Skill {
            name: name.into(),
            description: "desc".into(),
            source: source.into(),
            path: None,
            uses,
            last_used: None,
        }
    }

    #[test]
    fn renders_skills_table() {
        let skills = vec![
            skill("understand", "personal", 12),
            skill("deploy", "project", 5),
        ];
        let mut state = TableState::default();
        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 10,
                },
                &skills,
                super::TableCtx {
                    theme: &Theme::default(),
                    focused: true,
                    band: Band::Wide,
                    flash: &[],
                    empty: "nothing here",
                },
                &mut state,
            );
        })
        .unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("Skills"));
        assert!(s.contains("understand"));
        // A roomy cell keeps the optional Source column.
        assert!(s.contains("personal"));
    }

    #[test]
    fn a_narrow_cell_drops_optional_columns() {
        let skills = vec![skill("understand", "personal", 9)];
        let mut state = TableState::default();
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 10,
                },
                &skills,
                super::TableCtx {
                    theme: &Theme::default(),
                    focused: true,
                    band: Band::Wide,
                    flash: &[],
                    empty: "nothing here",
                },
                &mut state,
            );
        })
        .unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        // Better to drop a column than to render "Sourc" and a squeezed name.
        assert!(s.contains("understand"));
        assert!(!s.contains("personal"));
    }
}
