//! Responsive grid engine: size bands, widget placement, frame hit map.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use crate::widget::WidgetKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band { TooSmall, Compact, Medium, Wide }

pub fn band(a: Rect) -> Band {
    if a.width < 70 || a.height < 18 { return Band::TooSmall; }
    match a.width { 70..=109 => Band::Compact, 110..=169 => Band::Medium, _ => Band::Wide }
}

pub fn cols_for(b: Band) -> usize {
    match b { Band::Compact => 1, Band::Medium => 2, Band::Wide => 3, Band::TooSmall => 1 }
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

/// Place `kinds` into a `cols`-wide grid within `area`.
pub fn place(area: Rect, kinds: &[WidgetKind], cols: usize) -> Vec<(WidgetKind, Rect)> {
    if kinds.is_empty() || cols == 0 { return vec![]; }
    let rows = kinds.len().div_ceil(cols);
    let row_rects = Layout::vertical(vec![Constraint::Fill(1); rows]).split(area);
    let mut out = Vec::with_capacity(kinds.len());
    for (r, chunk) in kinds.chunks(cols).enumerate() {
        let cell_rects = Layout::horizontal(vec![Constraint::Fill(1); chunk.len()]).split(row_rects[r]);
        for (c, &kind) in chunk.iter().enumerate() {
            out.push((kind, cell_rects[c]));
        }
    }
    out
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
        self.widgets.iter().find(|(_, r)| r.contains(pos)).map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(w: u16, h: u16) -> Rect { Rect { x: 0, y: 0, width: w, height: h } }
    #[test] fn bands() {
        assert_eq!(band(r(60,40)), Band::TooSmall);
        assert_eq!(band(r(100,10)), Band::TooSmall);
        assert_eq!(band(r(90,30)),  Band::Compact);
        assert_eq!(band(r(130,40)), Band::Medium);
        assert_eq!(band(r(200,50)), Band::Wide);
    }
    #[test] fn place_covers_area() {
        let a = r(200, 60);
        let kinds = vec![WidgetKind::Worktrees, WidgetKind::Jobs, WidgetKind::Cost, WidgetKind::Code];
        let placed = place(a, &kinds, 2);
        assert_eq!(placed.len(), 4);
        for (_, rc) in &placed { assert!(rc.right() <= a.right() && rc.bottom() <= a.bottom()); }
    }
}
