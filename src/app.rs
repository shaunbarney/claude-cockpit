//! Application state and view stack.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use ratatui::widgets::TableState;

use crate::collect::git::Worktree;
use crate::collect::loc::LocRow;
use crate::layout::FrameRects;
use crate::theme::Theme;
use crate::widget::WidgetKind;

/// All cached datasets shown on the dashboard. Extended per phase.
#[derive(Default)]
pub struct DashboardData {
    pub worktrees: Vec<Worktree>,
    pub loc: Vec<LocRow>,
    pub jobs: Vec<crate::collect::jobs::Job>,
    pub usage: Option<crate::collect::usage::UsageTotals>,
}

/// A scrollable in-app diff/log view.
#[derive(Clone, Default)]
pub struct DiffView { pub title: String, pub lines: Vec<String>, pub scroll: u16 }

/// A drill-in detail target.
pub enum Detail { Worktree(usize), Job(usize), Container(usize), Diff(DiffView) }

/// The current screen.
pub enum View { Dashboard, Expanded(WidgetKind), Detail(Detail) }

/// Per-widget UI state (selection, sort, scroll).
#[derive(Default)]
pub struct WidgetUiState {
    pub table: TableState, pub sort_col: usize, pub sort_desc: bool, pub scroll: u16,
}

pub struct App {
    pub data: Arc<Mutex<DashboardData>>,
    pub view: View,
    pub focus: WidgetKind,
    pub ui: HashMap<WidgetKind, WidgetUiState>,
    pub rects: FrameRects,
    pub theme: Theme,
    pub should_quit: bool,
    pub detail_scroll: u16,
    pub detail_table: TableState,
    pub last_wt_idx: Option<usize>,
    pub wt_detail: Option<crate::collect::git_detail::WorktreeDetail>,
}

impl App {
    pub fn new(theme: Theme) -> Self {
        App {
            data: Arc::new(Mutex::new(DashboardData::default())),
            view: View::Dashboard,
            focus: WidgetKind::Worktrees,
            ui: HashMap::new(),
            rects: FrameRects::default(),
            theme,
            should_quit: false,
            detail_scroll: 0,
            detail_table: TableState::default(),
            last_wt_idx: None,
            wt_detail: None,
        }
    }
    pub fn focus_next(&mut self) { self.focus = self.focus.next(); }
    pub fn focus_prev(&mut self) { self.focus = self.focus.prev(); }
    pub fn expand_focused(&mut self) { self.view = View::Expanded(self.focus); }
    /// Pop one level: Diff -> Worktree detail -> Dashboard; everything else -> Dashboard.
    pub fn back(&mut self) {
        let cur = std::mem::replace(&mut self.view, View::Dashboard);
        self.view = match cur {
            View::Detail(Detail::Diff(_)) => match self.last_wt_idx {
                Some(i) => View::Detail(Detail::Worktree(i)),
                None => View::Dashboard,
            },
            _ => View::Dashboard,
        };
    }

    pub fn ui_mut(&mut self, k: WidgetKind) -> &mut WidgetUiState {
        self.ui.entry(k).or_default()
    }
    pub fn focus_ui_mut(&mut self) -> &mut WidgetUiState {
        let k = self.focus;
        self.ui.entry(k).or_default()
    }
    pub fn ui_offset(&self, k: WidgetKind) -> usize {
        self.ui.get(&k).map(|u| u.table.offset()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn expand_and_back() {
        let mut app = App::new(Theme::default());
        assert!(matches!(app.view, View::Dashboard));
        app.expand_focused();
        assert!(matches!(app.view, View::Expanded(_)));
        app.back();
        assert!(matches!(app.view, View::Dashboard));
    }
    #[test] fn focus_next_wraps() {
        let mut app = App::new(Theme::default());
        let start = app.focus;
        for _ in 0..WidgetKind::all().len() { app.focus_next(); }
        assert_eq!(app.focus, start);
    }
}
