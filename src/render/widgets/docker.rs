//! Docker widget: renders a stateful ratatui Table of containers.
//!
//! The rows arrive already scoped (see [`crate::collect::docker::scoped`]):
//! this repo's running containers by default. When the repo has none, the
//! list is every container on the host and the widget says so in its bottom
//! border, since every row then has `in_repo == false`.

use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use super::TableCtx;
use crate::collect::docker::Container;
use crate::theme::{Level, Theme};
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
fn row_level(c: &Container) -> Level {
    match c.health.as_deref() {
        Some("unhealthy") => Level::Err,
        Some("starting") | Some("health: starting") => Level::Warn,
        _ => {
            if c.state == "running" {
                Level::Ok
            } else {
                Level::Idle
            }
        }
    }
}

fn row_color(c: &Container, theme: &Theme) -> ratatui::style::Color {
    theme.level_color(row_level(c))
}

/// Health display: health word if present, else the state string.
fn health_text(c: &Container) -> &str {
    c.health.as_deref().unwrap_or(&c.state)
}

/// Shown under the caller's empty message when no filter is active.
const EMPTY_HINT: &str = "docker not running, not installed, or no containers";
/// Bottom-border note when the repo has no containers and all are shown.
const FALLBACK_NOTE: &str = " no containers for this repo — showing all ";

/// Draw the empty state plus a one-line hint beneath the message.
fn render_empty(f: &mut Frame, area: Rect, ctx: &TableCtx<'_>) {
    super::empty_state(f, area, " Docker ", ctx.empty, ctx);
    // A filter miss explains itself; only the "nothing at all" case needs help.
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

/// Name column width: as wide as the longest name, within what's left over.
fn name_width(containers: &[Container], area: Rect, others: u16) -> u16 {
    let longest = containers
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(4) as u16;
    let room = area.width.saturating_sub(2).saturating_sub(others);
    longest.min(room).max(8)
}

/// Render the Docker containers table into `area`.
///
/// Columns (Wide/Medium): `● | Name | Health | CPU% | Mem | Ports`
/// Compact band: `● | Name | CPU% | Mem`
pub fn render(
    f: &mut Frame,
    area: Rect,
    containers: &[Container],
    ctx: TableCtx<'_>,
    state: &mut TableState,
) {
    if containers.is_empty() {
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

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(" Docker ")
        .border_style(border_style);
    if containers.iter().all(|c| !c.in_repo) {
        block = block.title_bottom(Line::from(Span::styled(FALLBACK_NOTE, theme.dim_style())));
    }

    let full_widths: &[Constraint] = &[
        Constraint::Length(2),  // dot
        Constraint::Min(16),    // name
        Constraint::Length(10), // health
        Constraint::Length(7),  // cpu%
        Constraint::Min(14),    // mem
        Constraint::Min(12),    // ports
    ];
    let compact_widths: &[Constraint] = &[
        Constraint::Length(2), // dot
        Constraint::Min(14),   // name
        Constraint::Length(7), // cpu%
        Constraint::Min(12),   // mem
    ];

    let compact = super::tight(area, band, full_widths, 2);
    // Everything but the name column, including the 2-col gaps.
    let others = if compact {
        2 + 7 + 12 + 3 * 2
    } else {
        2 + 10 + 7 + 14 + 12 + 5 * 2
    };
    let name_w = name_width(containers, area, others);
    let mut widths: Vec<Constraint> = if compact {
        compact_widths.to_vec()
    } else {
        full_widths.to_vec()
    };
    widths[1] = Constraint::Length(name_w);

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

            let dot_cell = Cell::from(theme.level_span(row_level(c)));
            let name_cell = Cell::from(Span::styled(
                truncate(&c.name, name_w as usize),
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
    let body = super::flashed(body, flash);

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
            ..Container::default()
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
                super::TableCtx {
                    theme: &theme,
                    focused: false,
                    band: crate::layout::Band::Wide,
                    flash: &[],
                    empty: "no containers — is the docker daemon up?",
                },
                &mut st,
            )
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Docker"), "expected 'Docker' block title");
        // The caller supplies the message, so it can say "no matches" under a
        // filter and "daemon down" otherwise; the widget must render it.
        assert!(
            s.contains("no containers"),
            "expected the caller's empty message"
        );
    }

    fn draw(containers: &[Container], empty: &str, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut st = TableState::default();
        let theme = Theme::default();
        term.draw(|f| {
            render(
                f,
                f.area(),
                containers,
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
    }

    #[test]
    fn notes_fallback_only_when_no_repo_container() {
        let theirs = vec![running_container("other-project-db")];
        assert!(draw(&theirs, "", 120, 8).contains("showing all"));
        let mine = vec![Container {
            in_repo: true,
            ..running_container("mine")
        }];
        assert!(!draw(&mine, "", 120, 8).contains("showing all"));
    }

    #[test]
    fn long_names_are_not_cut_short() {
        let c = vec![running_container("sovrasrxproduction-victoriametrics-1")];
        assert!(draw(&c, "", 140, 8).contains("sovrasrxproduction-victoriametrics-1"));
    }

    #[test]
    fn empty_state_adds_hint_unless_filtered() {
        let s = draw(&[], "no containers — is the docker daemon up?", 80, 10);
        assert!(s.contains("docker not running"));
        let s = draw(&[], "nothing matches “zz”", 80, 10);
        assert!(!s.contains("docker not running"));
    }
}
