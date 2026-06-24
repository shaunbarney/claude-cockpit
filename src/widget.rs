//! Widget identity + focus order.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    Worktrees,
    Jobs,
    Cost,
    Activity,
    Code,
    Docker,
    Ports,
    Procs,
    Repo,
}

impl WidgetKind {
    pub fn all() -> &'static [WidgetKind] {
        use WidgetKind::*;
        &[
            Worktrees, Jobs, Cost, Activity, Code, Docker, Ports, Procs, Repo,
        ]
    }
    #[allow(dead_code)]
    pub fn title(self) -> &'static str {
        match self {
            WidgetKind::Worktrees => "Worktrees",
            WidgetKind::Jobs => "Jobs",
            WidgetKind::Cost => "Cost",
            WidgetKind::Activity => "Rate",
            WidgetKind::Code => "Code",
            WidgetKind::Docker => "Docker",
            WidgetKind::Ports => "Ports",
            WidgetKind::Procs => "Tools",
            WidgetKind::Repo => "Repo",
        }
    }
    pub fn next(self) -> WidgetKind {
        let all = WidgetKind::all();
        let i = all.iter().position(|&k| k == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }
    pub fn prev(self) -> WidgetKind {
        let all = WidgetKind::all();
        let i = all.iter().position(|&k| k == self).unwrap_or(0);
        all[(i + all.len() - 1) % all.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_cycles_all() {
        let all = WidgetKind::all();
        assert_eq!(all.first().copied(), Some(WidgetKind::Worktrees));
        let mut k = WidgetKind::Worktrees;
        for _ in 0..all.len() {
            k = k.next();
        }
        assert_eq!(k, WidgetKind::Worktrees);
    }
    #[test]
    fn titles_present() {
        assert_eq!(WidgetKind::Cost.title(), "Cost");
    }
}
