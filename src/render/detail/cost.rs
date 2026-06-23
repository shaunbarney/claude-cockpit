//! Full-screen Cost detail: all models with full token split, drill into one model.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::usage::{DayUsage, ModelUsage, UsageTotals};
use crate::theme::Theme;
use crate::util::thousands;

/// Strip a leading "claude-" prefix for compactness.
fn short_model(name: &str) -> String {
    name.strip_prefix("claude-").unwrap_or(name).to_string()
}

/// cache-read share of (input + cache-write + cache-read), as a percent. Output is
/// excluded to match the dashboard's cache-hit semantics.
fn cache_hit(input: u64, cw: u64, cr: u64) -> u64 {
    let total = input + cw + cr;
    if total == 0 {
        0
    } else {
        cr * 100 / total
    }
}

/// All-models breakdown with a selectable model table.
pub fn render(
    f: &mut Frame,
    area: Rect,
    totals: &UsageTotals,
    theme: &Theme,
    state: &mut TableState,
    today: &str,
) {
    let chunks = Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).split(area);
    render_header(f, chunks[0], totals, theme, today);
    render_models(f, chunks[1], &totals.by_model, theme, state);
}

fn render_header(f: &mut Frame, area: Rect, t: &UsageTotals, theme: &Theme, today: &str) {
    let today_cost = t
        .by_day
        .iter()
        .find(|d| d.day == today)
        .map(|d| d.cost_usd)
        .unwrap_or(0.0);
    let hit = cache_hit(t.fresh_input, t.cache_write, t.cache_read);
    let lines = vec![
        Line::from(vec![
            Span::raw("Total "),
            Span::styled(
                format!("${:.2}", t.total_cost_usd),
                Style::new().fg(theme.accent),
            ),
            Span::raw(format!("   Today ${today_cost:.2}   cache-hit {hit}%")),
        ]),
        Line::from(vec![
            Span::styled("tokens  ", theme.dim_style()),
            Span::raw(format!(
                "fresh {}  ·  cache-write {}  ·  cache-read {}",
                thousands(t.fresh_input),
                thousands(t.cache_write),
                thousands(t.cache_read),
            )),
        ]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Cost ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn render_models(
    f: &mut Frame,
    area: Rect,
    models: &[ModelUsage],
    theme: &Theme,
    state: &mut TableState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Models ")
        .title_style(theme.title());

    if models.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no usage data", theme.dim_style())).block(block),
            area,
        );
        return;
    }

    let rows: Vec<Row> = models
        .iter()
        .map(|m| {
            let hit = cache_hit(m.input, m.cache_write, m.cache_read);
            Row::new(vec![
                Cell::from(Span::styled(
                    short_model(&m.model),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Cell::from(format!("${:.2}", m.cost_usd)),
                Cell::from(thousands(m.input)),
                Cell::from(thousands(m.output)),
                Cell::from(thousands(m.cache_write)),
                Cell::from(thousands(m.cache_read)),
                Cell::from(format!("{hit}%")),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(12),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(5),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(["Model", "Cost", "In", "Out", "C-Write", "C-Read", "Hit"])
                .style(theme.dim_style()),
        )
        .column_spacing(1)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, state);
}

/// Single-model detail: token split + cost, then a scrollable day-by-day list.
pub fn render_model(
    f: &mut Frame,
    area: Rect,
    model: &ModelUsage,
    days: &[DayUsage],
    theme: &Theme,
    scroll: u16,
) {
    let chunks = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);

    let hit = cache_hit(model.input, model.cache_write, model.cache_read);
    let header = vec![
        Line::from(vec![
            Span::raw("Model: "),
            Span::styled(short_model(&model.model), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::raw("Cost:  "),
            Span::styled(
                format!("${:.2}", model.cost_usd),
                Style::new().fg(theme.accent),
            ),
        ]),
        Line::from(Span::raw(format!(
            "Tokens: in {}  out {}  c-write {}  c-read {}",
            thousands(model.input),
            thousands(model.output),
            thousands(model.cache_write),
            thousands(model.cache_read),
        ))),
        Line::from(Span::raw(format!("Cache-hit: {hit}%"))),
    ];
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(" Model ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(header)).block(hblock), chunks[0]);

    let n = days.len();
    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Daily ({n} days) "))
        .title_style(theme.title());

    if days.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no daily data", theme.dim_style())).block(bblock),
            chunks[1],
        );
        return;
    }

    let lines: Vec<Line> = days
        .iter()
        .rev()
        .map(|d| {
            Line::from(vec![
                Span::styled(d.day.clone(), theme.dim_style()),
                Span::raw(format!("   ${:.2}   ", d.cost_usd)),
                Span::styled(format!("{} tok", thousands(d.tokens)), theme.dim_style()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(bblock)
            .scroll((scroll, 0)),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::usage::{DayUsage, ModelUsage, UsageTotals};
    use ratatui::widgets::TableState;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn sample() -> UsageTotals {
        UsageTotals {
            by_model: vec![ModelUsage {
                model: "claude-opus-4-8".into(),
                cost_usd: 9.99,
                input: 1000,
                output: 2000,
                cache_write: 300,
                cache_read: 7000,
            }],
            total_cost_usd: 9.99,
            cache_read: 7000,
            cache_write: 300,
            fresh_input: 1000,
            ..Default::default()
        }
    }

    #[test]
    fn renders_all_models_table() {
        let totals = sample();
        let mut st = TableState::default();
        st.select(Some(0));
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| {
            render(
                f,
                f.area(),
                &totals,
                &Theme::default(),
                &mut st,
                "2026-06-23",
            )
        })
        .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("opus-4-8"), "expected shortened model name");
        assert!(s.contains("Hit"), "expected cache-hit column header");
    }

    #[test]
    fn renders_single_model_daily() {
        let m = sample().by_model[0].clone();
        let days = vec![DayUsage {
            day: "2026-06-22".into(),
            cost_usd: 4.0,
            tokens: 500,
        }];
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| render_model(f, f.area(), &m, &days, &Theme::default(), 0))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("opus-4-8"), "expected model name in header");
        assert!(s.contains("2026-06-22"), "expected day in daily list");
    }
}
