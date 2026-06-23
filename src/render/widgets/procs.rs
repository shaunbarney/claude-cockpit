//! Processes widget: renders a stateful ratatui Table of dev-process resource snapshots.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::procs::Proc;
use crate::layout::Band;
use crate::theme::Theme;
use crate::util::{human_bytes, human_duration};

/// Render the Processes table into `area`.
///
/// Columns (wide): `PID | Name | CPU% | Mem | Up`
/// Compact band: drops `Up` and `PID`.
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    procs: &[Proc],
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
        .title(" Processes ")
        .border_style(border_style);

    if procs.is_empty() {
        let msg = Span::styled("no dev processes", theme.dim_style());
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    }

    let compact = band == Band::Compact;

    let full_widths: &[Constraint] = &[
        Constraint::Length(7),  // pid
        Constraint::Min(14),    // name
        Constraint::Length(8),  // cpu%
        Constraint::Length(10), // mem
        Constraint::Length(8),  // up
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Min(14),    // name
        Constraint::Length(8),  // cpu%
        Constraint::Length(10), // mem
    ];

    let widths = if compact { compact_widths } else { full_widths };

    let header = if compact {
        Row::new(["Name", "CPU%", "Mem"]).style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        Row::new(["PID", "Name", "CPU%", "Mem", "Up"])
            .style(Style::new().add_modifier(Modifier::BOLD))
    };

    let body: Vec<Row> = procs
        .iter()
        .map(|p| {
            let cpu_style = if p.cpu_pct >= 50.0 {
                Style::new().fg(theme.warn)
            } else {
                Style::new()
            };

            let name_cell = Cell::from(Span::styled(
                p.name.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            let cpu_cell = Cell::from(Span::styled(format!("{:.1}%", p.cpu_pct), cpu_style));
            let mem_cell = Cell::from(human_bytes(p.mem_bytes));

            if compact {
                Row::new(vec![name_cell, cpu_cell, mem_cell])
            } else {
                let pid_cell = Cell::from(p.pid.to_string());
                let up_cell = Cell::from(Span::styled(
                    human_duration(p.uptime_secs),
                    theme.dim_style(),
                ));
                Row::new(vec![pid_cell, name_cell, cpu_cell, mem_cell, up_cell])
            }
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
    use crate::collect::procs::Proc;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, layout::Rect, widgets::TableState, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_proc_row() {
        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let mut st = TableState::default();
        let theme = Theme::default();
        let procs = vec![Proc {
            pid: 4242,
            name: "claude".into(),
            cmd: "claude --help".into(),
            cpu_pct: 12.0,
            mem_bytes: 256 * 1024 * 1024, // 256 MiB
            uptime_secs: 65,
        }];
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 12,
                },
                &procs,
                &theme,
                true,
                crate::layout::Band::Wide,
                &mut st,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("claude"), "expected 'claude' in buffer");
        assert!(
            s.contains("Processes"),
            "expected 'Processes' block title in buffer"
        );
    }
}
