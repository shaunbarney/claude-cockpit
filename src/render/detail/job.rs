//! Full-screen detail view for a single Claude Code background job.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use crate::collect::jobs::{Job, JobEvent};
use crate::theme::Theme;
use crate::util::human_duration;

/// Colour a state string using theme semantics.
fn state_style(state: &str, theme: &Theme) -> Style {
    match state {
        "working" => Style::new().fg(theme.ok),
        "blocked" => Style::new().fg(theme.warn),
        _ => theme.dim_style(),
    }
}

/// Render the full-screen job detail view.
///
/// Layout: 7-line header block (bordered) + body timeline block (rest).
pub fn render(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    job: &Job,
    events: &[JobEvent],
    theme: &Theme,
    now: i64,
    scroll: u16,
) {
    let chunks = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);

    render_header(f, chunks[0], job, theme, now);
    render_timeline(f, chunks[1], events, theme, scroll);
}

fn render_header(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    job: &Job,
    theme: &Theme,
    now: i64,
) {
    let title = format!(" Job · {} ", job.name);
    let block = Block::default().borders(Borders::ALL).title(title).title_style(theme.title());

    let state_sp = Span::styled(job.state.clone(), state_style(&job.state, theme));
    let tempo_sp = Span::styled(job.tempo.clone(), theme.dim_style());

    let branch_val = job.worktree_branch.as_deref().unwrap_or("—");

    let duration_val = job
        .created_at
        .map(|c| human_duration((now - c).max(0) as u64))
        .unwrap_or_else(|| "—".to_string());
    let updated_val = job
        .updated_at
        .map(|u| format!("{} ago", human_duration((now - u).max(0) as u64)))
        .unwrap_or_else(|| "—".to_string());

    // Truncate intent for header; full text is visible in intent field anyway.
    let intent_str = if job.intent.chars().count() > 80 {
        let cut: String = job.intent.chars().take(79).collect();
        format!("{cut}…")
    } else {
        job.intent.clone()
    };

    let lines: Vec<Line> = vec![
        Line::from(vec![
            Span::raw("State: "),
            state_sp,
            Span::raw("  Tempo: "),
            tempo_sp,
        ]),
        Line::from(vec![Span::raw("Branch: "), Span::raw(branch_val)]),
        Line::from(vec![
            Span::raw("Duration: "),
            Span::raw(duration_val),
            Span::raw("   Updated: "),
            Span::styled(updated_val, theme.dim_style()),
        ]),
        Line::from(vec![Span::raw("Intent: "), Span::raw(intent_str)]),
    ];

    let p = Paragraph::new(Text::from(lines)).block(block);
    f.render_widget(p, area);
}

fn render_timeline(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    events: &[JobEvent],
    theme: &Theme,
    scroll: u16,
) {
    let n = events.len();
    let title = format!(" Timeline ({n} events) ");
    let block = Block::default().borders(Borders::ALL).title(title).title_style(theme.title());

    if events.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "no timeline events",
            theme.dim_style(),
        )))
        .block(block);
        f.render_widget(p, area);
        return;
    }

    // Build lines: each event on its own line.
    let max_width = area.width.saturating_sub(4) as usize; // borders + scrollbar
    let lines: Vec<Line> = events
        .iter()
        .map(|ev| {
            let tag = format!("[{}]", ev.state);
            let tag_sp = Span::styled(tag, state_style(&ev.state, theme));

            let body = if !ev.text.is_empty() && ev.text != ev.detail {
                format!(" {} — {}", ev.detail, ev.text)
            } else {
                format!(" {}", ev.detail)
            };

            // Truncate long body to avoid wrapping.
            let body_trimmed = if body.chars().count() > max_width.saturating_sub(10) {
                let cut: String = body.chars().take(max_width.saturating_sub(11)).collect();
                format!("{cut}…")
            } else {
                body
            };

            Line::from(vec![tag_sp, Span::raw(body_trimmed)])
        })
        .collect();

    let inner = block.inner(area);
    let p = Paragraph::new(Text::from(lines)).block(block).scroll((scroll, 0));
    f.render_widget(p, area);

    // Scrollbar on the right side.
    let mut sb_state = ScrollbarState::new(n).position(scroll as usize);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    f.render_stateful_widget(scrollbar, inner, &mut sb_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::jobs::{Job, JobEvent};
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn mk_job(name: &str) -> Job {
        Job {
            id: "test-job-001".into(),
            name: name.into(),
            state: "working".into(),
            tempo: "active".into(),
            intent: "Implement the governour cockpit detail view".into(),
            tasks: 3,
            queued: 1,
            cwd: "/repo".into(),
            worktree_path: Some("/repo/.claude/worktrees/governour-cockpit".into()),
            worktree_branch: Some("worktree-governour-cockpit".into()),
            created_at: Some(1_000_000),
            updated_at: Some(1_000_060),
        }
    }

    fn mk_event(state: &str, detail: &str, text: &str) -> JobEvent {
        JobEvent {
            at: 1_000_000,
            state: state.into(),
            detail: detail.into(),
            text: text.into(),
        }
    }

    #[test]
    fn renders_job_detail_with_events() {
        let job = mk_job("GovernourCockpit");
        let events = vec![
            mk_event("working", "starting build", "cargo build started"),
            mk_event("working", "tests passing", "27 tests passed"),
        ];
        let theme = Theme::default();

        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| {
            render(
                f,
                ratatui::layout::Rect { x: 0, y: 0, width: 120, height: 24 },
                &job,
                &events,
                &theme,
                1_001_000,
                0,
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        assert!(s.contains("GovernourCockpit"), "expected job name in buffer");
        assert!(s.contains("starting build"), "expected first event detail in buffer");
        assert!(s.contains("tests passing"), "expected second event detail in buffer");
    }
}
