//! Full-screen detail view for a single worktree: activity, changed files, commits, merge status.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::git_detail::{DiffMode, FileChange, MergeStatus, WorktreeDetail};
use crate::collect::jobs::Job;
use crate::theme::Theme;

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
pub fn render(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    detail: &WorktreeDetail,
    jobs: &[Job],
    theme: &Theme,
    file_state: &mut TableState,
    _now: i64,
) {
    // Top: 4-line activity + merge block; bottom: files (left) + commits (right).
    let vert = Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).split(area);
    let horiz =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(vert[1]);

    render_activity(f, vert[0], detail, jobs, theme);
    render_files(f, horiz[0], detail, theme, file_state);
    render_commits(f, horiz[1], detail, theme);
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
    let owning_job = jobs.iter().find(|j| {
        j.worktree_path.as_deref() == Some(detail.path.as_str())
            || j.worktree_branch.as_deref() == Some(detail.branch.as_str())
    });
    let activity_line = match owning_job {
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

    // Merge verdict line.
    let merge_line = match &detail.merge {
        MergeStatus::UpToDate => Line::styled("up to date with main", theme.dim_style()),
        MergeStatus::Clean => Line::styled("clean — safe to merge", Style::new().fg(theme.ok)),
        MergeStatus::Behind(n) => Line::styled(
            format!("behind main by {} — rebase before merge", n),
            Style::new().fg(theme.warn),
        ),
        MergeStatus::Conflicts(c) => Line::styled(
            format!("conflicts with main ({} files)", c.len()),
            Style::new().fg(theme.err),
        ),
    };

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
                DiffMode::Staged => ("S", Style::new().fg(theme.ok)),
                DiffMode::Unstaged => ("M", Style::new().fg(theme.warn)),
                DiffMode::Committed => ("C", theme.dim_style()),
            };
            Row::new(vec![
                Cell::from(marker).style(marker_style),
                Cell::from(fc.path.clone()),
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
    .row_highlight_style(Style::new().fg(theme.accent));

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
        ratatui::text::Text::styled("no commits ahead of main", theme.dim_style())
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
            }],
            uncommitted_files: vec![],
            commits: vec![CommitRow {
                short: "abc1234".into(),
                subject: "feat: add widget".into(),
                age: "2 hours ago".into(),
            }],
            merge: MergeStatus::Clean,
        }
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
