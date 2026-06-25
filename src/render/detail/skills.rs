//! Full-screen Skill detail: identity, source, path, usage, and the skill's
//! own description from its `SKILL.md`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::collect::skills::Skill;
use crate::theme::Theme;

pub fn render(f: &mut Frame, area: Rect, skill: Option<&Skill>, theme: &Theme, scroll: u16) {
    let chunks = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).split(area);

    let Some(s) = skill else {
        f.render_widget(
            Paragraph::new("skill no longer present — Esc to go back")
                .style(theme.dim_style())
                .block(Block::default().borders(Borders::ALL).title(" Skill ")),
            area,
        );
        return;
    };

    let last = s
        .last_used
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "never".into());
    let path = s
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(invoked from another project — not on disk here)".into());

    let header = Text::from(vec![
        Line::from(vec![Span::styled(
            s.name.clone(),
            theme.title().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("source ", theme.dim_style()),
            Span::raw(s.source.clone()),
            Span::styled("   uses ", theme.dim_style()),
            Span::styled(s.uses.to_string(), Style::new().fg(theme.accent)),
            Span::styled("   last used ", theme.dim_style()),
            Span::raw(last),
        ]),
        Line::from(vec![
            Span::styled("path ", theme.dim_style()),
            Span::raw(path),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Skill ")
                .title_style(theme.title()),
        ),
        chunks[0],
    );

    let body = if s.description.is_empty() {
        "(no description)".to_string()
    } else {
        s.description.clone()
    };
    f.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: true })
            .scroll((scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Description ")
                    .title_style(theme.title()),
            ),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn renders_skill_detail() {
        let skill = Skill {
            name: "deploy".into(),
            description: "Run the CI gate then push.".into(),
            source: "personal".into(),
            path: None,
            uses: 7,
            last_used: None,
        };
        let mut term = Terminal::new(TestBackend::new(80, 16)).unwrap();
        term.draw(|f| render(f, f.area(), Some(&skill), &Theme::default(), 0))
            .unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("deploy"));
        assert!(s.contains("Description"));
        assert!(s.contains("Run the CI gate"));
    }
}
