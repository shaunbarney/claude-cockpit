//! Ports widget: renders a stateful ratatui Table of dev-endpoint health checks.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::ports::Endpoint;
use crate::layout::Band;
use crate::theme::Theme;

/// Render the Ports table into `area`.
///
/// Columns (wide): `● | Service | Addr | Latency | PID`
/// Compact band: drops `PID`.
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    endpoints: &[Endpoint],
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
        .title(" Ports ")
        .border_style(border_style);

    if endpoints.is_empty() {
        let msg = Span::styled(
            "no endpoints — add [[endpoints]] to claude-cockpit.toml",
            theme.dim_style(),
        );
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    }

    let compact = band == Band::Compact;

    let full_widths: &[Constraint] = &[
        Constraint::Length(2),  // dot
        Constraint::Min(12),    // service
        Constraint::Length(22), // addr
        Constraint::Length(10), // latency
        Constraint::Length(8),  // pid
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2),  // dot
        Constraint::Min(12),    // service
        Constraint::Length(20), // addr
        Constraint::Length(10), // latency
    ];

    let widths = if compact { compact_widths } else { full_widths };

    let header = if compact {
        Row::new(["", "Service", "Addr", "Latency"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        Row::new(["", "Service", "Addr", "Latency", "PID"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    };

    let body: Vec<Row> = endpoints
        .iter()
        .map(|e| {
            let dot_color = if e.up { theme.ok } else { theme.err };
            let dot_cell = Cell::from(Span::styled("●", Style::new().fg(dot_color)));

            let service_cell = Cell::from(Span::styled(
                e.label.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ));

            let addr_cell = Cell::from(format!("{}:{}", e.host, e.port));

            let latency_cell = match e.latency_ms {
                Some(ms) => Cell::from(format!("{ms}ms")),
                None => Cell::from(Span::styled("down", theme.dim_style())),
            };

            let mut cells = vec![dot_cell, service_cell, addr_cell, latency_cell];

            if !compact {
                let pid_cell = match e.pid {
                    Some(p) => Cell::from(p.to_string()),
                    None => Cell::from(Span::styled("\u{2014}", theme.dim_style())),
                };
                cells.push(pid_cell);
            }

            Row::new(cells)
        })
        .collect();

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(1)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::ports::Endpoint;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, layout::Rect, widgets::TableState, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_endpoint_row() {
        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let mut st = TableState::default();
        let theme = Theme::default();
        let endpoints = vec![Endpoint {
            label: "api".to_string(),
            host: "127.0.0.1".to_string(),
            port: 8080,
            up: true,
            latency_ms: Some(5),
            pid: Some(99),
        }];
        term.draw(|f| {
            render(
                f,
                Rect { x: 0, y: 0, width: 120, height: 10 },
                &endpoints,
                &theme,
                true,
                crate::layout::Band::Wide,
                &mut st,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("api"), "expected 'api' in buffer");
        assert!(s.contains("Ports"), "expected 'Ports' block title in buffer");
    }
}
