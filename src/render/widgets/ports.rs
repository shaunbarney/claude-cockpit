//! Ports widget: renders a stateful ratatui Table of dev-endpoint health checks.
//!
//! The Owner column is the container publishing the port when there is one
//! (Docker's `docker-proxy` PID is meaningless or invisible), otherwise the
//! listening PID when the OS lets us see it, otherwise a dash.

use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use super::TableCtx;
use crate::collect::ports::Endpoint;
use crate::theme::Level;

/// Shown under the caller's empty message when no filter is active.
const EMPTY_HINT: &str =
    "add [[endpoints]] to claude-cockpit.toml, or start this repo's compose stack";

/// Draw the empty state plus a one-line hint beneath the message.
fn render_empty(f: &mut Frame, area: Rect, ctx: &TableCtx<'_>) {
    super::empty_state(f, area, " Ports ", ctx.empty, ctx);
    if ctx.empty.starts_with("nothing matches") || area.height < 5 {
        return;
    }
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let row = Rect {
        y: inner.y + inner.height / 2 + 1,
        height: 1,
        ..inner
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(EMPTY_HINT, ctx.theme.dim_style())))
            .alignment(Alignment::Center),
        row,
    );
}

/// Owner column width: fits the longest owner, between 5 and 20 columns.
fn owner_width(endpoints: &[Endpoint]) -> u16 {
    endpoints
        .iter()
        .filter_map(|e| e.owner())
        .map(|o| o.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(5, 20) as u16
}

/// Render the Ports table into `area`.
///
/// Columns (wide): `● | Service | Addr | Latency | Owner`
/// Compact band: drops `Owner`.
pub fn render(
    f: &mut Frame,
    area: Rect,
    endpoints: &[Endpoint],
    ctx: TableCtx<'_>,
    state: &mut TableState,
) {
    if endpoints.is_empty() {
        return render_empty(f, area, &ctx);
    }
    let TableCtx {
        theme,
        focused,
        band,
        flash,
        ..
    } = ctx;
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Ports ")
        .border_style(border_style);

    let full_widths: &[Constraint] = &[
        Constraint::Length(2),                      // dot
        Constraint::Min(12),                        // service
        Constraint::Length(22),                     // addr
        Constraint::Length(10),                     // latency
        Constraint::Length(owner_width(endpoints)), // owner
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2),  // dot
        Constraint::Min(12),    // service
        Constraint::Length(20), // addr
        Constraint::Length(10), // latency
    ];

    let compact = super::tight(area, band, full_widths, 1);
    let widths = if compact { compact_widths } else { full_widths };

    let header = if compact {
        Row::new(["", "Service", "Addr", "Latency"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        Row::new(["", "Service", "Addr", "Latency", "Owner"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    };

    let body: Vec<Row> = endpoints
        .iter()
        .map(|e| {
            let level = if e.up { Level::Ok } else { Level::Err };
            let dot_cell = Cell::from(theme.level_span(level));

            let service_cell = Cell::from(Span::styled(
                e.label.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ));

            let addr_cell = Cell::from(e.addr());

            let latency_cell = match e.latency_ms {
                Some(ms) => Cell::from(format!("{ms}ms")),
                None => Cell::from(Span::styled("down", theme.dim_style())),
            };

            let mut cells = vec![dot_cell, service_cell, addr_cell, latency_cell];

            if !compact {
                let owner_cell = match e.owner() {
                    Some(o) => Cell::from(o),
                    None => Cell::from(Span::styled("\u{2014}", theme.dim_style())),
                };
                cells.push(owner_cell);
            }

            Row::new(cells)
        })
        .collect();
    let body = super::flashed(body, flash);

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
            ..Endpoint::default()
        }];
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 10,
                },
                &endpoints,
                super::TableCtx {
                    theme: &theme,
                    focused: true,
                    band: crate::layout::Band::Wide,
                    flash: &[],
                    empty: "nothing here",
                },
                &mut st,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("api"), "expected 'api' in buffer");
        assert!(
            s.contains("Ports"),
            "expected 'Ports' block title in buffer"
        );
    }

    #[test]
    fn owner_column_shows_container_and_empty_hint() {
        let theme = Theme::default();
        let endpoints = vec![Endpoint {
            label: "backend".into(),
            host: "127.0.0.1".into(),
            port: 8001,
            up: true,
            latency_ms: Some(1),
            container: Some("sovra-backend-dev".into()),
            ..Endpoint::default()
        }];
        let draw = |eps: &[Endpoint], empty: &str| {
            let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
            let mut st = TableState::default();
            term.draw(|f| {
                render(
                    f,
                    f.area(),
                    eps,
                    super::TableCtx {
                        theme: &theme,
                        focused: false,
                        band: crate::layout::Band::Wide,
                        flash: &[],
                        empty,
                    },
                    &mut st,
                )
            })
            .unwrap();
            buffer_text(term.backend().buffer())
        };
        let s = draw(&endpoints, "");
        assert!(s.contains("Owner") && s.contains("sovra-backend-dev"));
        assert!(draw(&[], "no dev endpoints found").contains("[[endpoints]]"));
    }
}
