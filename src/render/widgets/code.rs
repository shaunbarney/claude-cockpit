//! Code widget: renders the LOC table — per-language icon, brand colour, file
//! count, code lines, and a proportional bar so relative size reads at a glance.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::loc::LocRow;
use crate::theme::{Depth, Theme};
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

/// The colour a language is drawn in at this theme's depth. Brand colours are
/// 24-bit, so they only apply on a truecolor terminal; the ANSI tier uses the
/// accent and monochrome gets no colour at all.
fn lang_fg(theme: &Theme, lang: &str) -> Style {
    match theme.depth {
        Depth::True => Style::new().fg(lang_color(lang)),
        Depth::Ansi16 => Style::new().fg(theme.accent),
        Depth::Mono => Style::new(),
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
///
/// Stateful like every other table, so the cursor shows and scrolls. The
/// TOTAL sits in the bottom border rather than as a last row: as a row it
/// scrolled out of view the moment the table overflowed its cell.
pub fn render(
    f: &mut Frame,
    area: Rect,
    rows: &[LocRow],
    theme: &Theme,
    focused: bool,
    state: &mut TableState,
) {
    let tot = crate::collect::loc::totals(rows);
    let total = Line::from(vec![
        Span::styled(" TOTAL ", theme.title()),
        Span::styled(
            format!("{} files · ", thousands(tot.files as u64)),
            theme.dim_style(),
        ),
        Span::styled(
            format!("{} lines ", thousands(tot.lines as u64)),
            theme.title(),
        ),
    ])
    .right_aligned();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Code ")
        .title_bottom(total)
        .border_style(theme.border_style(focused));

    let widths: &[Constraint] = &[
        Constraint::Min(14),    // icon + language
        Constraint::Length(8),  // files
        Constraint::Length(10), // code lines
        Constraint::Min(8),     // proportional bar
    ];

    let header = Row::new(["Language", "Files", "Lines", ""])
        .style(theme.dim_style().add_modifier(Modifier::BOLD));

    let max_lines = rows.iter().map(|r| r.lines).max().unwrap_or(0).max(1) as f64;

    let body: Vec<Row> = rows
        .iter()
        .map(|r| {
            let fg = lang_fg(theme, &r.language);
            let frac = r.lines as f64 / max_lines;
            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{} {}", lang_icon(&r.language), r.language),
                    fg.add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(thousands(r.files as u64), theme.dim_style())),
                Cell::from(Span::styled(thousands(r.lines as u64), fg)),
                Cell::from(Span::styled(bar(frac, 8), fg)),
            ])
        })
        .collect();

    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(2)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, state);
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

    #[test]
    fn mono_theme_draws_languages_without_colour() {
        let mono = Theme::new(Depth::Mono, None);
        assert_eq!(lang_fg(&mono, "rust").fg, None);
    }

    #[test]
    fn total_stays_visible_when_rows_overflow() {
        use ratatui::{backend::TestBackend, Terminal};
        let rows: Vec<LocRow> = (0..30)
            .map(|i| LocRow {
                language: format!("L{i}"),
                files: 1,
                lines: 10,
                code: 10,
            })
            .collect();
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        let mut st = TableState::default();
        st.select(Some(29));
        term.draw(|f| render(f, f.area(), &rows, &Theme::default(), true, &mut st))
            .unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("TOTAL"), "{s}");
        assert!(s.contains("L29"), "selected last row must scroll into view");
    }
}
