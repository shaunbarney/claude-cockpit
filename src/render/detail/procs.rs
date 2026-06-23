//! Full-screen detail for a single dev process, including its full command line.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::collect::procs::Proc;
use crate::theme::Theme;
use crate::util::{human_bytes, human_duration};

pub fn render(f: &mut Frame, area: Rect, p: &Proc, theme: &Theme, scroll: u16) {
    let chunks = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).split(area);

    let cpu_style = if p.cpu_pct >= 50.0 {
        Style::new().fg(theme.warn)
    } else {
        Style::new()
    };
    let header = vec![
        Line::from(vec![
            Span::styled("name   ", theme.dim_style()),
            Span::styled(p.name.clone(), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("pid    ", theme.dim_style()),
            Span::raw(p.pid.to_string()),
        ]),
        Line::from(vec![
            Span::styled("cpu    ", theme.dim_style()),
            Span::styled(format!("{:.1}%", p.cpu_pct), cpu_style),
            Span::styled("   mem ", theme.dim_style()),
            Span::raw(human_bytes(p.mem_bytes)),
            Span::styled("   up ", theme.dim_style()),
            Span::raw(human_duration(p.uptime_secs)),
        ]),
    ];
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Process · {} ", p.name))
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(header)).block(hblock), chunks[0]);

    let cmd = if p.cmd.is_empty() {
        "—".to_string()
    } else {
        p.cmd.clone()
    };
    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(" Command ")
        .title_style(theme.title());
    f.render_widget(
        Paragraph::new(cmd)
            .block(bblock)
            .wrap(Wrap { trim: false })
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
    fn renders_process_and_command() {
        let p = Proc {
            pid: 4242,
            name: "claude".into(),
            cmd: "claude --dangerously-skip-permissions run".into(),
            cpu_pct: 12.0,
            mem_bytes: 256 * 1024 * 1024,
            uptime_secs: 65,
        };
        let mut term = Terminal::new(TestBackend::new(80, 14)).unwrap();
        term.draw(|f| render(f, f.area(), &p, &Theme::default(), 0))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("claude"), "expected process name");
        assert!(
            s.contains("--dangerously-skip-permissions"),
            "expected full cmd"
        );
    }
}
