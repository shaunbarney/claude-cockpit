//! Full-screen detail view for a single worktree: activity, changed files, commits, merge status.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::git_detail::{DiffMode, FileChange, MergeStatus, WorktreeDetail};
use crate::collect::jobs::Job;
use crate::theme::{Level, Theme};

/// Build the combined file list in render order: uncommitted (staged then unstaged) then committed.
fn combined_files(d: &WorktreeDetail) -> Vec<(&FileChange, DiffMode)> {
    let mut v: Vec<(&FileChange, DiffMode)> = Vec::new();
    for f in &d.uncommitted_files {
        v.push((
            f,
            if f.staged {
                DiffMode::Staged
            } else {
                DiffMode::Unstaged
            },
        ));
    }
    for f in &d.committed_files {
        v.push((f, DiffMode::Committed));
    }
    v
}

/// Render the full-screen worktree detail view.
///
/// Returns the changed-files table's inner rect (no header row), so mouse
/// clicks can be mapped onto file rows.
pub fn render(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    detail: &WorktreeDetail,
    jobs: &[Job],
    theme: &Theme,
    file_state: &mut TableState,
    _now: i64,
) -> ratatui::layout::Rect {
    // Top: 4-line activity + merge block; bottom: files (left) + commits (right).
    let vert = Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).split(area);
    let horiz =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(vert[1]);

    render_activity(f, vert[0], detail, jobs, theme);
    render_files(f, horiz[0], detail, theme, file_state);
    render_commits(f, horiz[1], detail, theme);
    Block::default().borders(Borders::ALL).inner(horiz[0])
}

/// The job working in this worktree: matched by path (branch names are reused
/// across worktrees and say nothing), preferring a live job over a finished one.
fn owning_job<'a>(jobs: &'a [Job], path: &str) -> Option<&'a Job> {
    let mut mine = jobs
        .iter()
        .filter(|j| j.worktree_path.as_deref() == Some(path));
    let first = mine.clone().next();
    mine.find(|j| !matches!(j.state.as_str(), "done" | "failed" | "error" | "cancelled"))
        .or(first)
}

/// The ref merges are judged against, falling back to "main" when unknown.
fn base_label(detail: &WorktreeDetail) -> &str {
    if detail.base.is_empty() {
        "main"
    } else {
        &detail.base
    }
}

/// Human verdict and severity for a merge status against `base`.
pub fn merge_text(m: &MergeStatus, base: &str) -> (Level, String) {
    match m {
        MergeStatus::Clean => (Level::Ok, format!("merges cleanly into {base}")),
        MergeStatus::UpToDate => (Level::Idle, format!("identical to {base}")),
        MergeStatus::NothingToMerge(n) => (
            Level::Idle,
            format!("no commits of its own; {n} behind {base}"),
        ),
        MergeStatus::Behind(n) => (
            Level::Info,
            format!("merges cleanly; {base} moved {n} commits — consider rebasing"),
        ),
        MergeStatus::Dirty(n) => (
            Level::Warn,
            format!("{n} uncommitted changes — commit first"),
        ),
        MergeStatus::Conflicts(files) => {
            let shown: Vec<&str> = files.iter().take(3).map(String::as_str).collect();
            let more = files.len().saturating_sub(shown.len());
            let tail = if more > 0 {
                format!(" +{more}")
            } else {
                String::new()
            };
            (
                Level::Err,
                format!(
                    "conflicts with {base} in {} files: {}{tail}",
                    files.len(),
                    shown.join(", ")
                ),
            )
        }
        MergeStatus::Unknown => (
            Level::Idle,
            "couldn't check (git ≥ 2.38 needed for merge-tree)".to_string(),
        ),
    }
}

/// Display label for a changed file: `old → new` for renames.
fn file_label(fc: &FileChange) -> String {
    match &fc.old_path {
        Some(old) => format!("{old} → {}", fc.path),
        None => fc.path.clone(),
    }
}

fn render_activity(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    detail: &WorktreeDetail,
    jobs: &[Job],
    theme: &Theme,
) {
    let title = format!(" {} ", detail.name);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(theme.title());
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Activity line: find the job owning this worktree.
    let activity_line = match owning_job(jobs, &detail.path) {
        Some(j) => {
            let state_style = match j.state.as_str() {
                "working" => Style::new().fg(theme.ok),
                "blocked" => Style::new().fg(theme.warn),
                _ => Style::new().fg(theme.accent),
            };
            Line::from(vec![
                Span::raw(format!("{} · ", j.name)),
                Span::styled(j.state.clone(), state_style),
                Span::raw(format!(" · {}", j.intent)),
            ])
        }
        None => Line::styled("no active job", theme.dim_style()),
    };

    let (level, msg) = merge_text(&detail.merge, base_label(detail));
    let merge_line = Line::from(vec![
        theme.level_span(level),
        Span::raw(" "),
        Span::styled(msg, Style::new().fg(theme.level_color(level))),
    ]);

    let text = ratatui::text::Text::from(vec![activity_line, merge_line]);
    f.render_widget(Paragraph::new(text), inner);
}

