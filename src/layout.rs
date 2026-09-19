//! Responsive grid engine: size bands, content-aware placement, frame hit map.
//!
//! The grid used to split the screen into equal cells, so an empty Worktrees
//! box in a single-worktree repo claimed exactly as much room as a Docker table
//! with fifteen containers. Placement is now weighted by how much each widget
//! actually has to show.
//!
//! What does *not* change is where a widget lives: the order of
//! [`WidgetKind::all`] fixes the grid position, and only the row heights move.
//! Spatial memory ("Docker is bottom-right") is most of what makes a dashboard
//! fast to read, and it survives exactly as long as panels stay put.

use crate::widget::WidgetKind;
use ratatui::layout::{Constraint, Layout, Position, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    TooSmall,
    Compact,
    Medium,
    Wide,
}

/// Borders (2) plus the table header line.
pub const CHROME: u16 = 3;
/// Shortest a widget may be: chrome plus one line of content or empty state.
pub const MIN_ROW: u16 = 4;
/// Tallest a single grid row may ask for, so one long table cannot starve the
/// rest of the dashboard.
pub const MAX_ROW: u16 = 22;

pub fn band(a: Rect) -> Band {
    if a.width < 70 || a.height < 18 {
        return Band::TooSmall;
    }
    match a.width {
        70..=109 => Band::Compact,
        110..=169 => Band::Medium,
        _ => Band::Wide,
    }
}

pub fn cols_for(b: Band) -> usize {
    match b {
        Band::Compact => 1,
        Band::Medium => 2,
        Band::Wide => 3,
        Band::TooSmall => 1,
    }
}

pub fn widgets_for(b: Band) -> Vec<WidgetKind> {
    use WidgetKind::*;
    match b {
        Band::Compact => vec![Worktrees, Jobs, Cost],
        Band::Medium => WidgetKind::all()[..6].to_vec(),
        Band::Wide => WidgetKind::all().to_vec(),
        Band::TooSmall => vec![],
    }
}

/// Height a widget would like, given how many content rows it has.
pub fn want_height(content_rows: usize) -> u16 {
    (content_rows as u16 + CHROME).clamp(MIN_ROW, MAX_ROW)
}

/// Split `total` across `wants`, honouring [`MIN_ROW`] and summing exactly.
///
/// Surplus goes to the hungriest rows and shortfall is taken from them, so a
/// dense table grows into free space while a one-line widget keeps its floor.
pub fn distribute(total: u16, wants: &[u16]) -> Vec<u16> {
    if wants.is_empty() {
        return vec![];
    }
    let n = wants.len() as u16;
    // Not even the minimums fit: share out evenly and let the renderer clip.
    if total < n * MIN_ROW {
        let each = (total / n).max(1);
        let mut out = vec![each; wants.len()];
        let used: u16 = each * n;
        if let Some(first) = out.first_mut() {
            *first += total.saturating_sub(used);
        }
        return out;
    }

    let mut out: Vec<u16> = wants.iter().map(|w| (*w).max(MIN_ROW)).collect();
    let sum: u16 = out.iter().sum();

    if sum < total {
        // Share the surplus in proportion to appetite, so a dense table takes
        // most of the free space and a one-line widget stays near its floor.
        // Leftover space has to go somewhere: leaving it blank at the bottom
        // reads worse than a slightly roomy table.
        let surplus = (total - sum) as u32;
        let want_sum: u32 = wants.iter().map(|w| *w as u32).sum::<u32>().max(1);
        for (i, w) in wants.iter().enumerate() {
            out[i] += (surplus * *w as u32 / want_sum) as u16;
        }
        // Rounding leaves a few lines over; give them to the hungriest rows.
        let mut order: Vec<usize> = (0..out.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(wants[i]));
        let mut left = total - out.iter().sum::<u16>();
        let mut i = 0;
        while left > 0 {
            out[order[i % order.len()]] += 1;
            left -= 1;
            i += 1;
        }
    } else if sum > total {
        // Shave from the tallest row until it fits, never below MIN_ROW.
        let mut over = sum - total;
        while over > 0 {
            let Some(tallest) = (0..out.len())
                .filter(|&i| out[i] > MIN_ROW)
                .max_by_key(|&i| out[i])
            else {
                break;
            };
            out[tallest] -= 1;
            over -= 1;
        }
    }
    out
}

