//! Repo health widget: branch, ahead/behind, stash, dirty files, last fetch age.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::git::RepoHealth;
use crate::theme::Theme;
use crate::util::human_duration;

/// Render the Repo health widget into `area`.
pub fn render(
    f: &mut Frame,
    area: Rect,
    repo: Option<&RepoHealth>,
    theme: &Theme,
    focused: bool,
) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Repo ")
        .border_style(border_style);

    let inner = block.inner(area);

    match repo {
        None => {
            let p = Paragraph::new("not a git repo")
                .style(theme.dim_style())
                .block(block);
            f.render_widget(p, area);
        }
        Some(r) => {
            let branch_line = Line::from(vec![
                Span::styled("branch  ", theme.dim_style()),
                Span::styled(r.branch.clone(), Style::new().fg(theme.accent)),
            ]);

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
            let sync_line = Line::from(vec![
                Span::styled("↑", ahead_style),
                Span::styled(r.ahead.to_string(), ahead_style),
                Span::raw(" ahead   "),
                Span::styled("↓", behind_style),
                Span::styled(r.behind.to_string(), behind_style),
                Span::raw(" behind"),
            ]);

            let dirty_style = if r.dirty > 0 {
                Style::new().fg(theme.warn)
            } else {
                theme.dim_style()
            };
            let stash_dirty_line = Line::from(vec![
                Span::styled("stash ", theme.dim_style()),
                Span::raw(r.stash.to_string()),
                Span::raw("   "),
                Span::styled("dirty ", theme.dim_style()),
                Span::styled(r.dirty.to_string(), dirty_style),
            ]);

            let fetch_line = match r.last_fetch_secs {
                Some(secs) => Line::from(vec![
                    Span::styled("fetched ", theme.dim_style()),
                    Span::raw(human_duration(secs)),
                    Span::raw(" ago"),
                ]),
                None => Line::from(vec![
                    Span::styled("fetched: ", theme.dim_style()),
                    Span::raw("never"),
                ]),
            };

            let lines = vec![branch_line, sync_line, stash_dirty_line, fetch_line];
            let _ = inner; // block.inner used only to satisfy borrow; paragraph owns the block
            let p = Paragraph::new(lines).block(block);
            f.render_widget(p, area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::git::RepoHealth;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn repo_widget_renders_branch_and_title() {
        let repo = RepoHealth {
            branch: "main".to_string(),
            ahead: 2,
            behind: 0,
            stash: 1,
            dirty: 3,
            last_fetch_secs: Some(120),
        };
        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| render(f, f.area(), Some(&repo), &theme, false)).unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Repo"), "expected 'Repo' in output");
        assert!(s.contains("main"), "expected 'main' in output");
    }
}
