//! The persistent header bar: where you are, and the four numbers worth
//! knowing before you look at anything else.
//!
//! Without it the dashboard is nine boxes with no centre — you have to read a
//! whole widget to learn something a word could have told you. The bar answers
//! "am I burning money / near a limit / is anything stuck" at a glance, and
//! carries the live pulse that shows the cockpit is actually watching rather
//! than displaying a frozen snapshot.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::DashboardData;
use crate::theme::{Level, Theme};

/// The headline numbers, already reduced from the raw datasets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kpis {
    pub jobs_working: usize,
    pub jobs_blocked: usize,
    pub jobs_total: usize,
    pub worktrees_dirty: usize,
    pub worktrees_total: usize,
    pub spend_today: f64,
    /// Percent of the 5-hour prompt allowance used, when a cap is known.
    pub rate_pct: Option<u64>,
    pub containers_up: usize,
    pub containers_total: usize,
    /// Collector failures + config diagnostics (filled in by the caller).
    pub problems: usize,
}

/// Reduce a dashboard snapshot to its headline numbers.
pub fn kpis(d: &DashboardData, today: &str) -> Kpis {
    let jobs_working = d.jobs.iter().filter(|j| j.state == "working").count();
    let jobs_blocked = d.jobs.iter().filter(|j| j.state == "blocked").count();
    let worktrees_dirty = d.worktrees.iter().filter(|w| w.dirty > 0).count();

    let spend_today = d
        .usage
        .as_ref()
        .and_then(|u| u.by_day.iter().find(|x| x.day == today))
        .map(|x| x.cost_usd)
        .unwrap_or(0.0);

    // Only meaningful against a real cap; an auto-scaled cap is a guess at the
    // user's own peak, not a limit, so it would read as false precision.
    let rate_pct = d.usage.as_ref().and_then(|u| {
        let r = &u.rate;
        (!r.auto_prompts_5h && r.cap_prompts_5h > 0)
            .then(|| (r.prompts_5h * 100 / r.cap_prompts_5h).min(999))
    });

    // Only this project's containers: the Docker widget's "show everything"
    // fallback is a courtesy, not something to report as the project's state.
    let mine: Vec<_> = d.containers.iter().filter(|c| c.in_repo).collect();
    let containers_up = mine.iter().filter(|c| c.state == "running").count();

    Kpis {
        jobs_working,
        jobs_blocked,
        // Synthetic "no agent" worktree rows are not agents.
        jobs_total: d.jobs.iter().filter(|j| j.state != "no agent").count(),
        worktrees_dirty,
        worktrees_total: d.worktrees.len(),
        spend_today,
        rate_pct,
        containers_up,
        containers_total: mine.len(),
        problems: 0,
    }
}

/// One header segment, in drop order: the last ones go first when space runs out.
struct Seg {
    spans: Vec<Span<'static>>,
    width: usize,
}

fn seg(spans: Vec<Span<'static>>) -> Seg {
    let width = spans.iter().map(|s| s.content.chars().count()).sum();
    Seg { spans, width }
}

/// Level for a count where any nonzero value is worth noticing.
fn nonzero_level(n: usize, warn: Level) -> Level {
    if n > 0 {
        warn
    } else {
        Level::Idle
    }
}

