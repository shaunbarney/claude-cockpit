//! The status/hint line. Its whole job is to never be wrong.
//!
//! The old footer was one hardcoded string, so it advertised "e expand" while
//! you were three levels deep in a diff where `e` does nothing. Hints are now
//! derived from the current view and mode, which is also how the filter and
//! palette prompts get somewhere to live.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Detail, Mode, View};
use crate::view;

/// `(key, description)` hints for the current context, in priority order.
pub fn hints(app: &App) -> Vec<(&'static str, &'static str)> {
    match (&app.mode, &app.view) {
        (Mode::Filter, _) => vec![("Enter", "keep"), ("Esc", "clear"), ("↑↓", "select")],
        (Mode::Palette, _) => vec![("↑↓", "choose"), ("Enter", "run"), ("Esc", "cancel")],
        (_, View::Detail(Detail::Worktree)) => {
            vec![
                ("↑↓", "select"),
                ("Enter", "diff"),
                ("Esc", "back"),
                ("q", "quit"),
            ]
        }
        (_, View::Detail(Detail::Cost)) => {
            vec![
                ("↑↓", "select"),
                ("Enter", "model"),
                ("Esc", "back"),
                ("q", "quit"),
            ]
        }
        (_, View::Detail(_)) => vec![("↑↓", "scroll"), ("Esc", "back"), ("q", "quit")],
        (_, View::Expanded(_)) => vec![
            ("↑↓", "rows"),
            ("Enter", "open"),
            ("/", "filter"),
            ("s", "sort"),
            ("Esc", "back"),
        ],
        (_, View::Dashboard) => vec![
            ("↑↓←→", "move"),
            ("j/k", "rows"),
            ("Enter", "open"),
            ("/", "filter"),
            (":", "commands"),
            ("s", "sort"),
            ("e", "expand"),
            ("r", "refresh"),
            ("?", "keys"),
            ("q", "quit"),
        ],
    }
}

/// The filter/sort badge for the focused widget, e.g. `filter:sovra 3/15` or
/// `sort:cpu ↓`. Empty when the widget is showing everything, unsorted.
pub fn status_badge(app: &App) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let mut out = Vec::new();
    let k = app.focus;
    let Some(st) = app.ui.get(&k) else {
        return out;
    };

    if !st.filter.is_empty() {
        let shown = app.visible_count(k);
        let total = { view::total(k, &app.data.lock().unwrap()) };
        out.push(Span::styled(
            format!(" filter:{} ", st.filter),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
        out.push(Span::styled(
            format!("{shown}/{total} "),
            theme.muted_style(),
        ));
    }
    if st.sort_col != 0 {
        let cols = view::sort_columns(k);
        let name = cols.get(st.sort_col).copied().unwrap_or("?");
        let arrow = if st.sort_desc { "↓" } else { "↑" };
        out.push(Span::styled(
            format!(" sort:{name}{arrow} "),
            theme.muted_style(),
        ));
    }
    out
}

/// Render the footer: either a text prompt (filter mode) or the hint line.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;

    let mut spans: Vec<Span<'static>> = Vec::new();
    if app.mode == Mode::Filter {
        spans.push(Span::styled(
            " /",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            app.input.clone(),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
        // A block cursor makes it obvious the keyboard is captured.
        spans.push(Span::styled(
            "█",
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::SLOW_BLINK),
        ));
        let shown = app.visible_count(app.focus);
        spans.push(Span::styled(
            format!("  {shown} match{}", if shown == 1 { "" } else { "es" }),
            if shown == 0 {
                Style::new().fg(theme.warn)
            } else {
                theme.muted_style()
            },
        ));
        spans.push(Span::raw("  "));
    } else {
        spans.extend(status_badge(app));
        if spans.is_empty() {
            spans.push(Span::raw(" "));
        }
    }

    let mut left = (area.width as usize).saturating_sub(
        spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum::<usize>(),
    );

    for (i, (key, desc)) in hints(app).into_iter().enumerate() {
        let sep = if i == 0 { "  " } else { "  ·  " };
        let w = sep.chars().count() + key.chars().count() + desc.chars().count() + 1;
        if w > left {
            break;
        }
        left -= w;
        spans.push(Span::styled(sep, theme.dim_style()));
        spans.push(Span::styled(key.to_string(), Style::new().fg(theme.accent)));
        spans.push(Span::styled(format!(" {desc}"), theme.dim_style()));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use crate::widget::WidgetKind;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(app: &App, width: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(width, 1)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height: 1,
                },
                app,
            )
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
    fn hints_follow_the_view() {
        let mut app = App::new(Theme::default());
        assert!(hints(&app).iter().any(|(k, _)| *k == "e"));

        // A diff has no expand and no filter — the footer must not claim otherwise.
        app.view = View::Detail(Detail::Diff(Default::default()));
        let h = hints(&app);
        assert!(h.iter().all(|(k, _)| *k != "e"));
        assert!(h.iter().any(|(k, d)| *k == "Esc" && *d == "back"));
    }

    #[test]
    fn filter_mode_shows_the_prompt_and_match_count() {
        let mut app = App::new(Theme::default());
        app.mode = Mode::Filter;
        app.input = "sovra".into();
        let s = text(&app, 80);
        assert!(s.contains("/sovra"), "{s}");
        assert!(s.contains("0 matches"), "{s}");
    }

    #[test]
    fn badge_reports_filter_and_sort_state() {
        let mut app = App::new(Theme::default());
        app.focus = WidgetKind::Jobs;
        {
            let st = app.ui_mut(WidgetKind::Jobs);
            st.filter = "abc".into();
            st.sort_col = 2;
            st.sort_desc = true;
        }
        app.refilter();
        let joined: String = status_badge(&app)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(joined.contains("filter:abc"), "{joined}");
        assert!(joined.contains("sort:state↓"), "{joined}");
    }

    #[test]
    fn no_badge_when_unfiltered_and_unsorted() {
        let app = App::new(Theme::default());
        assert!(status_badge(&app).is_empty());
    }

    #[test]
    fn narrow_footer_drops_hints_instead_of_overflowing() {
        let app = App::new(Theme::default());
        let s = text(&app, 30);
        assert_eq!(s.chars().count(), 30);
        assert!(!s.contains("quit"), "low-priority hints should drop first");
    }
}
