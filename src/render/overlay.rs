//! Floating surfaces: the key help and the command palette.
//!
//! Both are summoned, do one job, and vanish — they own no persistent state and
//! never displace the dashboard underneath. The help sheet is grouped by the
//! layer a user reaches for (move, then act, then find), so it reads as a map
//! of the vocabulary rather than an alphabetised dump.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::palette::Entry;
use crate::theme::Theme;

/// Key help, grouped by layer. `None` marks a group break.
const HELP: &[Option<(&str, &str)>] = &[
    Some(("Tab / ⇧Tab", "cycle widgets")),
    Some(("↑↓←→ / h l", "move between widgets (dashboard)")),
    Some(("j / k", "select row · scroll a detail")),
    Some(("g G / Home End", "first / last")),
    Some(("PgUp PgDn / ^U ^D", "page")),
    None,
    Some(("Enter / click", "open the selection (click it again)")),
    Some(("e", "expand the focused widget")),
    Some(("Esc", "back, or clear the filter")),
    Some(("r", "refresh now")),
    None,
    Some(("/", "filter rows as you type")),
    Some((": / ^K / ^P", "command palette")),
    Some(("s / S", "cycle sort column / reverse")),
    None,
    Some(("wheel", "scroll the widget under the pointer")),
    Some(("?", "toggle this help")),
    Some(("q / ^C", "quit")),
];

/// Centre a `w` x `h` rect inside `area`, clamped to fit.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

/// Draw the key help sheet centred on `area`.
pub fn help(f: &mut Frame, area: Rect, theme: &Theme) {
    let key_w = HELP
        .iter()
        .flatten()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(8);

    let lines: Vec<Line> = HELP
        .iter()
        .map(|row| match row {
            Some((k, d)) => Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{k:>key_w$}"),
                    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("   {d}"), theme.muted_style()),
            ]),
            None => Line::from(""),
        })
        .collect();

    let desc_w = HELP
        .iter()
        .flatten()
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(30);
    let width = (key_w + desc_w + 8) as u16;
    let popup = centered(area, width, lines.len() as u16 + 2);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Keys ")
        .title_style(theme.title())
        .border_style(Style::new().fg(theme.accent));

    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Draw the command palette: a prompt plus the matching commands.
pub fn palette(
    f: &mut Frame,
    area: Rect,
    input: &str,
    entries: &[Entry],
    hits: &[usize],
    selected: usize,
    theme: &Theme,
) {
    const MAX_ROWS: usize = 12;
    let rows = hits.len().clamp(1, MAX_ROWS) as u16;
    let width = area.width.saturating_sub(8).clamp(30, 72);
    // +2 border, +1 prompt row.
    let popup = centered(area, width, rows + 3);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Command ")
        .title_style(theme.title())
        .border_style(Style::new().fg(theme.accent));
    let inner = block.inner(popup);

    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    if inner.height == 0 {
        return;
    }

    let prompt = Rect { height: 1, ..inner };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "› ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(input.to_string(), Style::new().fg(theme.fg)),
            Span::styled("█", Style::new().fg(theme.accent)),
        ])),
        prompt,
    );

    let list_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    if list_area.height == 0 {
        return;
    }

    if hits.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  no matching command",
                theme.dim_style(),
            ))),
            list_area,
        );
        return;
    }

    let items: Vec<ListItem> = hits
        .iter()
        .filter_map(|&i| entries.get(i))
        .map(|e| {
            let pad = (list_area.width as usize)
                .saturating_sub(e.label.chars().count() + e.hint.chars().count() + 4);
            ListItem::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(e.label.clone(), Style::new().fg(theme.fg)),
                Span::raw(" ".repeat(pad)),
                Span::styled(e.hint.clone(), theme.dim_style()),
            ]))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::new()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
    );
    let mut state = ListState::default();
    state.select(Some(selected.min(hits.len().saturating_sub(1))));
    f.render_stateful_widget(list, list_area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzzy::Fuzzy;
    use crate::palette::registry;
    use crate::theme::{Depth, Theme};
    use crate::widget::WidgetKind;
    use ratatui::{backend::TestBackend, Terminal};

    fn screen(width: u16, height: u16, draw: impl Fn(&mut Frame, Rect)) -> String {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| {
            let a = f.area();
            draw(f, a)
        })
        .unwrap();
        term.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn help_lists_every_layer() {
        let theme = Theme::new(Depth::True, None);
        let s = screen(90, 30, |f, a| help(f, a, &theme));
        assert!(s.contains("Keys"));
        assert!(s.contains("move between widgets"));
        assert!(s.contains("command palette"));
        assert!(s.contains("cycle sort column"));
    }

    #[test]
    fn help_fits_a_small_terminal_without_panicking() {
        let theme = Theme::new(Depth::True, None);
        let s = screen(24, 6, |f, a| help(f, a, &theme));
        assert_eq!(s.chars().count(), 24 * 6);
    }

    #[test]
    fn palette_shows_matches_and_marks_the_selection() {
        let theme = Theme::new(Depth::True, None);
        let entries = registry(WidgetKind::Jobs, true);
        let mut fz = Fuzzy::default();
        let hits = crate::palette::search(&entries, "dock", &mut fz);
        let s = screen(90, 24, |f, a| {
            palette(f, a, "dock", &entries, &hits, 0, &theme)
        });
        assert!(s.contains("Command"));
        assert!(s.contains("dock"));
        assert!(s.contains("Go to Docker"));
    }

    #[test]
    fn palette_says_so_when_nothing_matches() {
        let theme = Theme::new(Depth::True, None);
        let entries = registry(WidgetKind::Jobs, true);
        let s = screen(90, 24, |f, a| {
            palette(f, a, "zzzz", &entries, &[], 0, &theme)
        });
        assert!(s.contains("no matching command"), "{s}");
    }
}
