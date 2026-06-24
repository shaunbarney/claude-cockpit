//! Cost widget: total spend, today's spend, cache-hit %, braille trend, per-model table.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table};
use ratatui::Frame;

use crate::collect::usage::{DayUsage, ModelUsage, UsageTotals};
use crate::layout::Band;
use crate::theme::Theme;
use crate::util::{short_model, thousands};

/// Pick the widest header variant that fits `width` columns.
fn header_line(
    total: f64,
    today_cost: f64,
    cache_pct: u64,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let full = format!("Total ${total:.2}  ·  Today ${today_cost:.2}  ·  cache-hit {cache_pct}%");
    let med = format!("${total:.2} · today ${today_cost:.2} · {cache_pct}%");
    let w = width as usize;
    if full.chars().count() <= w {
        Line::from(vec![
            Span::styled(format!("Total ${total:.2}"), Style::new().fg(theme.accent)),
            Span::raw(format!(
                "  ·  Today ${today_cost:.2}  ·  cache-hit {cache_pct}%"
            )),
        ])
    } else if med.chars().count() <= w {
        Line::from(vec![
            Span::styled(format!("${total:.2}"), Style::new().fg(theme.accent)),
            Span::raw(format!(" · today ${today_cost:.2} · {cache_pct}%")),
        ])
    } else {
        Line::from(Span::styled(
            format!("${total:.2}"),
            Style::new().fg(theme.accent),
        ))
    }
}

pub fn render(
    f: &mut Frame,
    area: Rect,
    totals: Option<&UsageTotals>,
    theme: &Theme,
    focused: bool,
    band: Band,
    today: &str,
) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Cost ")
        .border_style(border_style);

    let Some(totals) = totals else {
        let p = Paragraph::new(Line::from("no usage data"))
            .block(block)
            .style(theme.dim_style());
        f.render_widget(p, area);
        return;
    };

    if totals.total_cost_usd == 0.0 && totals.by_day.is_empty() {
        let p = Paragraph::new(Line::from("no usage data"))
            .block(block)
            .style(theme.dim_style());
        f.render_widget(p, area);
        return;
    }

    // Compute today's cost.
    let today_cost = totals
        .by_day
        .iter()
        .find(|d: &&DayUsage| d.day == today)
        .map(|d| d.cost_usd)
        .unwrap_or(0.0);

    // Cache-hit %.
    let total_tokens = totals.cache_read + totals.cache_write + totals.fresh_input;
    let cache_pct = (totals.cache_read * 100)
        .checked_div(total_tokens)
        .unwrap_or(0);

    // Adaptive spend curve — hourly when usage spans ~one day, daily over weeks.
    let cost_values = &totals.cost_trend.values;
    let trend_label = totals.cost_trend.label.as_str();

    let inner = block.inner(area);
    f.render_widget(block, area);

    let compact = band == Band::Compact;

    // Show as many models as vertically fit; the small dashboard cell stays tight,
    // the full-screen Expanded view shows the whole breakdown.
    let max_models = if compact {
        3
    } else {
        (inner.height as usize).saturating_sub(4).clamp(1, 20)
    };
    let top_models: Vec<&ModelUsage> = totals.by_model.iter().take(max_models).collect();

    if compact {
        // Compact: header + model table stacked.
        let rows_count = (top_models.len() as u16).max(1);
        let cuts =
            Layout::vertical([Constraint::Length(1), Constraint::Min(rows_count)]).split(inner);

        let header = header_line(
            totals.total_cost_usd,
            today_cost,
            cache_pct,
            cuts[0].width,
            theme,
        );
        f.render_widget(Paragraph::new(header), cuts[0]);
        render_model_table(f, cuts[1], &top_models, theme);
    } else {
        // Wide/medium: header + chart + model table.
        let chart_h = inner
            .height
            .saturating_sub(1 + top_models.len() as u16 + 1)
            .max(3);
        let model_h = (top_models.len() as u16 + 1).max(2);
        let cuts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(chart_h),
            Constraint::Min(model_h),
        ])
        .split(inner);

        let header = header_line(
            totals.total_cost_usd,
            today_cost,
            cache_pct,
            cuts[0].width,
            theme,
        );
        f.render_widget(Paragraph::new(header), cuts[0]);
        render_chart(f, cuts[1], cost_values, trend_label, theme);
        render_model_table(f, cuts[2], &top_models, theme);
    }
}

