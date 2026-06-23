//! Scrollable in-app diff viewer for a single file.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use crate::app::DiffView;
use crate::theme::Theme;

/// Render a scrollable diff view.
pub fn render(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    dv: &DiffView,
    theme: &Theme,
    scroll: u16,
) {
    // Truncate title if necessary.
    let max_title = area.width.saturating_sub(4) as usize;
    let title_str = if dv.title.len() > max_title && max_title > 3 {
        format!(" {}… ", &dv.title[..max_title - 1])
    } else {
        format!(" {} ", dv.title)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_str)
        .title_style(theme.title());

    if dv.lines.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled("no diff", theme.dim_style())).block(block),
            area,
        );
        return;
    }

    let lines: Vec<Line> = dv
        .lines
        .iter()
        .map(|l| colour_diff_line(l, theme))
        .collect();

    let para = Paragraph::new(lines).block(block).scroll((scroll, 0));
    f.render_widget(para, area);

    // Vertical scrollbar on the right.
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    let mut sb_state = ScrollbarState::new(dv.lines.len()).position(scroll as usize);
    f.render_stateful_widget(scrollbar, area, &mut sb_state);
}

/// Colour a single diff line based on its prefix.
fn colour_diff_line<'a>(line: &'a str, theme: &Theme) -> Line<'a> {
    if line.starts_with("+++") || line.starts_with("---") {
        // file header lines → dim
        Line::from(Span::styled(line, theme.dim_style()))
    } else if line.starts_with("@@") {
        Line::from(Span::styled(line, Style::new().fg(theme.accent)))
    } else if line.starts_with('+') {
        Line::from(Span::styled(line, Style::new().fg(theme.ok)))
    } else if line.starts_with('-') {
        Line::from(Span::styled(line, Style::new().fg(theme.err)))
    } else if line.starts_with("diff ") || line.starts_with("index ") {
        Line::from(Span::styled(line, theme.dim_style()))
    } else {
        Line::from(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_diff_view() {
        let dv = DiffView {
            title: "src/lib.rs · unstaged".into(),
            lines: vec![
                "@@ -1 +1 @@".into(),
                "+added line".into(),
                "-removed line".into(),
            ],
        };
        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| render(f, f.area(), &dv, &theme, 0)).unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("added"), "buffer should contain 'added'");
        assert!(s.contains("removed"), "buffer should contain 'removed'");
    }
}