/// Build the header segments for `k`, most important first.
fn segments(k: &Kpis, theme: &Theme) -> Vec<Seg> {
    let mut out = Vec::new();

    if k.jobs_total > 0 {
        let lvl = if k.jobs_blocked > 0 {
            Level::Warn
        } else if k.jobs_working > 0 {
            Level::Active
        } else {
            Level::Idle
        };
        let text = if k.jobs_blocked > 0 {
            format!("{} working · {} blocked", k.jobs_working, k.jobs_blocked)
        } else {
            format!("{}/{} agents working", k.jobs_working, k.jobs_total)
        };
        out.push(seg(vec![
            theme.level_span(lvl),
            Span::raw(" "),
            Span::styled(text, Style::new().fg(theme.level_color(lvl))),
        ]));
    }

    if k.spend_today > 0.0 {
        out.push(seg(vec![
            Span::styled(
                format!("${:.2}", k.spend_today),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" today", theme.muted_style()),
        ]));
    }

    if let Some(pct) = k.rate_pct {
        let lvl = match pct {
            0..=69 => Level::Ok,
            70..=89 => Level::Warn,
            _ => Level::Err,
        };
        out.push(seg(vec![
            Span::styled(format!("{pct}%"), Style::new().fg(theme.level_color(lvl))),
            Span::styled(" of 5h limit", theme.muted_style()),
        ]));
    }

    if k.worktrees_total > 0 {
        let lvl = nonzero_level(k.worktrees_dirty, Level::Warn);
        out.push(seg(vec![
            Span::styled(
                format!("{}", k.worktrees_dirty),
                Style::new().fg(theme.level_color(lvl)),
            ),
            Span::styled(format!("/{} dirty", k.worktrees_total), theme.muted_style()),
        ]));
    }

    if k.containers_total > 0 {
        let down = k.containers_total - k.containers_up;
        let lvl = nonzero_level(down, Level::Err);
        out.push(seg(vec![
            Span::styled(
                format!("{}/{}", k.containers_up, k.containers_total),
                Style::new().fg(theme.level_color(lvl)),
            ),
            Span::styled(" containers", theme.muted_style()),
        ]));
    }

    if k.problems > 0 {
        out.push(seg(vec![
            Span::styled(format!("⚠ {}", k.problems), Style::new().fg(theme.warn)),
            Span::styled(
                if k.problems == 1 {
                    " problem"
                } else {
                    " problems"
                },
                theme.muted_style(),
            ),
        ]));
    }

    out
}

