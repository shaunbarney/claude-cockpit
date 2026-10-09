//! The project picker: every project at a glance, and the place you manage them.
//!
//! Registered projects sit on top (pinned first, with their `1`–`9` hotkeys);
//! suggestions from Claude Code history follow, dimmed, until you add them.
//! Columns shed right-to-left as the terminal narrows — name and status
//! survive longest because they are what you pick by.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState,
};
use ratatui::Frame;

use crate::projects::overview::ProjectRow;
use crate::projects::paths::{canonical, contract_tilde};
use crate::projects::picker::{Picker, Prompt};
use crate::theme::Theme;
use crate::util::human_duration;

const HINTS: &str =
    "Enter open · 1-9 pinned · a add · n add path · d remove · p pin · r rename · t tags · / filter · Esc close";

/// Columns in priority order; the first `n` that fit are shown.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Col {
    Key,
    Name,
    Status,
    Branch,
    Jobs,
    Active,
    Worktrees,
    Cost,
    Path,
}

impl Col {
    fn header(self) -> &'static str {
        match self {
            Col::Key => "",
            Col::Name => "Project",
            Col::Status => "Dirty",
            Col::Branch => "Branch",
            Col::Jobs => "Jobs",
            Col::Active => "Active",
            Col::Worktrees => "Trees",
            Col::Cost => "Last $",
            Col::Path => "Path",
        }
    }
    fn width(self) -> u16 {
        match self {
            Col::Key => 3,
            Col::Name => 22,
            Col::Status => 6,
            Col::Branch => 18,
            Col::Jobs => 7,
            Col::Active => 7,
            Col::Worktrees => 6,
            Col::Cost => 8,
            Col::Path => 30,
        }
    }
}

const PRIORITY: [Col; 9] = [
    Col::Key,
    Col::Name,
    Col::Status,
    Col::Jobs,
    Col::Active,
    Col::Branch,
    Col::Worktrees,
    Col::Cost,
    Col::Path,
];

/// Columns that fit `width`, kept in display order.
fn columns_for(width: u16) -> Vec<Col> {
    let mut used = 0u16;
    let mut chosen: Vec<Col> = Vec::new();
    for c in PRIORITY {
        let w = c.width() + 1;
        if used + w > width && chosen.len() >= 2 {
            break;
        }
        used += w;
        chosen.push(c);
    }
    let order = [
        Col::Key,
        Col::Name,
        Col::Branch,
        Col::Status,
        Col::Worktrees,
        Col::Jobs,
        Col::Active,
        Col::Cost,
        Col::Path,
    ];
    order.into_iter().filter(|c| chosen.contains(c)).collect()
}

fn ago(t: Option<i64>) -> String {
    let now = chrono::Utc::now().timestamp();
    t.map(|t| human_duration((now - t).max(0) as u64))
        .unwrap_or_else(|| "—".into())
}

fn cell(picker: &Picker, row: &ProjectRow, col: Col, theme: &Theme) -> Cell<'static> {
    let dim = theme.dim_style();
    match col {
        Col::Key => {
            let hot = picker
                .pinned_paths()
                .iter()
                .position(|p| canonical(p) == canonical(&row.path));
            let is_current = picker.current.as_deref() == Some(canonical(&row.path).as_path());
            let mark = if is_current { "▶" } else { " " };
            let key = hot
                .filter(|&i| i < 9)
                .map(|i| (i + 1).to_string())
                .unwrap_or_default();
            Cell::from(Line::from(vec![
                Span::styled(mark, Style::new().fg(theme.accent)),
                Span::styled(key, theme.muted_style()),
            ]))
        }
        Col::Name => {
            let (glyph, style) = match (row.registered, row.pinned, row.exists) {
                (_, _, false) => ("✕ ", Style::new().fg(theme.err)),
                (true, true, _) => ("★ ", Style::new().fg(theme.accent)),
                (true, false, _) => ("● ", Style::new().fg(theme.fg)),
                (false, _, _) => ("+ ", dim),
            };
            let name_style = if row.registered {
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            Cell::from(Line::from(vec![
                Span::styled(glyph, style),
                Span::styled(row.name.clone(), name_style),
            ]))
        }
        Col::Branch => Cell::from(Span::styled(row.branch.clone(), theme.muted_style())),
        Col::Status => match row.dirty {
            0 if row.exists => Cell::from(Span::styled("clean", dim)),
            0 => Cell::from(Span::styled("gone", Style::new().fg(theme.err))),
            n => Cell::from(Span::styled(n.to_string(), Style::new().fg(theme.warn))),
        },
        Col::Worktrees => Cell::from(Span::styled(
            if row.worktrees == 0 {
                "—".into()
            } else {
                row.worktrees.to_string()
            },
            theme.muted_style(),
        )),
        Col::Jobs => jobs_cell(row, theme),
        Col::Active => Cell::from(Span::styled(ago(row.last_active), theme.muted_style())),
        Col::Cost => Cell::from(Span::styled(
            row.last_cost
                .map(|c| format!("${c:.2}"))
                .unwrap_or_else(|| "—".into()),
            theme.muted_style(),
        )),
        Col::Path => Cell::from(Span::styled(contract_tilde(&row.path), dim)),
    }
}

fn jobs_cell(row: &ProjectRow, theme: &Theme) -> Cell<'static> {
    let mut spans = Vec::new();
    if row.jobs_active > 0 {
        spans.push(Span::styled(
            format!("◉{}", row.jobs_active),
            Style::new().fg(theme.ok),
        ));
    }
    if row.jobs_blocked > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("◐{}", row.jobs_blocked),
            Style::new().fg(theme.warn),
        ));
    }
    if spans.is_empty() {
        spans.push(Span::styled("—", theme.dim_style()));
    }
    Cell::from(Line::from(spans))
}

