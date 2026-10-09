//! One description of "what a table row is", shared by filter, sort, and flash.
//!
//! Every table widget used to be its own island: the renderer knew the columns,
//! `event.rs` knew the row count, and nothing knew how to search or reorder.
//! [`TableRow`] gives each row type a stable identity (`key`), a change
//! signature (`sig`), searchable text, and a set of sortable columns — so
//! filtering, sorting, and change-highlighting are written once and work
//! everywhere instead of being reimplemented per widget.

use std::cmp::Ordering;

use crate::collect::docker::Container;
use crate::collect::git::Worktree;
use crate::collect::jobs::Job;
use crate::collect::loc::LocRow;
use crate::collect::ports::Endpoint;
use crate::collect::skills::Skill;
use crate::collect::tools::ToolStat;
use crate::fuzzy::Fuzzy;

/// A row that can be identified, searched, and ordered.
pub trait TableRow {
    /// Stable identity across refreshes. Used to follow a row as data updates.
    fn key(&self) -> String;
    /// Changes whenever anything user-visible about the row changes.
    fn sig(&self) -> String;
    /// Haystack for the fuzzy filter.
    fn search(&self) -> String;
    /// Sortable column names. Index 0 is always the collector's own order.
    fn sort_columns() -> &'static [&'static str];
    /// Compare two rows by `col`. Column 0 compares equal, leaving the
    /// collector's order intact under a stable sort.
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering;
}

/// Total order over `f32`, treating NaN as smallest. `f32::total_cmp` would
/// order -0.0 before 0.0, which is noise in a CPU column.
fn cmp_f32(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

impl TableRow for Worktree {
    fn key(&self) -> String {
        self.path.clone()
    }
    fn sig(&self) -> String {
        format!("{}|{}|{}|{}", self.branch, self.ahead, self.dirty, self.age)
    }
    fn search(&self) -> String {
        format!("{} {} {}", self.name, self.branch, self.path)
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "name", "ahead", "dirty"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.name.cmp(&b.name),
            2 => a.ahead.cmp(&b.ahead),
            3 => a.dirty.cmp(&b.dirty),
            _ => Ordering::Equal,
        }
    }
}

impl TableRow for Job {
    fn key(&self) -> String {
        self.id.clone()
    }
    fn sig(&self) -> String {
        format!(
            "{}|{}|{}|{}|{:?}",
            self.state, self.tempo, self.tasks, self.queued, self.updated_at
        )
    }
    fn search(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.state,
            self.intent,
            self.worktree_branch.as_deref().unwrap_or("")
        )
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "name", "state", "tasks", "updated"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.name.cmp(&b.name),
            2 => a.state.cmp(&b.state),
            3 => (a.tasks + a.queued).cmp(&(b.tasks + b.queued)),
            4 => a.updated_at.cmp(&b.updated_at),
            _ => Ordering::Equal,
        }
    }
}

impl TableRow for Container {
    fn key(&self) -> String {
        self.id.clone()
    }
    fn sig(&self) -> String {
        // CPU and memory are quantised: raw values jitter every sample, which
        // would leave every row permanently flashing.
        format!(
            "{}|{:?}|{}|{}",
            self.state,
            self.health,
            self.cpu_pct.round(),
            self.mem_used / (16 * 1024 * 1024)
        )
    }
    fn search(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.state,
            self.health.as_deref().unwrap_or(""),
            self.ports.join(" ")
        )
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "name", "cpu", "mem", "state"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.name.cmp(&b.name),
            2 => cmp_f32(a.cpu_pct, b.cpu_pct),
            3 => a.mem_used.cmp(&b.mem_used),
            4 => a.state.cmp(&b.state),
            _ => Ordering::Equal,
        }
    }
}

impl TableRow for Endpoint {
    fn key(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
    fn sig(&self) -> String {
        format!("{}|{:?}", self.up, self.latency_ms)
    }
    fn search(&self) -> String {
        format!("{} {} {}", self.label, self.host, self.port)
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "service", "port", "latency"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.label.cmp(&b.label),
            2 => a.port.cmp(&b.port),
            // Unreachable endpoints sort last rather than as latency 0.
            3 => a
                .latency_ms
                .unwrap_or(u64::MAX)
                .cmp(&b.latency_ms.unwrap_or(u64::MAX)),
            _ => Ordering::Equal,
        }
    }
}

impl TableRow for Skill {
    fn key(&self) -> String {
        format!("{}/{}", self.source, self.name)
    }
    fn sig(&self) -> String {
        format!("{}|{:?}", self.uses, self.last_used)
    }
    fn search(&self) -> String {
        format!("{} {} {}", self.name, self.source, self.description)
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "name", "uses", "source"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.name.cmp(&b.name),
            2 => a.uses.cmp(&b.uses),
            3 => a.source.cmp(&b.source),
            _ => Ordering::Equal,
        }
    }
}

