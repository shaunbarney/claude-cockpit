//! Full-screen Rate detail: cache efficiency + full prompt-cadence history.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::usage::UsageTotals;
use crate::theme::Theme;
use crate::util::thousands;

pub fn render(
    f: &mut Frame,
    area: Rect,
    cache: Option<&UsageTotals>,
    cadence: &[(String, u32)],
    theme: &Theme,
    scroll: u16,
) {
    let chunks = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).split(area);

    // Header: cache efficiency + prompt summary.
    let total_prompts: u32 = cadence.iter().map(|(_, n)| n).sum();
    let busiest = cadence
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(d, n)| format!("{d} ({n})"))
        .unwrap_or_else(|| "—".to_string());

    let cache_line = match cache {
        Some(t) if t.cache_read + t.cache_write + t.fresh_input > 0 => {
            let total = t.cache_read + t.cache_write + t.fresh_input;
            let hit = t.cache_read * 100 / total;
            Line::from(vec![
                Span::raw("cache-hit "),
                Span::styled(format!("{hit}%"), Style::new().fg(theme.accent)),
                Span::raw(format!(
                    "   read {}  write {}  fresh {}",
                    thousands(t.cache_read),
                    thousands(t.cache_write),
                    thousands(t.fresh_input),
                )),
            ])
        }
        _ => Line::from(Span::styled("no usage yet", theme.dim_style())),
    };

    let header = vec![
        cache_line,
        Line::from(vec![
            Span::styled("prompts ", theme.dim_style()),
            Span::raw(total_prompts.to_string()),
            Span::styled("   busiest ", theme.dim_style()),
            Span::raw(busiest),
            Span::styled("   active days ", theme.dim_style()),
            Span::raw(cadence.len().to_string()),
        ]),
    ];
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(" Rate ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(header)).block(hblock), chunks[0]);

    // Body: full daily cadence, newest first, with a simple bar.
    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(" Daily prompts ")
        .title_style(theme.title());
    if cadence.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no prompt history", theme.dim_style())).block(bblock),
            chunks[1],
        );
        return;
    }
    let max = cadence.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    let lines: Vec<Line> = cadence
        .iter()
        .rev()
        .map(|(day, n)| {
            let bar_len = (*n as usize * 24 / max as usize).max(if *n > 0 { 1 } else { 0 });
            let bar: String = "▇".repeat(bar_len);
            Line::from(vec![
                Span::styled(day.clone(), theme.dim_style()),
                Span::raw("  "),
                Span::styled(bar, Style::new().fg(theme.accent)),
                Span::raw(format!(" {n}")),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(bblock)
            .scroll((scroll, 0)),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_cadence_history() {
        let cadence = vec![
            ("2026-06-22".to_string(), 3u32),
            ("2026-06-23".to_string(), 8u32),
        ];
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| render(f, f.area(), None, &cadence, &Theme::default(), 0))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("Rate"), "expected title");
        assert!(s.contains("2026-06-23"), "expected a day row");
        assert!(s.contains("busiest"), "expected busiest summary");
    }
}