/// Prompt / feedback line under the table.
fn status_line(picker: &Picker, theme: &Theme) -> Line<'static> {
    let accent = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    let label = match &picker.prompt {
        Some(Prompt::Filter) => Some("filter"),
        Some(Prompt::AddPath) => Some("add repo at (Tab completes)"),
        Some(Prompt::Rename) => Some("rename to"),
        Some(Prompt::Tags) => Some("tags (+add -remove)"),
        Some(Prompt::ConfirmRemove(name)) => {
            return Line::from(Span::styled(
                format!(" unregister {name}? the repository itself is untouched  [y/N]"),
                Style::new().fg(theme.warn),
            ))
        }
        None => None,
    };
    if let Some(label) = label {
        return Line::from(vec![
            Span::styled(format!(" {label} › "), accent),
            Span::styled(picker.input.clone(), Style::new().fg(theme.fg)),
            Span::styled("█", Style::new().fg(theme.accent)),
        ]);
    }
    match &picker.message {
        Some((m, true)) => Line::from(Span::styled(format!(" {m}"), Style::new().fg(theme.err))),
        Some((m, false)) => Line::from(Span::styled(format!(" {m}"), Style::new().fg(theme.ok))),
        None if !picker.filter.is_empty() => Line::from(Span::styled(
            format!(" filter: {}  (Esc in / clears)", picker.filter),
            theme.muted_style(),
        )),
        None => Line::from(Span::styled(format!(" {HINTS}"), theme.dim_style())),
    }
}

fn empty_hint(theme: &Theme) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No projects yet.",
            Style::new().fg(theme.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  n  add a repository by path",
            theme.muted_style(),
        )),
        Line::from(Span::styled(
            "  or run: claude-cockpit project add <path>",
            theme.muted_style(),
        )),
    ])
}

/// Draw the picker over `area`, nearly full-screen.
pub fn render(f: &mut Frame, area: Rect, picker: &Picker, theme: &Theme) {
    let popup = Rect {
        x: area.x + area.width.min(160).abs_diff(area.width) / 2,
        y: area.y + u16::from(area.height > 20),
        width: area.width.min(160),
        height: area
            .height
            .saturating_sub(if area.height > 20 { 2 } else { 0 }),
    };
    let registered = picker.rows.iter().filter(|r| r.registered).count();
    let suggested = picker.rows.len() - registered;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(
            " Projects · {registered} registered · {suggested} suggested "
        ))
        .title_style(theme.title())
        .border_style(Style::new().fg(theme.accent));
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    if inner.height < 3 {
        return;
    }
    let table_area = Rect {
        height: inner.height - 1,
        ..inner
    };
    let status_area = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    f.render_widget(Paragraph::new(status_line(picker, theme)), status_area);

    if picker.rows.is_empty() {
        f.render_widget(empty_hint(theme), table_area);
        return;
    }
    let cols = columns_for(table_area.width);
    let header = Row::new(cols.iter().map(|c| Cell::from(c.header())))
        .style(theme.muted_style().add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = picker
        .visible
        .iter()
        .map(|&i| {
            Row::new(
                cols.iter()
                    .map(|&c| cell(picker, &picker.rows[i], c, theme)),
            )
        })
        .collect();
    let widths: Vec<Constraint> = cols
        .iter()
        .map(|&c| {
            if c == Col::Path {
                Constraint::Min(c.width())
            } else {
                Constraint::Length(c.width())
            }
        })
        .collect();
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = TableState::default()
        .with_selected((!picker.visible.is_empty()).then_some(picker.selected));
    f.render_stateful_widget(table, table_area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::picker::Files;
    use crate::projects::{Registry, State};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(p: &Picker, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, f.area(), p, &Theme::default()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn files() -> Files {
        let d = crate::projects::testutil::temp_dir("render-pick");
        Files {
            registry: d.join("p.toml"),
            state: d.join("s.toml"),
        }
    }

    #[test]
    fn empty_registry_explains_how_to_add() {
        let p = Picker::new(Registry::default(), State::default(), files(), None);
        let s = screen(&p, 100, 20);
        assert!(s.contains("No projects yet"), "{s}");
        assert!(s.contains("claude-cockpit project add"));
    }

    #[test]
    fn rows_render_and_columns_shed_when_narrow() {
        let mut reg = Registry::default();
        reg.add(std::path::Path::new("/nonexistent/alpha"), Some("alpha"))
            .unwrap();
        reg.set_pinned("alpha", true).unwrap();
        let mut p = Picker::new(reg, State::default(), files(), None);
        let mut rows = p.rows.clone();
        rows.push(ProjectRow {
            name: "suggested-one".into(),
            path: "/nonexistent/s".into(),
            exists: true,
            ..Default::default()
        });
        p.set_rows(rows, &mut crate::fuzzy::Fuzzy::default());

        let wide = screen(&p, 150, 12);
        assert!(wide.contains("alpha") && wide.contains("suggested-one"));
        assert!(wide.contains("Path") && wide.contains("Branch"));
        let narrow = screen(&p, 50, 12);
        assert!(narrow.contains("alpha"));
        assert!(!narrow.contains("Path"));
    }

    #[test]
    fn column_order_is_stable_regardless_of_priority() {
        let cols = columns_for(500);
        assert_eq!(cols.first(), Some(&Col::Key));
        assert_eq!(cols.last(), Some(&Col::Path));
        assert!(columns_for(10).len() >= 2);
    }
}