/// Render the header into a single-line `area`.
///
/// Segments are dropped from the right as the terminal narrows, so the bar
/// degrades instead of wrapping or truncating mid-word.
pub fn render(
    f: &mut Frame,
    area: Rect,
    repo: &crate::collect::git::RepoInfo,
    k: &Kpis,
    theme: &Theme,
    pulse: &str,
    busy: bool,
) {
    if area.height == 0 {
        return;
    }
    let mut spans: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(
            pulse.to_string(),
            Style::new().fg(if busy { theme.accent } else { theme.ok }),
        ),
        Span::raw(" "),
    ];

    let name = if repo.name.is_empty() {
        "claude-cockpit".to_string()
    } else {
        repo.name.clone()
    };
    spans.push(Span::styled(
        name,
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    ));
    if !repo.branch.is_empty() && repo.branch != "HEAD" {
        spans.push(Span::styled(
            format!("  {}", repo.branch),
            theme.muted_style(),
        ));
    }

    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let mut left = (area.width as usize).saturating_sub(used);

    for s in segments(k, theme) {
        // 3 for the "  ·  " style separator.
        if s.width + 5 > left {
            break;
        }
        left -= s.width + 5;
        spans.push(Span::styled("  ·  ", theme.dim_style()));
        spans.extend(s.spans);
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::docker::Container;
    use crate::collect::git::Worktree;
    use crate::collect::jobs::Job;
    use crate::collect::usage::{DayUsage, RateStats, UsageTotals};
    use crate::theme::Depth;
    use ratatui::{backend::TestBackend, Terminal};

    fn job(state: &str) -> Job {
        Job {
            id: "i".into(),
            name: "n".into(),
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

    fn wt(dirty: u32) -> Worktree {
        Worktree {
            name: "w".into(),
            path: format!("/w/{dirty}"),
            branch: "b".into(),
            ahead: 0,
            dirty,
            committed: (0, 0),
            uncommitted: (0, 0),
            age: "1d".into(),
            ..Default::default()
        }
    }

    fn data() -> DashboardData {
        DashboardData {
            jobs: vec![job("working"), job("blocked"), job("done")],
            worktrees: vec![wt(0), wt(2)],
            containers: vec![
                Container {
                    id: "a".into(),
                    name: "a".into(),
                    state: "running".into(),
                    in_repo: true,
                    status: String::new(),
                    health: None,
                    cpu_pct: 0.0,
                    mem_used: 0,
                    mem_limit: 0,
                    ports: vec![],
                    ..Container::default()
                },
                Container {
                    id: "b".into(),
                    name: "b".into(),
                    state: "exited".into(),
                    in_repo: true,
                    status: String::new(),
                    health: None,
                    cpu_pct: 0.0,
                    mem_used: 0,
                    mem_limit: 0,
                    ports: vec![],
                    ..Container::default()
                },
            ],
            usage: Some(UsageTotals {
                by_day: vec![DayUsage {
                    day: "2026-09-19".into(),
                    cost_usd: 12.5,
                    tokens: 10,
                }],
                rate: RateStats {
                    prompts_5h: 25,
                    cap_prompts_5h: 100,
                    auto_prompts_5h: false,
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn other_projects_containers_and_orphan_rows_are_not_counted() {
        let mut d = data();
        for c in d.containers.iter_mut() {
            c.in_repo = false;
        }
        d.jobs.push(Job {
            state: "no agent".into(),
            ..job("idle")
        });
        let k = kpis(&d, "2026-09-19");
        assert_eq!((k.containers_up, k.containers_total), (0, 0));
        assert_eq!(k.jobs_total, 3);
    }

    #[test]
    fn reduces_a_snapshot_to_headline_numbers() {
        let k = kpis(&data(), "2026-09-19");
        assert_eq!(k.jobs_working, 1);
        assert_eq!(k.jobs_blocked, 1);
        assert_eq!(k.jobs_total, 3);
        assert_eq!(k.worktrees_dirty, 1);
        assert_eq!(k.spend_today, 12.5);
        assert_eq!(k.rate_pct, Some(25));
        assert_eq!(k.containers_up, 1);
        assert_eq!(k.containers_total, 2);
    }

    #[test]
    fn spend_is_zero_on_a_day_with_no_usage() {
        let k = kpis(&data(), "1999-01-01");
        assert_eq!(k.spend_today, 0.0);
    }

    #[test]
    fn auto_scaled_rate_cap_reports_no_percentage() {
        // An auto-scaled cap is the user's own observed peak, not a limit —
        // showing "80% of limit" against it would be a fabricated number.
        let mut d = data();
        d.usage.as_mut().unwrap().rate.auto_prompts_5h = true;
        assert_eq!(kpis(&d, "2026-09-19").rate_pct, None);
    }

    fn draw(width: u16) -> String {
        let d = data();
        let k = kpis(&d, "2026-09-19");
        let repo = crate::collect::git::RepoInfo {
            name: "claude-cockpit".into(),
            branch: "feat/dynamic-tui".into(),
        };
        let theme = Theme::new(Depth::True, None);
        let mut term = Terminal::new(TestBackend::new(width, 1)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height: 1,
                },
                &repo,
                &k,
                &theme,
                "●",
                false,
            )
        })
        .unwrap();
        term.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn problems_segment_only_when_there_are_problems() {
        let theme = Theme::default();
        let text = |k: &Kpis| -> String {
            segments(k, &theme)
                .iter()
                .flat_map(|s| s.spans.iter().map(|sp| sp.content.to_string()))
                .collect()
        };
        let mut k = Kpis::default();
        assert!(!text(&k).contains('⚠'));
        k.problems = 2;
        assert!(text(&k).contains("⚠ 2 problems"));
        k.problems = 1;
        assert!(text(&k).ends_with("⚠ 1 problem"));
    }

    #[test]
    fn wide_header_shows_repo_and_kpis() {
        let s = draw(200);
        assert!(s.contains("claude-cockpit"));
        assert!(s.contains("feat/dynamic-tui"));
        assert!(s.contains("blocked"));
        assert!(s.contains("$12.50"));
        assert!(s.contains("25%"));
    }

    #[test]
    fn narrow_header_drops_segments_instead_of_overflowing() {
        let s = draw(40);
        // Identity survives; the trailing KPIs are dropped rather than wrapped.
        assert!(s.contains("claude-cockpit"));
        assert!(!s.contains("containers"));
        assert_eq!(s.chars().count(), 40, "must not overflow its row");
    }
}
