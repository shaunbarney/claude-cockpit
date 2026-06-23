//! Activity widget: cache-efficiency gauges + prompt-cadence sparkline.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Sparkline};
use ratatui::Frame;

use crate::collect::usage::UsageTotals;
use crate::layout::Band;
use crate::theme::Theme;
use crate::util::thousands;

pub fn render(
    f: &mut Frame,
    area: Rect,
    cache: Option<&UsageTotals>,
    cadence: &[(String, u32)],
    theme: &Theme,
    focused: bool,
    band: Band,
) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Activity ")
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // -- Cache efficiency section --
    let (cache_line, read_ratio) = if let Some(t) = cache {
        let total = t.cache_read + t.cache_write + t.fresh_input;
        if total == 0 {
            (
                Line::from(Span::styled("no usage yet", theme.dim_style())),
                None,
            )
        } else {
            let read_pct = t.cache_read * 100 / total;
            let ratio = t.cache_read as f64 / total as f64;
            let pct_style = if read_pct >= 50 {
                Style::new().fg(theme.ok)
            } else {
                Style::new().fg(theme.warn)
            };
            let line = Line::from(vec![
                Span::raw("cache-hit "),
                Span::styled(format!("{read_pct}%"), pct_style),
                Span::raw(format!(
                    "  ·  read {}  write {}  fresh {}",
                    thousands(t.cache_read),
                    thousands(t.cache_write),
                    thousands(t.fresh_input),
                )),
            ]);
            (line, Some((read_pct, ratio)))
        }
    } else {
        (
            Line::from(Span::styled("no usage yet", theme.dim_style())),
            None,
        )
    };

    // -- Sparkline data: last up-to-30 days --
    let spark_data: Vec<u64> = {
        let start = cadence.len().saturating_sub(30);
        cadence[start..].iter().map(|(_, n)| *n as u64).collect()
    };
    let spark_max = spark_data.iter().copied().max().unwrap_or(1).max(1);

    let compact = band == Band::Compact;

    if compact {
        // Compact: cache text + sparkline stacked, no gauge.
        let spark_h = inner.height.saturating_sub(1).max(1);
        let cuts =
            Layout::vertical([Constraint::Length(1), Constraint::Length(spark_h)]).split(inner);

        f.render_widget(Paragraph::new(cache_line), cuts[0]);
        render_sparkline(f, cuts[1], &spark_data, spark_max, theme);
    } else {
        // Wide/medium: cache text + optional gauge + sparkline.
        let gauge_h: u16 = if read_ratio.is_some() { 1 } else { 0 };
        let spark_h = inner.height.saturating_sub(2 + gauge_h).max(1);
        let cuts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(gauge_h),
            Constraint::Length(spark_h),
        ])
        .split(inner);

        f.render_widget(Paragraph::new(cache_line), cuts[0]);

        if let Some((_pct, ratio)) = read_ratio {
            let gauge = Gauge::default()
                .ratio(ratio)
                .label(Span::raw("cache hit"))
                .gauge_style(Style::new().fg(theme.accent));
            f.render_widget(gauge, cuts[1]);
        }

        render_sparkline(f, cuts[2], &spark_data, spark_max, theme);
    }
}

fn render_sparkline(f: &mut Frame, area: Rect, data: &[u64], max: u64, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let spark = Sparkline::default()
        .data(data)
        .max(max)
        .style(Style::new().fg(theme.accent));
    f.render_widget(spark, area);
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
    fn renders_activity_and_cache() {
        let totals = UsageTotals {
            by_day: vec![DayUsage {
                day: "2026-06-22".into(),
                cost_usd: 0.10,
                tokens: 1000,
            }],
            by_model: vec![ModelUsage {
                model: "claude-sonnet-4-6".into(),
                cost_usd: 0.10,
                input: 0,
                output: 0,
                cache_write: 0,
                cache_read: 0,
            }],
            total_cost_usd: 0.10,
            cache_read: 900,
            cache_write: 100,
            fresh_input: 0,
            ..Default::default()
        };
        let cadence = vec![
            ("2026-06-22".to_string(), 3u32),
            ("2026-06-23".to_string(), 5u32),
        ];

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
                &cadence,
                &Theme::default(),
                false,
                Band::Wide,
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Activity"), "expected 'Activity' block title");
        assert!(s.contains("cache"), "expected 'cache' text in buffer");
    }
}