fn render_files(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    detail: &WorktreeDetail,
    theme: &Theme,
    file_state: &mut TableState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Changed files ")
        .title_style(theme.title());

    let files = combined_files(detail);
    let rows: Vec<Row> = files
        .iter()
        .map(|(fc, mode)| {
            let (marker, marker_style) = match mode {
                _ if fc.untracked => ("?", Style::new().fg(theme.info)),
                DiffMode::Staged => ("S", Style::new().fg(theme.ok)),
                DiffMode::Unstaged => ("M", Style::new().fg(theme.warn)),
                DiffMode::Committed => ("C", theme.dim_style()),
            };
            let mut name = vec![Span::raw(file_label(fc))];
            if fc.untracked {
                name.push(Span::styled(" untracked", theme.dim_style()));
            }
            Row::new(vec![
                Cell::from(marker).style(marker_style),
                Cell::from(Line::from(name)),
                Cell::from(format!("+{}", fc.added)).style(Style::new().fg(theme.ok)),
                Cell::from(format!("-{}", fc.deleted)).style(Style::new().fg(theme.err)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .block(block)
    .row_highlight_style(
        Style::new()
            .fg(theme.accent)
            .add_modifier(ratatui::style::Modifier::REVERSED),
    );

    f.render_stateful_widget(table, area, file_state);
}

fn render_commits(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    detail: &WorktreeDetail,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Commits ")
        .title_style(theme.title());

    let lines: Vec<Line> = detail
        .commits
        .iter()
        .map(|c| {
            Line::from(vec![
                Span::styled(format!("{} ", c.short), theme.dim_style()),
                Span::raw(c.subject.clone()),
                Span::raw("  "),
                Span::styled(c.age.clone(), theme.dim_style()),
            ])
        })
        .collect();

    let text = if lines.is_empty() {
        ratatui::text::Text::styled(
            format!("no commits ahead of {}", base_label(detail)),
            theme.dim_style(),
        )
    } else {
        ratatui::text::Text::from(lines)
    };

    f.render_widget(Paragraph::new(text).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::git_detail::{CommitRow, FileChange, MergeStatus, WorktreeDetail};
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn mk_detail() -> WorktreeDetail {
        WorktreeDetail {
            name: "my-feature".into(),
            branch: "feature/test".into(),
            path: "/tmp/test".into(),
            committed_files: vec![FileChange {
                path: "src/main.rs".into(),
                added: 10,
                deleted: 2,
                staged: false,
                ..Default::default()
            }],
            uncommitted_files: vec![],
            commits: vec![CommitRow {
                short: "abc1234".into(),
                subject: "feat: add widget".into(),
                age: "2 hours ago".into(),
            }],
            merge: MergeStatus::Clean,
            ..Default::default()
        }
    }

    fn job(path: &str, state: &str, name: &str) -> Job {
        Job {
            id: name.into(),
            name: name.into(),
            state: state.into(),
            tempo: "idle".into(),
            intent: String::new(),
            tasks: 0,
            queued: 0,
            cwd: String::new(),
            worktree_path: Some(path.into()),
            worktree_branch: Some("main".into()),
            created_at: None,
            updated_at: None,
            ..Default::default()
        }
    }

    #[test]
    fn owning_job_matches_path_and_prefers_live_jobs() {
        let jobs = vec![
            job("/w/a", "done", "old"),
            job("/w/b", "working", "other"),
            job("/w/a", "working", "live"),
        ];
        assert_eq!(
            owning_job(&jobs, "/w/a").map(|j| j.name.as_str()),
            Some("live")
        );
        // A shared branch name is not ownership.
        assert!(owning_job(&jobs, "/w/c").is_none());
        let done_only = vec![job("/w/a", "done", "old")];
        assert_eq!(
            owning_job(&done_only, "/w/a").map(|j| j.name.as_str()),
            Some("old")
        );
    }

    #[test]
    fn merge_text_names_the_base_and_severity() {
        let base = "origin/main";
        let (l, t) = merge_text(&MergeStatus::Behind(4), base);
        assert_eq!(l, Level::Info);
        assert_eq!(
            t,
            "merges cleanly; origin/main moved 4 commits — consider rebasing"
        );
        let (_, t) = merge_text(&MergeStatus::NothingToMerge(2), base);
        assert_eq!(t, "no commits of its own; 2 behind origin/main");
        let (l, t) = merge_text(&MergeStatus::Dirty(3), base);
        assert_eq!(
            (l, t.as_str()),
            (Level::Warn, "3 uncommitted changes — commit first")
        );
        let (l, t) = merge_text(&MergeStatus::Unknown, base);
        assert_eq!(l, Level::Idle);
        assert!(t.contains("2.38"));
        let files: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let (l, t) = merge_text(&MergeStatus::Conflicts(files), base);
        assert_eq!(l, Level::Err);
        assert_eq!(t, "conflicts with origin/main in 4 files: a, b, c +1");
        assert_eq!(merge_text(&MergeStatus::Clean, base).0, Level::Ok);
        assert_eq!(merge_text(&MergeStatus::UpToDate, base).0, Level::Idle);
    }

    #[test]
    fn renames_and_untracked_files_are_labelled() {
        let mut d = mk_detail();
        d.base = "origin/dev".into();
        d.merge = MergeStatus::Behind(2);
        d.uncommitted_files = vec![
            FileChange {
                path: "new.rs".into(),
                old_path: Some("old.rs".into()),
                staged: true,
                ..Default::default()
            },
            FileChange {
                path: "scratch.txt".into(),
                untracked: true,
                ..Default::default()
            },
        ];
        let mut st = TableState::default();
        let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
        term.draw(|f| {
            render(f, f.area(), &d, &[], &Theme::default(), &mut st, 0);
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("old.rs → new.rs"));
        assert!(s.contains("scratch.txt untracked"));
        assert!(s.contains("origin/dev moved 2 commits"));
    }

    #[test]
    fn renders_worktree_detail() {
        let app_detail = mk_detail();
        let theme = Theme::default();
        let mut file_state = TableState::default();
        file_state.select(Some(0));

        let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
        term.draw(|f| {
            render(f, f.area(), &app_detail, &[], &theme, &mut file_state, 0);
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("src/main.rs"), "buffer should contain file path");
        assert!(
            s.contains("Commits"),
            "buffer should contain Commits header"
        );
    }
}
