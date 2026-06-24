//! Code widget: renders the LOC table — per-language icon, brand colour, file
//! count, code lines, and a proportional bar so relative size reads at a glance.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;

use crate::collect::loc::LocRow;
use crate::theme::Theme;
use crate::util::thousands;

/// A Nerd Font glyph for a language name (falls back to a generic file icon).
/// Requires a Nerd Font in the terminal; unknown languages still render fine.
fn lang_icon(lang: &str) -> &'static str {
    match lang.to_ascii_lowercase().as_str() {
        "rust" => "\u{e7a8}",
        "python" => "\u{e73c}",
        "typescript" | "ts" => "\u{e628}",
        "tsx" | "jsx" => "\u{e7ba}",
        "javascript" | "js" => "\u{e74e}",
        "html" => "\u{e736}",
        "css" | "scss" | "sass" => "\u{e749}",
        "json" => "\u{e60b}",
        "yaml" | "yml" => "\u{e6a8}",
        "toml" => "\u{e615}",
        "sql" => "\u{e706}",
        "shell" | "bash" | "sh" | "zsh" => "\u{ea85}",
        "dockerfile" | "docker" => "\u{e7b0}",
        "markdown" | "md" => "\u{e73e}",
        "go" => "\u{e627}",
        "c" | "c header" => "\u{e61e}",
        "c++" | "cpp" => "\u{e61d}",
        "java" => "\u{e738}",
        "ruby" => "\u{e739}",
        "svg" | "xml" => "\u{e619}",
        "makefile" | "make" | "just" => "\u{e673}",
        "lua" => "\u{e620}",
        _ => "\u{f15b}",
    }
}

/// A brand-ish colour for a language name (falls back to a neutral grey).
fn lang_color(lang: &str) -> Color {
    match lang.to_ascii_lowercase().as_str() {
        "rust" => Color::Rgb(0xD9, 0x6B, 0x4B),
        "python" => Color::Rgb(0x4B, 0x8B, 0xBE),
        "typescript" | "ts" | "tsx" => Color::Rgb(0x31, 0x78, 0xC6),
        "javascript" | "js" | "jsx" => Color::Rgb(0xF7, 0xDF, 0x1E),
        "html" => Color::Rgb(0xE3, 0x4C, 0x26),
        "css" | "scss" | "sass" => Color::Rgb(0x56, 0x4C, 0xF0),
        "json" => Color::Rgb(0x9C, 0xA3, 0xAF),
        "yaml" | "yml" => Color::Rgb(0xE0, 0x6B, 0x74),
        "toml" => Color::Rgb(0xB5, 0x6A, 0x3B),
        "sql" => Color::Rgb(0xE3, 0x8C, 0x00),
        "shell" | "bash" | "sh" | "zsh" => Color::Rgb(0x4E, 0xAA, 0x25),
        "dockerfile" | "docker" => Color::Rgb(0x2E, 0x86, 0xE8),
        "markdown" | "md" => Color::Rgb(0x75, 0x9E, 0xB8),
        "go" => Color::Rgb(0x00, 0xAD, 0xD8),
        "c" | "c header" => Color::Rgb(0x55, 0x9B, 0xD4),
        "c++" | "cpp" => Color::Rgb(0xF3, 0x4B, 0x7D),
        "java" => Color::Rgb(0xE7, 0x6F, 0x00),
        "ruby" => Color::Rgb(0xCC, 0x34, 0x2D),
        "lua" => Color::Rgb(0x51, 0x6C, 0xCE),
        _ => Color::Rgb(0x9E, 0xA3, 0xAE),
    }
}

/// A left-aligned bar of `frac` (0..1) over `width` cells, using eighth-blocks
/// for sub-cell precision — gives the column a clean little histogram.
fn bar(frac: f64, width: usize) -> String {
    let frac = frac.clamp(0.0, 1.0);
    let eighths = (frac * width as f64 * 8.0).round() as usize;
    let full = (eighths / 8).min(width);
    let rem = eighths % 8;
    let mut s = "\u{2588}".repeat(full); // █
    if rem > 0 && full < width {
        s.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][rem - 1]);
    }
    s
}

/// Render the Code (LOC) table into `area`.
pub fn render(f: &mut Frame, area: Rect, rows: &[LocRow], theme: &Theme, focused: bool) {
    let border_style = if focused {
        Style::new().fg(theme.focus_border)
    } else {
        theme.dim_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Code ")
        .border_style(border_style);

    let widths: &[Constraint] = &[
        Constraint::Min(14),    // icon + language
        Constraint::Length(8),  // files
        Constraint::Length(10), // code lines
        Constraint::Min(8),     // proportional bar
    ];

    let header = Row::new(["Language", "Files", "Lines", ""])
        .style(theme.dim_style().add_modifier(Modifier::BOLD));

    let max_lines = rows.iter().map(|r| r.lines).max().unwrap_or(0).max(1) as f64;

    let mut body: Vec<Row> = rows
        .iter()
        .map(|r| {
            let color = lang_color(&r.language);
            let frac = r.lines as f64 / max_lines;
            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{} {}", lang_icon(&r.language), r.language),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    thousands(r.files as u64),
                    Style::new().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    thousands(r.lines as u64),
                    Style::new().fg(color),
                )),
                Cell::from(Span::styled(bar(frac, 8), Style::new().fg(color))),
            ])
        })
        .collect();

    // TOTAL row in accent colour.
    let tot = crate::collect::loc::totals(rows);
    body.push(Row::new(vec![
        Cell::from(Span::styled(
            "\u{f085} TOTAL", //
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            thousands(tot.files as u64),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            thousands(tot.lines as u64),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        Cell::from(""),
    ]));

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(2)
        .block(block);

    f.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_scales_and_caps() {
        assert_eq!(bar(0.0, 8), "");
        assert_eq!(bar(1.0, 8), "████████");
        assert!(bar(0.5, 8).chars().count() <= 8);
        // Over-unity is clamped, never overflows the width.
        assert_eq!(bar(2.0, 8).chars().count(), 8);
    }

    #[test]
    fn known_languages_get_distinct_colours() {
        assert_ne!(lang_color("rust"), lang_color("python"));
        assert_eq!(lang_color("ts"), lang_color("typescript"));
        // Unknown languages fall back to the neutral grey, not a panic.
        assert_eq!(lang_color("brainfuck"), Color::Rgb(0x9E, 0xA3, 0xAE));
    }
}
