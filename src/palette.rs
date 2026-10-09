//! The command palette: every action the cockpit can take, in one searchable list.
//!
//! Keyboard vocabularies grow faster than anyone's memory of them. A palette
//! turns discovery into typing — `:dock<Enter>` gets you to Docker without
//! knowing that Docker is the sixth Tab stop — and it doubles as the honest
//! answer to "what can this thing actually do?", because the registry *is* the
//! feature list rather than a help screen that drifts out of date.

use crate::fuzzy::Fuzzy;
use crate::widget::WidgetKind;

/// An action the palette can dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Focus a widget on the dashboard.
    Focus(WidgetKind),
    /// Expand the focused widget to full screen.
    Expand,
    /// Drill into the focused widget's selection.
    Drill,
    /// Start a filter on the focused widget.
    Filter,
    /// Drop the focused widget's filter.
    ClearFilter,
    /// Sort the focused widget by a column index.
    Sort(usize),
    /// Re-run the collectors now.
    Refresh,
    /// Open the project picker.
    Projects,
    /// Switch to the `n`th entry of `App::project_list`.
    SwitchProject(usize),
    /// Toggle the key help.
    Help,
    Quit,
}

/// One palette row.
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    /// Right-aligned hint: the key that does the same thing, or context.
    pub hint: String,
    pub cmd: Command,
}

impl Entry {
    fn new(label: impl Into<String>, hint: impl Into<String>, cmd: Command) -> Self {
        Entry {
            label: label.into(),
            hint: hint.into(),
            cmd,
        }
    }
}

/// Every command available right now.
///
/// The sort entries are built from the focused widget's own columns, so the
/// palette offers "Sort by cpu" on Docker and "Sort by uses" on Skills rather
/// than a generic list that is wrong most of the time. `table_ops` says whether
/// filter and sort can apply at all — false on a chart widget or a detail
/// screen, where offering them would promise something that cannot happen.
/// "Switch to …" entries for registered projects, so `:sovra<Enter>` jumps
/// straight there; plus the picker itself.
pub fn project_entries(list: &[crate::projects::host::ListedProject]) -> Vec<Entry> {
    let mut out = vec![Entry::new(
        "Projects — switch, add, manage",
        "P",
        Command::Projects,
    )];
    let mut hot = 0;
    for (i, p) in list.iter().enumerate() {
        let hint = if p.pinned && hot < 9 {
            hot += 1;
            hot.to_string()
        } else {
            String::new()
        };
        out.push(Entry::new(
            format!("Switch to project {}", p.name),
            hint,
            Command::SwitchProject(i),
        ));
    }
    out
}

pub fn registry(focus: WidgetKind, table_ops: bool) -> Vec<Entry> {
    let mut out: Vec<Entry> = WidgetKind::all()
        .iter()
        .map(|&k| Entry::new(format!("Go to {}", k.title()), "", Command::Focus(k)))
        .collect();

    out.push(Entry::new("Expand focused widget", "e", Command::Expand));
    out.push(Entry::new("Open selected row", "Enter", Command::Drill));
    if table_ops {
        out.push(Entry::new("Filter rows", "/", Command::Filter));
        out.push(Entry::new("Clear filter", "Esc", Command::ClearFilter));
        for (i, col) in crate::view::sort_columns(focus).iter().enumerate() {
            let label = if i == 0 {
                format!("Sort {} by default order", focus.title())
            } else {
                format!("Sort {} by {}", focus.title(), col)
            };
            out.push(Entry::new(label, "s", Command::Sort(i)));
        }
    }

    out.push(Entry::new("Refresh now", "r", Command::Refresh));
    out.push(Entry::new("Toggle key help", "?", Command::Help));
    out.push(Entry::new("Quit", "q", Command::Quit));
    out
}

/// Entries matching `needle`, best first. An empty needle keeps registry order.
pub fn search(entries: &[Entry], needle: &str, fz: &mut Fuzzy) -> Vec<usize> {
    let mut scored: Vec<(usize, u32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| fz.score(needle, &e.label).map(|s| (i, s)))
        .collect();
    if !needle.is_empty() {
        scored.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
    }
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_every_widget() {
        let r = registry(WidgetKind::Jobs, true);
        for &k in WidgetKind::all() {
            assert!(
                r.iter().any(|e| e.cmd == Command::Focus(k)),
                "{k:?} unreachable from the palette"
            );
        }
        assert!(r.iter().any(|e| e.cmd == Command::Quit));
    }

    #[test]
    fn sort_entries_follow_the_focused_widget() {
        let docker = registry(WidgetKind::Docker, true);
        assert!(docker.iter().any(|e| e.label.contains("by cpu")));
        let skills = registry(WidgetKind::Repo, true);
        assert!(skills.iter().any(|e| e.label.contains("by uses")));
        assert!(!skills.iter().any(|e| e.label.contains("by cpu")));
    }

    #[test]
    fn table_commands_only_offered_where_they_apply() {
        let cost = registry(WidgetKind::Cost, false);
        assert!(!cost.iter().any(|e| e.cmd == Command::Filter));
        assert!(!cost.iter().any(|e| matches!(e.cmd, Command::Sort(_))));
        assert!(!cost.iter().any(|e| e.cmd == Command::ClearFilter));
        // Navigation stays available everywhere.
        assert!(cost
            .iter()
            .any(|e| e.cmd == Command::Focus(WidgetKind::Jobs)));
        let jobs = registry(WidgetKind::Jobs, true);
        assert!(jobs.iter().any(|e| e.cmd == Command::Filter));
    }

    #[test]
    fn search_ranks_and_narrows() {
        let r = registry(WidgetKind::Jobs, true);
        let mut fz = Fuzzy::default();

        // Empty needle keeps everything, in order.
        assert_eq!(search(&r, "", &mut fz).len(), r.len());

        let hits = search(&r, "dock", &mut fz);
        assert!(!hits.is_empty());
        assert_eq!(r[hits[0]].cmd, Command::Focus(WidgetKind::Docker));

        assert!(search(&r, "zzzzzzq", &mut fz).is_empty());
    }

    #[test]
    fn abbreviations_find_commands() {
        let r = registry(WidgetKind::Jobs, true);
        let mut fz = Fuzzy::default();
        let hits = search(&r, "refr", &mut fz);
        assert_eq!(r[hits[0]].cmd, Command::Refresh);
    }
}
