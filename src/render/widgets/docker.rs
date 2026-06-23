//! Docker widget: renders a stateful ratatui Table of running/stopped containers.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::docker::Container;
use crate::layout::Band;
use crate::theme::Theme;
use crate::util::human_bytes;

/// Truncate `s` to at most `max` chars, appending `…` if cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Row colour determined by health and state.
fn row_color(c: &Container, theme: &Theme) -> ratatui::style::Color {
    match c.health.as_deref() {
        Some("unhealthy") => theme.err,
        Some("starting") | Some("health: starting") => theme.warn,
        _ => {
            if c.state == "running" {
                theme.ok
            } else {
                theme.dim
            }
        }
    }
}

/// Health display: health word if present, else the state string.
fn health_text(c: &Container) -> &str {
    c.health.as_deref().unwrap_or(&c.state)
}

/// Render the Docker containers table into `area`.
///
/// Columns (Wide/Medium): `● | Name | Health | CPU% | Mem | Ports`
/// Compact band: `● | Name | CPU% | Mem`
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    containers: &[Container],
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
        .title(" Docker ")
        .border_style(border_style);

    if containers.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "Docker not running / no containers",
            theme.dim_style(),
        )))
        .block(block);
        f.render_widget(p, area);
        return;
    }

    let compact = band == Band::Compact;

    let full_widths: &[Constraint] = &[
        Constraint::Length(2),  // dot
        Constraint::Min(16),    // name
        Constraint::Length(10), // health
        Constraint::Length(7),  // cpu%
        Constraint::Min(14),    // mem
        Constraint::Min(16),    // ports
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2), // dot
        Constraint::Min(14),   // name
        Constraint::Length(7), // cpu%
        Constraint::Min(12),   // mem
    ];

    let widths = if compact { compact_widths } else { full_widths };

    let header = if compact {
        Row::new(["", "Name", "CPU%", "Mem"]).style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        Row::new(["", "Name", "Health", "CPU%", "Mem", "Ports"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    };

    let body: Vec<Row> = containers
        .iter()
        .map(|c| {
            let color = row_color(c, theme);

            let dot_cell = Cell::from(Span::styled("●", Style::new().fg(color)));
            let name_cell = Cell::from(Span::styled(
                truncate(&c.name, 24),
                Style::new().add_modifier(Modifier::BOLD),
            ));

            let cpu_cell = Cell::from(format!("{:.1}%", c.cpu_pct));

            let mem_text = if compact || c.mem_limit == 0 {
                human_bytes(c.mem_used)
            } else {
                format!("{} / {}", human_bytes(c.mem_used), human_bytes(c.mem_limit))
            };
            let mem_cell = Cell::from(mem_text);

            if compact {
                Row::new(vec![dot_cell, name_cell, cpu_cell, mem_cell])
            } else {
                let health_cell = Cell::from(Span::styled(
                    health_text(c).to_string(),
                    Style::new().fg(color),
                ));
                let ports_str = truncate(&c.ports.join(", "), 30);
                let ports_cell = Cell::from(Span::styled(ports_str, theme.dim_style()));
                Row::new(vec![
                    dot_cell,
                    name_cell,
                    health_cell,
                    cpu_cell,
                    mem_cell,
                    ports_cell,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::docker::Container;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, layout::Rect, widgets::TableState, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn running_container(name: &str) -> Container {
        Container {
            id: "abc123".into(),
            name: name.into(),
            state: "running".into(),
            status: "Up 5 minutes (healthy)".into(),
            health: Some("healthy".into()),
            cpu_pct: 12.5,
            mem_used: 256 * 1024 * 1024,
            mem_limit: 2 * 1024 * 1024 * 1024,
            ports: vec!["0.0.0.0:8080->8080/tcp".into()],
        }
    }

    #[test]
    fn renders_container_row() {
        let mut term = Terminal::new(TestBackend::new(140, 12)).unwrap();
        let mut st = TableState::default();
        let theme = Theme::default();
        let containers = vec![running_container("svc")];
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 140,
                    height: 12,
                },
                &containers,
                &theme,
                true,
                crate::layout::Band::Wide,
                &mut st,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("svc"), "expected 'svc' in buffer");
        assert!(
            s.contains("Docker"),
            "expected 'Docker' block title in buffer"
        );
    }

    #[test]
    fn renders_empty_state() {
        let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut st = TableState::default();
        let theme = Theme::default();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 10,
                },
                &[],
                &theme,
                false,
                crate::layout::Band::Wide,
                &mut st,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Docker"), "expected 'Docker' block title");
        assert!(
            s.to_lowercase().contains("no containers") || s.to_lowercase().contains("not running")
        );
    }
}
