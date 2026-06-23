//! Full-screen Repo health detail (branch, sync, stash, dirty, last fetch).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::git::RepoHealth;
use crate::theme::Theme;
use crate::util::human_duration;

pub fn render(f: &mut Frame, area: Rect, repo: Option<&RepoHealth>, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Repo ")
        .title_style(theme.title());

    let Some(r) = repo else {
        f.render_widget(
            Paragraph::new("not a git repo")
                .style(theme.dim_style())
                .block(block),
            area,
        );
        return;
    };

    let ahead_style = if r.ahead > 0 {
        Style::new().fg(theme.ok)
    } else {
        theme.dim_style()
    };
    let behind_style = if r.behind > 0 {
        Style::new().fg(theme.warn)
    } else {
        theme.dim_style()
    };
    let dirty_style = if r.dirty > 0 {
        Style::new().fg(theme.warn)
    } else {
        theme.dim_style()
    };
    let fetch = match r.last_fetch_secs {
        Some(secs) => format!("{} ago", human_duration(secs)),
        None => "never".to_string(),
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("branch   ", theme.dim_style()),
            Span::styled(r.branch.clone(), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("ahead    ", theme.dim_style()),
            Span::styled(r.ahead.to_string(), ahead_style),
            Span::styled("   behind ", theme.dim_style()),
            Span::styled(r.behind.to_string(), behind_style),
        ]),
        Line::from(vec![
            Span::styled("stash    ", theme.dim_style()),
            Span::raw(r.stash.to_string()),
            Span::styled("   dirty ", theme.dim_style()),
            Span::styled(r.dirty.to_string(), dirty_style),
        ]),
        Line::from(vec![
            Span::styled("fetched  ", theme.dim_style()),
            Span::raw(fetch),
        ]),
    ];
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
    fn renders_repo_detail() {
        let repo = RepoHealth {
            branch: "main".into(),
            ahead: 2,
            behind: 0,
            stash: 1,
            dirty: 3,
            last_fetch_secs: Some(120),
        };
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| render(f, f.area(), Some(&repo), &Theme::default()))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("main"), "expected branch");
        assert!(s.contains("fetched"), "expected fetch line");
    }
}