fn render_chart(f: &mut Frame, area: Rect, costs: &[f64], label: &str, theme: &Theme) {
    if costs.is_empty() {
        return;
    }
    // A line needs two points. With only one bucket (all spend inside a single
    // hour so far) draw the running figure instead of an empty chart — it still
    // reflects live data and explains the time basis.
    if costs.len() < 2 {
        let v = costs.first().copied().unwrap_or(0.0);
        hint(f, area, &format!("{label} · ${v:.2} so far"), theme);
        return;
    }
    let pts = crate::graph::points(costs);
    let max = crate::graph::max_y(costs);
    let n = costs.len() as f64;

    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(theme.accent))
        .data(&pts);

    let chart = Chart::new(vec![dataset])
        .x_axis(
            Axis::default()
                .bounds([0.0, (n - 1.0).max(1.0)])
                .labels(vec![Span::styled(label.to_string(), theme.dim_style())]),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, max])
                .labels(vec![Span::raw(""), Span::raw(format!("${max:.2}"))]),
        );

    f.render_widget(chart, area);
}

/// Render a dim, centered single-line hint in `area` (used when there is
/// too little data to draw a meaningful chart).
fn hint(f: &mut Frame, area: Rect, msg: &str, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let mid = area.height / 2;
    let row = Rect {
        y: area.y + mid,
        height: 1,
        ..area
    };
    let p = Paragraph::new(Line::from(Span::styled(msg.to_string(), theme.dim_style())))
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(p, row);
}

fn render_model_table(f: &mut Frame, area: Rect, models: &[&ModelUsage], theme: &Theme) {
    let rows: Vec<Row> = models
        .iter()
        .map(|m| {
            Row::new(vec![
                short_model(&m.model),
                format!("${:.2}", m.cost_usd),
                format!("{} out", thousands(m.output)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        &[
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(12),
        ],
    )
    .header(Row::new(["Model", "Cost", "Output"]).style(theme.dim_style()))
    .column_spacing(1);

    f.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::usage::{DayUsage, ModelUsage, UsageTotals};
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_cost_and_total() {
        let totals = UsageTotals {
            by_day: vec![DayUsage {
                day: "2026-06-23".into(),
                cost_usd: 0.50,
                tokens: 1000,
            }],
            by_model: vec![ModelUsage {
                model: "claude-sonnet-4-6".into(),
                cost_usd: 1.23,
                input: 100_000,
                output: 50_000,
                cache_write: 0,
                cache_read: 0,
            }],
            total_cost_usd: 1.23,
            cache_read: 10_000,
            cache_write: 5_000,
            fresh_input: 85_000,
            ..Default::default()
        };

        let mut term = Terminal::new(TestBackend::new(120, 16)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 16,
                },
                Some(&totals),
                &Theme::default(),
                false,
                Band::Wide,
                "2026-06-23",
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Cost"), "expected 'Cost' block title in buffer");
        assert!(s.contains("1.23"), "expected '1.23' total cost in buffer");
    }

    #[test]
    fn narrow_width_does_not_panic_and_shows_total() {
        let totals = UsageTotals {
            by_day: vec![DayUsage {
                day: "2026-06-23".into(),
                cost_usd: 0.50,
                tokens: 1000,
            }],
            by_model: vec![ModelUsage {
                model: "claude-opus-4-8-some-very-long-model-name".into(),
                cost_usd: 1.23,
                input: 100_000,
                output: 50_000,
                cache_write: 0,
                cache_read: 0,
            }],
            total_cost_usd: 1.23,
            cache_read: 10_000,
            cache_write: 5_000,
            fresh_input: 85_000,
            ..Default::default()
        };

        let mut term = Terminal::new(TestBackend::new(28, 10)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width: 28,
                    height: 10,
                },
                Some(&totals),
                &Theme::default(),
                false,
                Band::Compact,
                "2026-06-23",
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        // The compact header keeps the total visible even at 28 cols.
        assert!(
            s.contains("1.23"),
            "expected total cost visible when narrow"
        );
        // The right border column must not be overwritten by header text.
        let buf = term.backend().buffer();
        assert_eq!(
            buf.cell((27, 1)).map(|c| c.symbol().to_string()),
            Some("│".to_string()),
            "right border must be intact (no overflow)"
        );
    }
}
