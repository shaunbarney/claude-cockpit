pub mod activity;
pub mod code;
pub mod cost;
pub mod docker;
pub mod jobs;
pub mod ports;
pub mod skills;
pub mod tools;
pub mod worktrees;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row};
use ratatui::Frame;

use crate::layout::Band;
use crate::theme::Theme;

/// Everything a table widget needs besides its rows.
///
/// These used to be five loose parameters; bundling them keeps the signatures
/// readable and means adding presentation state (the change-flash, the empty
/// message) does not widen five call sites every time.
pub struct TableCtx<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
    pub band: Band,
    /// Per-row change highlight, parallel to the rows slice.
    pub flash: &'a [Option<Style>],
    /// What to say when there are no rows to show.
    pub empty: &'a str,
}

impl TableCtx<'_> {
    /// Border style for this widget's block.
    pub fn border(&self) -> Style {
        self.theme.border_style(self.focused)
    }
}

/// Apply per-row change highlights to an already-built row list.
///
/// A flashed row's style replaces its base style for the ~1.6s fade. That is
/// deliberate: while a row is announcing that it just changed, "it changed" is
/// the most important thing about it. Cell-level styling still layers on top,
/// so state colours survive the flash.
pub fn flashed<'a>(rows: Vec<Row<'a>>, flash: &[Option<Style>]) -> Vec<Row<'a>> {
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| match flash.get(i).copied().flatten() {
            Some(s) => r.style(s),
            None => r,
        })
        .collect()
}

/// Draw a bordered widget whose body is a single centred, dim message.
///
/// An empty box tells you nothing — you cannot tell "no containers" from
/// "Docker is not running" from "this widget is broken". Every table says which.
pub fn empty_state(f: &mut Frame, area: Rect, title: &str, msg: &str, ctx: &TableCtx<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .title_style(ctx.theme.title())
        .border_style(ctx.border());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let row = Rect {
        y: inner.y + inner.height / 2,
        height: 1,
        ..inner
    };
    f.render_widget(
        Paragraph::new(Line::from(msg.to_string()))
            .style(ctx.theme.dim_style())
            .alignment(Alignment::Center),
        row,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Depth;
    use ratatui::{backend::TestBackend, Terminal};

    fn ctx(theme: &Theme) -> TableCtx<'_> {
        TableCtx {
            theme,
            focused: false,
            band: Band::Wide,
            flash: &[],
            empty: "nothing here",
        }
    }

    #[test]
    fn empty_state_explains_itself() {
        let theme = Theme::new(Depth::True, None);
        let mut term = Terminal::new(TestBackend::new(40, 6)).unwrap();
        term.draw(|f| empty_state(f, f.area(), " Docker ", "docker not running", &ctx(&theme)))
            .unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("Docker"));
        assert!(s.contains("docker not running"));
    }

    #[test]
    fn flashed_only_touches_marked_rows() {
        let rows = vec![Row::new(vec!["a"]), Row::new(vec!["b"])];
        let marks = [None, Some(Style::new().bg(ratatui::style::Color::Red))];
        let out = flashed(rows, &marks);
        assert_eq!(out.len(), 2);
        // Shorter flash slices must not panic — data can change between frames.
        let out = flashed(vec![Row::new(vec!["a"]), Row::new(vec!["b"])], &[]);
        assert_eq!(out.len(), 2);
    }
}
