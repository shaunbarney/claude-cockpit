//! Full-screen detail for a single dev endpoint.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::ports::Endpoint;
use crate::theme::Theme;

pub fn render(f: &mut Frame, area: Rect, ep: &Endpoint, theme: &Theme) {
    let status = if ep.up {
        Span::styled("up", Style::new().fg(theme.ok))
    } else {
        Span::styled("down", Style::new().fg(theme.err))
    };
    let latency = ep
        .latency_ms
        .map(|ms| format!("{ms}ms"))
        .unwrap_or_else(|| "—".to_string());
    let pid = ep
        .pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "—".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled("service  ", theme.dim_style()),
            Span::styled(ep.label.clone(), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("address  ", theme.dim_style()),
            Span::raw(format!("{}:{}", ep.host, ep.port)),
        ]),
        Line::from(vec![Span::styled("status   ", theme.dim_style()), status]),
        Line::from(vec![
            Span::styled("latency  ", theme.dim_style()),
            Span::raw(latency),
        ]),
        Line::from(vec![
            Span::styled("pid      ", theme.dim_style()),
            Span::raw(pid),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Port · {} ", ep.label))
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_endpoint_detail() {
        let ep = Endpoint {
            label: "api".into(),
            host: "127.0.0.1".into(),
            port: 8080,
            up: true,
            latency_ms: Some(5),
            pid: Some(99),
        };
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| render(f, f.area(), &ep, &Theme::default()))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("api"), "expected label");
        assert!(s.contains("127.0.0.1:8080"), "expected address");
    }
}
