//! Rate widget: rate-limit proximity — rolling 5-hour window gauge, per-minute
//! output-token burn gauge, and a token-rate sparkline. Caps come from config
//! when set, otherwise auto-scale to your own busiest observed window.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Sparkline};
use ratatui::Frame;

use crate::collect::usage::{RateStats, UsageTotals};
use crate::layout::Band;
use crate::theme::Theme;

/// Compact human token count: 1_234_567 -> "1.2M", 41_000 -> "41k".
fn human(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Seconds -> short duration like "1h12m", "8m", "45s".
fn dur(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn ratio(val: u64, cap: u64) -> f64 {
    if cap == 0 {
        0.0
    } else {
        (val as f64 / cap as f64).clamp(0.0, 1.0)
    }
}

/// Gauge colour by proximity: calm under 70%, warn under 90%, then error.
fn proximity_color(r: f64, theme: &Theme) -> ratatui::style::Color {
    if r >= 0.9 {
        theme.err
    } else if r >= 0.7 {
        theme.warn
    } else {
        theme.accent
    }
}

/// Summary line, trimmed to fit `width`.
///
/// In the three-column grid this widget gets ~65 columns, where the full line
/// ran off the end mid-word ("cache-hit " with no number). Drop whole facts
/// instead of characters: a shorter true line beats a longer truncated one.
fn summary_line(
    prompts: u64,
    plan: &str,
    tokens: u64,
    cache_pct: u64,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let head = format!("{prompts} prompts");
    let tok = format!("{} tok", human(tokens));
    let cache = format!("cache-hit {cache_pct}%");
    let w = width as usize;

    // 3 for the "5h " prefix, 5 per "  ·  " separator.
    let full = 3 + head.len() + 5 + plan.len() + 5 + tok.len() + 5 + cache.len();
    let mid = 3 + head.len() + 5 + tok.len() + 5 + cache.len();

    let mut spans = vec![
        Span::styled("5h ", theme.dim_style()),
        Span::styled(head, Style::new().fg(theme.accent)),
    ];
    let sep = |t: &Theme| Span::styled("  ·  ", t.dim_style());
    if full <= w {
        spans.push(sep(theme));
        spans.push(Span::styled(plan.to_string(), theme.muted_style()));
    }
    if mid <= w {
        spans.push(sep(theme));
        spans.push(Span::raw(tok));
        spans.push(sep(theme));
        spans.push(Span::raw(cache));
    }
    Line::from(spans)
}

pub fn render(
    f: &mut Frame,
    area: Rect,
    usage: Option<&UsageTotals>,
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
        .title(" Rate ")
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(totals) = usage else {
        // The first transcript scan hasn't finished yet.
        let p = Paragraph::new(Line::from(Span::styled("scanning…", theme.dim_style())));
        f.render_widget(p, inner);
        return;
    };
    let rate = &totals.rate;

    // Cache-hit %, folded into the summary line (still useful context).
    let total_tok = totals.cache_read + totals.cache_write + totals.fresh_input;
    let cache_pct = (totals.cache_read * 100)
        .checked_div(total_tok)
        .unwrap_or(0);

    let plan = if rate.plan_label.is_empty() {
        "plan? run /status".to_string()
    } else {
        rate.plan_label.clone()
    };
    let summary = summary_line(
        rate.prompts_5h,
        &plan,
        rate.tokens_5h,
        cache_pct,
        inner.width,
        theme,
    );

    let spark: Vec<u64> = rate.burn_trend.values.iter().map(|v| *v as u64).collect();
    let spark_label = format!(
        "output tok/{} · {}",
        rate.burn_trend.unit(),
        rate.burn_trend.label
    );
    let spark_max = spark.iter().copied().max().unwrap_or(1).max(1);

    let compact = band == Band::Compact;

    if compact {
        // Compact: summary + 5h gauge + sparkline.
        let spark_h = inner.height.saturating_sub(2).max(1);
        let cuts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(spark_h),
        ])
        .split(inner);
        f.render_widget(Paragraph::new(summary), cuts[0]);
        render_window_gauge(f, cuts[1], rate, theme);
        render_sparkline(f, cuts[2], &spark, spark_max, &spark_label, theme);
    } else {
        // Wide/medium: summary + 5h gauge + per-minute gauge + sparkline.
        let spark_h = inner.height.saturating_sub(3).max(1);
        let cuts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(spark_h),
        ])
        .split(inner);
        f.render_widget(Paragraph::new(summary), cuts[0]);
        render_window_gauge(f, cuts[1], rate, theme);
        render_minute_gauge(f, cuts[2], rate, theme);
        render_sparkline(f, cuts[3], &spark, spark_max, &spark_label, theme);
    }
}

