//! Terminal rendering: tables, churn cells, summary line, side-by-side join.
pub mod detail;

pub mod dashboard;
pub mod widgets;

use comfy_table::Color;

/// Accent violet, used for table titles and the LOC total.
pub const VIOLET: Color = Color::Rgb {
    r: 0x7C,
    g: 0x5C,
    b: 0xFF,
};

/// Join two pre-rendered multi-line blocks horizontally with `gap` spaces
/// between them. Shorter block is padded with blank lines; each left line is
/// padded to the left block's max *display* width (ANSI-aware) so the right
/// block aligns even when the left contains colour codes.
pub fn join_side_by_side(left: &str, right: &str, gap: usize) -> String {
    let l: Vec<&str> = left.lines().collect();
    let r: Vec<&str> = right.lines().collect();
    let lw = l
        .iter()
        .map(|s| console::measure_text_width(s))
        .max()
        .unwrap_or(0);
    let rows = l.len().max(r.len());
    let mut out = String::new();
    for i in 0..rows {
        let ls = l.get(i).copied().unwrap_or("");
        let rs = r.get(i).copied().unwrap_or("");
        let pad = lw - console::measure_text_width(ls) + gap;
        // Build the line in isolation, then trim only its own trailing gap
        // padding (present when the right side is empty for this row).
        let line = format!("{ls}{}{rs}", " ".repeat(pad));
        out.push_str(line.trim_end_matches(' '));
        out.push('\n');
    }
    out.pop();
    out
}

use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Attribute, Cell, CellAlignment,
    ContentArrangement, Table,
};

use crate::collect::git::Worktree;

/// A `+A/-D` churn cell, or a dim em-dash when not shown.
///
/// The whole cell is one colour: comfy-table styles a `Cell` with a single
/// `fg`, so the Python original's per-segment colouring (green `+A`, red `-D`)
/// can't be reproduced. We pick green for the additions-led string — an
/// intentional, accepted simplification, not a bug.
fn churn_cell(pair: (u32, u32), show: bool) -> Cell {
    if !show {
        return Cell::new("—").fg(Color::DarkGrey);
    }
    Cell::new(format!("+{}/-{}", pair.0, pair.1)).fg(Color::Green)
}

/// Build the Worktrees table.
pub fn worktree_table(rows: &[Worktree]) -> Table {
    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    t.set_header(vec![
        Cell::new(""),
        Cell::new("Worktree").add_attribute(Attribute::Bold),
        Cell::new("Ahead")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Dirty")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Committed")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Uncommitted")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Age")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
    ]);

    for r in rows {
        let (dot, ahead_cell) = if r.ahead > 0 {
            (
                Cell::new("●").fg(Color::Red),
                Cell::new(r.ahead)
                    .fg(Color::Red)
                    .add_attribute(Attribute::Bold),
            )
        } else if r.dirty > 0 {
            (
                Cell::new("●").fg(Color::Yellow),
                Cell::new("0").fg(Color::DarkGrey),
            )
        } else {
            (
                Cell::new("●").fg(Color::Green),
                Cell::new("0").fg(Color::DarkGrey),
            )
        };
        let dirty_cell = if r.dirty > 0 {
            Cell::new(r.dirty).fg(Color::Yellow)
        } else {
            Cell::new("0").fg(Color::DarkGrey)
        };
        t.add_row(vec![
            dot.set_alignment(CellAlignment::Center),
            Cell::new(&r.name).add_attribute(Attribute::Bold),
            ahead_cell.set_alignment(CellAlignment::Right),
            dirty_cell.set_alignment(CellAlignment::Right),
            churn_cell(r.committed, r.ahead > 0).set_alignment(CellAlignment::Right),
            churn_cell(r.uncommitted, r.dirty > 0).set_alignment(CellAlignment::Right),
            Cell::new(&r.age)
                .fg(Color::DarkGrey)
                .set_alignment(CellAlignment::Right),
        ]);
    }
    t
}

use crate::collect::loc::{loc_rows, loc_table};
use console::style;

/// Which layout to render.
#[derive(Clone, Copy)]
pub enum Mode {
    Worktrees,
    Code,
    Side,
}