/// Place `kinds` into a `cols`-wide grid within `area`.
///
/// `content_rows` reports how many rows each widget has to show; a grid row is
/// as tall as its hungriest member.
pub fn place_with(
    area: Rect,
    kinds: &[WidgetKind],
    cols: usize,
    content_rows: impl Fn(WidgetKind) -> usize,
) -> Vec<(WidgetKind, Rect)> {
    if kinds.is_empty() || cols == 0 || area.height == 0 {
        return vec![];
    }
    let wants: Vec<u16> = kinds
        .chunks(cols)
        .map(|chunk| {
            chunk
                .iter()
                .map(|&k| want_height(content_rows(k)))
                .max()
                .unwrap_or(MIN_ROW)
        })
        .collect();

    let heights = distribute(area.height, &wants);
    let row_rects = Layout::vertical(
        heights
            .iter()
            .map(|h| Constraint::Length(*h))
            .collect::<Vec<_>>(),
    )
    .split(area);

    let mut out = Vec::with_capacity(kinds.len());
    for (r, chunk) in kinds.chunks(cols).enumerate() {
        let cell_rects =
            Layout::horizontal(vec![Constraint::Fill(1); chunk.len()]).split(row_rects[r]);
        for (c, &kind) in chunk.iter().enumerate() {
            out.push((kind, cell_rects[c]));
        }
    }
    out
}

/// Equal-height placement — used by the Expanded view, where one widget owns
/// the screen and there is nothing to weigh it against.
pub fn place(area: Rect, kinds: &[WidgetKind], cols: usize) -> Vec<(WidgetKind, Rect)> {
    place_with(area, kinds, cols, |_| usize::from(u8::MAX))
}

/// The last frame's hit map for mouse routing.
#[derive(Default, Clone)]
pub struct FrameRects {
    pub widgets: Vec<(WidgetKind, Rect)>,
    pub table_inner: Option<Rect>,
    pub table_offset: usize,
}

impl FrameRects {
    pub fn widget_at(&self, pos: Position) -> Option<WidgetKind> {
        self.widgets
            .iter()
            .find(|(_, r)| r.contains(pos))
            .map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn bands() {
        assert_eq!(band(r(60, 40)), Band::TooSmall);
        assert_eq!(band(r(100, 10)), Band::TooSmall);
        assert_eq!(band(r(90, 30)), Band::Compact);
        assert_eq!(band(r(130, 40)), Band::Medium);
        assert_eq!(band(r(200, 50)), Band::Wide);
    }

    #[test]
    fn place_covers_area() {
        let a = r(200, 60);
        let kinds = vec![
            WidgetKind::Worktrees,
            WidgetKind::Jobs,
            WidgetKind::Cost,
            WidgetKind::Code,
        ];
        let placed = place(a, &kinds, 2);
        assert_eq!(placed.len(), 4);
        for (_, rc) in &placed {
            assert!(rc.right() <= a.right() && rc.bottom() <= a.bottom());
        }
    }

    #[test]
    fn distribute_sums_exactly() {
        for total in [12u16, 20, 47, 60] {
            let got = distribute(total, &[4, 12, 8]);
            assert_eq!(got.iter().sum::<u16>(), total, "total={total}");
        }
    }

    #[test]
    fn distribute_respects_the_floor() {
        // One row wants far more than the others; nobody drops below MIN_ROW.
        let got = distribute(20, &[MIN_ROW, 40, MIN_ROW]);
        assert!(got.iter().all(|&h| h >= MIN_ROW), "{got:?}");
        assert_eq!(got.iter().sum::<u16>(), 20);
    }

    #[test]
    fn distribute_feeds_the_hungriest_row() {
        // Plenty of space: the row that asked for most ends up tallest.
        let got = distribute(60, &[MIN_ROW, 20, 8]);
        assert_eq!(got.iter().sum::<u16>(), 60);
        assert!(got[1] > got[2] && got[2] > got[0], "{got:?}");
    }

    #[test]
    fn empty_widgets_do_not_hog_space() {
        let a = r(180, 40);
        let kinds = vec![WidgetKind::Worktrees, WidgetKind::Docker];
        // Worktrees empty, Docker with 15 containers, side by side in one row.
        let placed = place_with(
            a,
            &kinds,
            2,
            |k| {
                if k == WidgetKind::Docker {
                    15
                } else {
                    0
                }
            },
        );
        assert_eq!(placed.len(), 2);

        // Stacked instead: the dense one must get the taller cell.
        let stacked = place_with(
            a,
            &kinds,
            1,
            |k| {
                if k == WidgetKind::Docker {
                    15
                } else {
                    0
                }
            },
        );
        let wt = stacked[0].1.height;
        let dk = stacked[1].1.height;
        assert!(dk > wt * 2, "docker={dk} worktrees={wt}");
        assert!(wt >= MIN_ROW, "an empty widget still needs its floor");
        assert_eq!(wt + dk, 40, "the grid must fill the area exactly");
    }

    #[test]
    fn tiny_area_still_returns_every_widget() {
        let placed = place_with(
            r(80, 6),
            &[WidgetKind::Worktrees, WidgetKind::Jobs],
            1,
            |_| 5,
        );
        assert_eq!(placed.len(), 2);
        assert!(placed.iter().all(|(_, rc)| rc.height >= 1));
    }

    #[test]
    fn want_height_clamps_both_ends() {
        assert_eq!(want_height(0), MIN_ROW);
        assert_eq!(want_height(5), 5 + CHROME);
        assert_eq!(want_height(1000), MAX_ROW);
    }
}
