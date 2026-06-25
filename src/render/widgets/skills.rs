//! Skills widget: Claude Code skills ranked by invocation count, with source
//! and a usage bar. A selectable table — drill into a row for the full skill.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::skills::Skill;
use crate::layout::Band;
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
        .title(" Skills ")
        .border_style(border_style);

    if skills.is_empty() {
        let msg = Span::styled(
            "no skills found — add one in ~/.claude/skills/<name>/SKILL.md",
            theme.dim_style(),
        );
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    }

    let max = skills.iter().map(|s| s.uses).max().unwrap_or(1).max(1) as f64;
    let compact = band == Band::Compact;

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
        let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 10,
                },
                &skills,
                &Theme::default(),
                true,
                Band::Wide,
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
        assert!(s.contains("personal"));
    }
}