/// The bottom summary line. `note` (may be empty) is appended dim.
pub fn summary_line(rows: &[Worktree], note: &str) -> String {
    let unmerged = rows.iter().filter(|r| r.ahead > 0).count();
    let dirty = rows.iter().filter(|r| r.ahead == 0 && r.dirty > 0).count();
    let clean = rows.len() - unmerged - dirty;
    let mut s = format!("  {}", style(format!("{} worktrees", rows.len())).bold());
    s.push_str(&format!(
        "   {} {}",
        style("●").red(),
        style(format!("{unmerged} unmerged")).red()
    ));
    s.push_str(&format!(
        "   {} {}",
        style("●").yellow(),
        style(format!("{dirty} dirty")).yellow()
    ));
    s.push_str(&format!(
        "   {} {}",
        style("●").green(),
        style(format!("{clean} clean")).green()
    ));
    if !note.is_empty() {
        s.push_str(&format!("      {}", style(note).dim()));
    }
    s
}

/// Prefix a rendered table block with a bold-violet title line.
fn titled(title: &str, table: &str) -> String {
    let heading = style(title).color256(99).bold(); // 99 ≈ violet in 256-palette
    format!("{heading}\n{table}")
}

/// Build the full frame for a mode. `note` is the optional watch annotation.
pub fn build_frame(root: &str, mode: Mode, note: &str) -> String {
    let rows = crate::collect::git::gather_worktrees(root);
    let wt = titled("Worktrees", &worktree_table(&rows).to_string());

    let body = match mode {
        Mode::Worktrees => wt,
        Mode::Code => {
            let loc = titled(
                "Repo Code · git-tracked",
                &loc_table(&loc_rows(root)).to_string(),
            );
            format!("{wt}\n\n{loc}")
        }
        Mode::Side => {
            let loc = titled(
                "Repo Code · git-tracked",
                &loc_table(&loc_rows(root)).to_string(),
            );
            join_side_by_side(&wt, &loc, 4)
        }
    };

    format!("\n{body}\n{}", summary_line(&rows, note))
}

use std::io::Write;
use std::time::Duration;

/// Top-anchored refresh: build the frame (slow git/tokei work) BEFORE clearing,
/// so the previous frame stays on screen until the new one is ready (no flash).
/// Ctrl-C exits cleanly.
pub fn watch(root: &str, mode: Mode, interval: f64) {
    let note = format!("⟳ every {}s · Ctrl-C to stop", trim_float(interval));
    loop {
        crate::collect::git::fetch_origin(root);
        let frame = build_frame(root, mode, &note);
        // clear screen + move cursor home, then print.
        println!("\x1b[2J\x1b[H{frame}");
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs_f64(interval));
    }
}

/// Format a watch interval compactly: 10.0 → "10", 2.5 → "2.5".
///
/// Rust's `f64` Display matches Python's `%g` across the realistic interval
/// range; the two only diverge at extreme magnitudes a `--interval` never takes.
fn trim_float(v: f64) -> String {
    format!("{v}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_float_matches_python_g() {
        assert_eq!(trim_float(10.0), "10");
        assert_eq!(trim_float(2.5), "2.5");
        assert_eq!(trim_float(0.5), "0.5");
    }

    #[test]
    fn joins_blocks_and_pads_uneven_heights() {
        let left = "aa\nbbbb";
        let right = "1\n2\n3";
        let out = join_side_by_side(left, right, 2);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "aa    1");
        assert_eq!(lines[1], "bbbb  2");
        assert_eq!(lines[2], "      3");
    }

    #[test]
    fn summary_counts_unmerged_dirty_clean() {
        use crate::collect::git::Worktree;
        let mk = |ahead: u32, dirty: u32| Worktree {
            name: "x".into(),
            path: String::new(),
            branch: String::new(),
            ahead,
            dirty,
            committed: (0, 0),
            uncommitted: (0, 0),
            age: String::new(),
        };
        let rows = vec![mk(3, 0), mk(0, 2), mk(0, 0), mk(0, 0)];
        let line = summary_line(&rows, "");
        // plain (ANSI-stripped) content is what we assert on
        let plain = console::strip_ansi_codes(&line);
        assert!(plain.contains("4 worktrees"));
        assert!(plain.contains("1 unmerged"));
        assert!(plain.contains("1 dirty"));
        assert!(plain.contains("2 clean"));
    }
}
