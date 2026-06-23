//! Full-screen detail view for a single Docker container's recent logs.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use crate::theme::Theme;

/// Render the scrollable container log view into `area`.
///
/// Renders a bordered block titled ` Container · {name} ` containing the
/// log lines as a scrollable `Paragraph`. A vertical scrollbar is shown on
/// the right edge of the inner content area.
pub fn render(f: &mut Frame, area: Rect, name: &str, logs: &[String], theme: &Theme, scroll: u16) {
    // Truncate the name so the title never wraps.
    let display_name: String = if name.chars().count() > 40 {
        let cut: String = name.chars().take(39).collect();
        format!("{cut}…")
    } else {
        name.to_string()
    };

    let title = format!(" Container · {display_name} ");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(theme.title());

    if logs.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled("no logs", theme.dim_style()))).block(block);
        f.render_widget(p, area);
        return;
    }

    let n = logs.len();
    let lines: Vec<Line> = logs.iter().map(|l| Line::from(l.as_str())).collect();

    let inner = block.inner(area);
    let p = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, 0));
    f.render_widget(p, area);

    // Scrollbar on the right side of the inner rect.
    let mut sb_state = ScrollbarState::new(n).position(scroll as usize);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    f.render_stateful_widget(scrollbar, inner, &mut sb_state);
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
    fn renders_container_logs() {
        let logs: Vec<String> = vec![
            "2026-06-23 INFO  server started".into(),
            "2026-06-23 INFO  listening on :8080".into(),
        ];
        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| {
            render(
                f,
                ratatui::layout::Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 20,
                },
                "sovra-backend",
                &logs,
                &theme,
                0,
            );
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(
            s.contains("Container"),
            "expected 'Container' in block title"
        );
        assert!(
            s.contains("sovra-backend"),
            "expected container name in title"
        );
        assert!(s.contains("server started"), "expected first log line");
    }

    #[test]
    fn renders_empty_logs() {
        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| {
            render(
                f,
                ratatui::layout::Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 12,
                },
                "svc",
                &[],
                &theme,
                0,
            );
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("no logs"), "expected 'no logs' message");
    }
}
