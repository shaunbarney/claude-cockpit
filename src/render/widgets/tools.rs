//! Tools widget: most-used tools across recent sessions, with proportional
//! bars. Replaces the old generic process list with something Claude-relevant.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::collect::tools::ToolStat;
use crate::layout::Band;
use crate::theme::Theme;
use crate::util::thousands;

/// A Nerd Font glyph per common tool (falls back to a generic gear).
fn tool_icon(name: &str) -> &'static str {
    match name {
        "Bash" | "BashOutput" | "KillShell" => "\u{ea85}", // terminal
        "Edit" | "Write" | "NotebookEdit" => "\u{f044}",   // pencil
        "Read" => "\u{f02d}",                              // book
        "Grep" | "Glob" => "\u{f002}",                     // magnifier
        "Task" | "Agent" => "\u{f0e7}",                    // bolt
        "WebFetch" | "WebSearch" => "\u{f0ac}",            // globe
        "TodoWrite" => "\u{f046}",                         // checklist
        _ if name.starts_with("mcp__") => "\u{f1e6}",      // plug (MCP)
        _ => "\u{f013}",                                   // gear
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

/// Strip the `mcp__server__` prefix so MCP tools read cleanly.
fn short_name(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

/// Render the Tools table into `area`.
pub fn render(f: &mut Frame, area: Rect, tools: &[ToolStat], theme: &Theme, focused: bool, band: Band) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Tools ")
        .border_style(border_style);

    if tools.is_empty() {
        let msg = Span::styled("no tool usage yet", theme.dim_style());
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    }

    let max = tools.iter().map(|t| t.count).max().unwrap_or(1).max(1) as f64;
    let compact = band == Band::Compact;

    let widths: &[Constraint] = if compact {
        &[Constraint::Min(10), Constraint::Length(7)]
    } else {
        &[Constraint::Min(14), Constraint::Length(9), Constraint::Min(10)]
    };

    let rows: Vec<Row> = tools
        .iter()
        .map(|t| {
            let name = format!("{} {}", tool_icon(&t.name), short_name(&t.name));
            let count = thousands(t.count);
            if compact {
                Row::new(vec![
                    Cell::from(Span::styled(name, Style::new().add_modifier(Modifier::BOLD))),
                    Cell::from(Span::styled(count, Style::new().fg(theme.accent))),
                ])
            } else {
                Row::new(vec![
                    Cell::from(Span::styled(name, Style::new().add_modifier(Modifier::BOLD))),
                    Cell::from(Span::styled(count, Style::new().fg(Color::DarkGray))),
                    Cell::from(Span::styled(
                        bar(t.count as f64 / max, 10),
                        Style::new().fg(theme.accent),
                    )),
                ])
            }
        })
        .collect();

    let header = if compact {
        Row::new(["Tool", "Uses"])
    } else {
        Row::new(["Tool", "Uses", ""])
    }
    .style(theme.dim_style().add_modifier(Modifier::BOLD));

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(2)
        .block(block);
    f.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_and_short_name() {
        assert_eq!(bar(1.0, 10), "██████████");
        assert_eq!(bar(0.0, 10), "");
        assert_eq!(short_name("mcp__chrome__click"), "click");
        assert_eq!(short_name("Bash"), "Bash");
    }

    #[test]
    fn renders_tools() {
        use ratatui::{backend::TestBackend, Terminal};
        let tools = vec![
            ToolStat { name: "Bash".into(), count: 1204 },
            ToolStat { name: "Edit".into(), count: 890 },
        ];
        let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect { x: 0, y: 0, width: 80, height: 10 },
                &tools,
                &Theme::default(),
                false,
                Band::Wide,
            );
        })
        .unwrap();
        let s: String = term.backend().buffer().content.iter().map(|c| c.symbol()).collect();
        assert!(s.contains("Tools"));
        assert!(s.contains("Bash"));
        assert!(s.contains("1,204"));
    }
}