impl TableRow for ToolStat {
    fn key(&self) -> String {
        self.name.clone()
    }
    fn sig(&self) -> String {
        self.count.to_string()
    }
    fn search(&self) -> String {
        self.name.clone()
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "name", "uses"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.name.cmp(&b.name),
            2 => a.count.cmp(&b.count),
            _ => Ordering::Equal,
        }
    }
}

impl TableRow for LocRow {
    fn key(&self) -> String {
        self.language.clone()
    }
    fn sig(&self) -> String {
        format!("{}|{}", self.files, self.lines)
    }
    fn search(&self) -> String {
        self.language.clone()
    }
    fn sort_columns() -> &'static [&'static str] {
        &["order", "language", "files", "lines"]
    }
    fn compare(a: &Self, b: &Self, col: usize) -> Ordering {
        match col {
            1 => a.language.cmp(&b.language),
            2 => a.files.cmp(&b.files),
            3 => a.lines.cmp(&b.lines),
            _ => Ordering::Equal,
        }
    }
}

/// Resolve `rows` into the indices to display, applying `filter` then `sort`.
///
/// With a filter and no explicit sort, results are ranked best-match-first —
/// what a fuzzy search is for. With an explicit sort the table's order wins, so
/// "sort by CPU, then narrow to sovra" behaves the way it reads.
pub fn prepare<T: TableRow>(
    rows: &[T],
    filter: &str,
    sort: Option<(usize, bool)>,
    fz: &mut Fuzzy,
) -> Vec<usize> {
    let mut scored: Vec<(usize, u32)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| fz.score(filter, &r.search()).map(|s| (i, s)))
        .collect();

    match sort {
        Some((col, desc)) if col != 0 => {
            scored.sort_by(|&(a, _), &(b, _)| {
                let o = T::compare(&rows[a], &rows[b], col);
                if desc {
                    o.reverse()
                } else {
                    o
                }
            });
        }
        _ if !filter.is_empty() => scored.sort_by_key(|&(_, score)| std::cmp::Reverse(score)),
        _ => {}
    }

    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(name: &str, ahead: u32, dirty: u32) -> Worktree {
        Worktree {
            name: name.into(),
            path: format!("/w/{name}"),
            branch: format!("feat/{name}"),
            ahead,
            dirty,
            committed: (0, 0),
            uncommitted: (0, 0),
            age: "1d".into(),
            ..Default::default()
        }
    }

    #[test]
    fn no_filter_no_sort_preserves_order() {
        let rows = vec![wt("b", 0, 0), wt("a", 0, 0)];
        let mut fz = Fuzzy::default();
        assert_eq!(prepare(&rows, "", None, &mut fz), vec![0, 1]);
    }

    #[test]
    fn sort_by_column_and_reverse() {
        let rows = vec![wt("b", 1, 0), wt("a", 5, 0)];
        let mut fz = Fuzzy::default();
        // by name ascending
        assert_eq!(prepare(&rows, "", Some((1, false)), &mut fz), vec![1, 0]);
        // by ahead descending
        assert_eq!(prepare(&rows, "", Some((2, true)), &mut fz), vec![1, 0]);
        // by ahead ascending
        assert_eq!(prepare(&rows, "", Some((2, false)), &mut fz), vec![0, 1]);
    }

    #[test]
    fn filter_narrows_rows() {
        let rows = vec![wt("alpha", 0, 0), wt("beta", 0, 0)];
        let mut fz = Fuzzy::default();
        assert_eq!(prepare(&rows, "alph", None, &mut fz), vec![0]);
        assert!(prepare(&rows, "zzzz", None, &mut fz).is_empty());
    }

    #[test]
    fn sort_column_zero_is_collector_order() {
        let rows = vec![wt("b", 0, 0), wt("a", 0, 0)];
        let mut fz = Fuzzy::default();
        assert_eq!(prepare(&rows, "", Some((0, true)), &mut fz), vec![0, 1]);
    }

    #[test]
    fn keys_are_stable_and_sigs_track_change() {
        let a = wt("x", 0, 0);
        let mut b = a.clone();
        assert_eq!(a.key(), b.key());
        assert_eq!(a.sig(), b.sig());
        b.dirty = 3;
        assert_eq!(a.key(), b.key(), "identity must survive a state change");
        assert_ne!(a.sig(), b.sig());
    }

    #[test]
    fn endpoints_sort_unreachable_last() {
        let ep = |label: &str, lat: Option<u64>| Endpoint {
            label: label.into(),
            host: "127.0.0.1".into(),
            port: 80,
            up: lat.is_some(),
            latency_ms: lat,
            pid: None,
        };
        let rows = vec![ep("down", None), ep("fast", Some(2))];
        let mut fz = Fuzzy::default();
        assert_eq!(prepare(&rows, "", Some((3, false)), &mut fz), vec![1, 0]);
    }
}
