//! Per-widget dispatch over [`crate::rows::TableRow`].
//!
//! `rows.rs` says what a single row type can do; this module maps a
//! [`WidgetKind`] onto the matching slice of [`DashboardData`] so callers can
//! ask "how many rows does Docker have, once the filter is applied?" without
//! knowing that Docker rows are `Container`s. One macro keeps the eight arms
//! honest — add a variant and the compiler points here.

use std::collections::HashMap;

use crate::app::DashboardData;
use crate::fuzzy::Fuzzy;
use crate::rows::{prepare, TableRow};
use crate::widget::WidgetKind;

/// Run `$body` with `$rows` bound to the slice backing `$kind`.
///
/// Widgets with no row model (Cost, Rate) bind an empty slice so every caller
/// gets a total function instead of an `Option`.
macro_rules! on_rows {
    ($kind:expr, $d:expr, |$rows:ident| $body:expr) => {
        match $kind {
            WidgetKind::Worktrees => {
                let $rows = &$d.worktrees[..];
                $body
            }
            WidgetKind::Jobs => {
                let $rows = &$d.jobs[..];
                $body
            }
            WidgetKind::Docker => {
                let $rows = &$d.containers[..];
                $body
            }
            WidgetKind::Ports => {
                let $rows = &$d.endpoints[..];
                $body
            }
            WidgetKind::Repo => {
                let $rows = &$d.skills[..];
                $body
            }
            WidgetKind::Procs => {
                let $rows = &$d.tools[..];
                $body
            }
            WidgetKind::Code => {
                let $rows = &$d.loc[..];
                $body
            }
            WidgetKind::Cost | WidgetKind::Activity => {
                let $rows: &[crate::collect::git::Worktree] = &[];
                $body
            }
        }
    };
}

/// Widgets backed by a row list — selectable, filterable, sortable.
pub fn is_table(kind: WidgetKind) -> bool {
    !matches!(kind, WidgetKind::Cost | WidgetKind::Activity)
}

/// Total rows before filtering.
pub fn total(kind: WidgetKind, d: &DashboardData) -> usize {
    on_rows!(kind, d, |rows| rows.len())
}

/// Indices to display, after `filter` and `sort`.
pub fn visible(
    kind: WidgetKind,
    d: &DashboardData,
    filter: &str,
    sort: Option<(usize, bool)>,
    fz: &mut Fuzzy,
) -> Vec<usize> {
    on_rows!(kind, d, |rows| prepare(rows, filter, sort, fz))
}

/// `(key, signature)` for every row, used to detect what changed between
/// refreshes.
pub fn signatures(kind: WidgetKind, d: &DashboardData) -> HashMap<String, String> {
    on_rows!(kind, d, |rows| rows
        .iter()
        .map(|r| (r.key(), r.sig()))
        .collect())
}

/// Sortable column names for a widget, for the sort indicator and help text.
pub fn sort_columns(kind: WidgetKind) -> &'static [&'static str] {
    match kind {
        WidgetKind::Worktrees => <crate::collect::git::Worktree as TableRow>::sort_columns(),
        WidgetKind::Jobs => <crate::collect::jobs::Job as TableRow>::sort_columns(),
        WidgetKind::Docker => <crate::collect::docker::Container as TableRow>::sort_columns(),
        WidgetKind::Ports => <crate::collect::ports::Endpoint as TableRow>::sort_columns(),
        WidgetKind::Repo => <crate::collect::skills::Skill as TableRow>::sort_columns(),
        WidgetKind::Procs => <crate::collect::tools::ToolStat as TableRow>::sort_columns(),
        WidgetKind::Code => <crate::collect::loc::LocRow as TableRow>::sort_columns(),
        WidgetKind::Cost | WidgetKind::Activity => &["order"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::jobs::Job;

    fn job(id: &str, name: &str, state: &str) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            state: state.into(),
            tempo: "active".into(),
            intent: String::new(),
            tasks: 0,
            queued: 0,
            cwd: String::new(),
            worktree_path: None,
            worktree_branch: None,
            created_at: Some(0),
            updated_at: Some(0),
            ..Default::default()
        }
    }

    fn data() -> DashboardData {
        DashboardData {
            jobs: vec![
                job("1", "alpha", "working"),
                job("2", "beta", "done"),
                job("3", "gamma", "blocked"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn counts_and_filters_rows() {
        let d = data();
        let mut fz = Fuzzy::default();
        assert_eq!(total(WidgetKind::Jobs, &d), 3);
        assert_eq!(visible(WidgetKind::Jobs, &d, "", None, &mut fz).len(), 3);
        assert_eq!(
            visible(WidgetKind::Jobs, &d, "beta", None, &mut fz),
            vec![1]
        );
        // Filtering also searches state, not just name.
        assert_eq!(
            visible(WidgetKind::Jobs, &d, "blocked", None, &mut fz),
            vec![2]
        );
    }

    #[test]
    fn chartless_widgets_have_no_rows() {
        let d = data();
        assert!(!is_table(WidgetKind::Cost));
        assert_eq!(total(WidgetKind::Cost, &d), 0);
        assert!(is_table(WidgetKind::Jobs));
    }

    #[test]
    fn signatures_are_keyed_by_identity() {
        let mut d = data();
        let before = signatures(WidgetKind::Jobs, &d);
        assert_eq!(before.len(), 3);
        d.jobs[0].state = "blocked".into();
        let after = signatures(WidgetKind::Jobs, &d);
        assert_eq!(before.keys().len(), after.keys().len());
        assert_ne!(before["1"], after["1"], "changed row must change signature");
        assert_eq!(before["2"], after["2"], "untouched row must not");
    }

    #[test]
    fn every_widget_declares_sort_columns() {
        for &k in WidgetKind::all() {
            let cols = sort_columns(k);
            assert!(!cols.is_empty(), "{k:?} has no sort columns");
            assert_eq!(cols[0], "order", "{k:?} must start from collector order");
        }
    }
}