/// The rolling 5-hour window gauge — prompts used, the unit the Claude Code
/// subscription limit is actually measured in.
fn render_window_gauge(f: &mut Frame, area: Rect, rate: &RateStats, theme: &Theme) {
    let r = ratio(rate.prompts_5h, rate.cap_prompts_5h);
    let cap_note = if rate.auto_prompts_5h {
        format!("{} peak", rate.cap_prompts_5h)
    } else {
        rate.cap_prompts_5h.to_string()
    };
    let resets = rate
        .resets_in_secs
        .map(|s| format!("  resets {}", dur(s)))
        .unwrap_or_default();
    let label = format!(
        "5h  {} / {} prompts  {}%{}",
        rate.prompts_5h,
        cap_note,
        (r * 100.0).round() as u64,
        resets
    );
    let gauge = Gauge::default()
        .ratio(r)
        .label(Span::raw(label))
        .gauge_style(Style::new().fg(proximity_color(r, theme)));
    f.render_widget(gauge, area);
}

/// The per-minute output-token (OTPM) burn gauge.
fn render_minute_gauge(f: &mut Frame, area: Rect, rate: &RateStats, theme: &Theme) {
    let r = ratio(rate.output_1m, rate.cap_out_1m);
    let cap_note = if rate.auto_out_1m {
        format!("{} peak", human(rate.cap_out_1m))
    } else {
        human(rate.cap_out_1m)
    };
    let label = format!(
        "out/min  {} / {}  {}%",
        human(rate.output_1m),
        cap_note,
        (r * 100.0).round() as u64
    );
    let gauge = Gauge::default()
        .ratio(r)
        .label(Span::raw(label))
        .gauge_style(Style::new().fg(proximity_color(r, theme)));
    f.render_widget(gauge, area);
}

fn render_sparkline(f: &mut Frame, area: Rect, data: &[u64], max: u64, label: &str, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    // Token-rate per time bucket (hourly today, daily over weeks). One bucket is
    // nothing to plot yet — say so rather than drawing a lone full-height bar.
    if data.len() < 2 {
        let mid = area.height / 2;
        let row = Rect {
            y: area.y + mid,
            height: 1,
            ..area
        };
        let p = Paragraph::new(Line::from(Span::styled(
            "token-rate history fills in as you work".to_string(),
            theme.dim_style(),
        )))
        .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(p, row);
        return;
    }
    // Caption what the bars are, when there's room for a line above them.
    let area = if area.height >= 3 {
        let cuts = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label.to_string(),
                theme.dim_style(),
            ))),
            cuts[0],
        );
        cuts[1]
    } else {
        area
    };
    let spark = Sparkline::default()
        .data(data)
        .max(max)
        .style(Style::new().fg(theme.accent));
    f.render_widget(spark, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::usage::{RateStats, UsageTotals};
    use crate::theme::Theme;
    use crate::trend::Trend;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn human_and_dur_format() {
        assert_eq!(human(1_234_567), "1.2M");
        assert_eq!(human(41_000), "41k");
        assert_eq!(human(950), "950");
        assert_eq!(dur(4320), "1h12m");
        assert_eq!(dur(480), "8m");
        assert_eq!(dur(45), "45s");
    }

    #[test]
    fn renders_rate_widget() {
        let totals = UsageTotals {
            cache_read: 900,
            cache_write: 100,
            fresh_input: 0,
            rate: RateStats {
                prompts_5h: 72,
                cap_prompts_5h: 100,
                auto_prompts_5h: false,
                plan_label: "Max 5x".into(),
                resets_in_secs: Some(4320),
                output_1m: 41_000,
                cap_out_1m: 100_000,
                auto_out_1m: false,
                tokens_5h: 5_800_000,
                burn_trend: Trend {
                    values: vec![1000.0, 2000.0, 1500.0],
                    label: "last 3h".into(),
                    hourly: true,
                },
            },
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
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("Rate"), "expected 'Rate' title");
        assert!(s.contains("5h"), "expected 5h window gauge");
        assert!(s.contains("resets"), "expected reset countdown");
        assert!(
            s.contains("output tok/hour · last 3h"),
            "expected the sparkline caption"
        );
    }

    #[test]
    fn shows_scanning_before_first_scan() {
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term.draw(|f| render(f, f.area(), None, &Theme::default(), false, Band::Wide))
            .unwrap();
        assert!(buffer_text(term.backend().buffer()).contains("scanning"));
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::theme::Depth;

    fn text(width: u16) -> String {
        let theme = Theme::new(Depth::True, None);
        summary_line(13, "Max 5x", 255_400_000, 97, width, &theme)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect()
    }

    #[test]
    fn wide_summary_keeps_every_fact() {
        let s = text(120);
        assert!(s.contains("13 prompts"));
        assert!(s.contains("Max 5x"));
        assert!(s.contains("cache-hit 97%"));
    }

    #[test]
    fn narrow_summary_drops_facts_rather_than_truncating() {
        // A grid cell: the plan goes first, the numbers stay whole.
        let s = text(52);
        assert!(s.contains("13 prompts"));
        assert!(!s.contains("Max 5x"));
        assert!(s.contains("cache-hit 97%"), "{s}");
        assert!(s.chars().count() <= 52, "{s}");

        // Very narrow: only the headline survives, still complete.
        let s = text(20);
        assert!(s.contains("13 prompts"));
        assert!(!s.contains("cache-hit"));
        assert!(s.chars().count() <= 20, "{s}");
    }
}
