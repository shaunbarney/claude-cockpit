//! Full-screen detail view for a single Claude Code background job.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use crate::collect::jobs::{is_error_state, Job, JobEvent};
use crate::theme::Theme;
use crate::util::{human_duration, worktree_label};

/// States after which a job does no more work.
fn is_terminal(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "done" | "completed" | "cancelled" | "canceled" | "stopped"
    ) || is_error_state(state)
}

/// Colour a state string using theme semantics.
fn state_style(state: &str, theme: &Theme) -> Style {
    match state {
        "working" => Style::new().fg(theme.ok),
        "blocked" => Style::new().fg(theme.warn),
        s if is_error_state(s) => Style::new().fg(theme.err),
        _ => theme.dim_style(),
    }
}

/// How long the job has run: up to now while live, and frozen at the moment
/// it finished (`last_terminal_at`, else its last update) once terminal.
pub fn job_duration(job: &Job, now: i64) -> Option<u64> {
    let start = job.created_at?;
    let end = if is_terminal(&job.state) {
        job.last_terminal_at.or(job.updated_at).unwrap_or(now)
    } else {
        now
    };
    Some((end - start).max(0) as u64)
}

/// Height of the header block for `job`: its content lines plus the border.
pub fn header_height(job: &Job) -> u16 {
    header_lines(job, &Theme::default(), 0).len() as u16 + 2
}

/// Render the full-screen job detail view.
///
/// Layout: a header block sized to its own content + the timeline taking the
/// rest.
pub fn render(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    job: &Job,
    events: &[JobEvent],
    theme: &Theme,
    now: i64,
    scroll: u16,
) {
    let chunks =
        Layout::vertical([Constraint::Length(header_height(job)), Constraint::Min(0)]).split(area);

    render_header(f, chunks[0], job, theme, now);
    render_timeline(f, chunks[1], events, theme, scroll);
}

/// A `Label: value` line, omitted (None) when the value is empty.
fn field(label: &str, value: &str, style: Style, theme: &Theme) -> Option<Line<'static>> {
    if value.trim().is_empty() {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(format!("{label}: "), theme.dim_style()),
        Span::styled(value.trim().to_string(), style),
    ]))
}

/// Header content. A blocked job leads with what it needs and the reply it
/// suggests, since answering it is the reason to open this view.
fn header_lines(job: &Job, theme: &Theme, now: i64) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if job.state == "blocked" {
        let warn = Style::new().fg(theme.warn).add_modifier(Modifier::BOLD);
        lines.extend(field("Needs", &job.needs, warn, theme));
        let reply = Style::new().fg(theme.accent);
        lines.extend(field("Suggested reply", &job.suggested_reply, reply, theme));
    }
    lines.push(Line::from(vec![
        Span::styled("State: ", theme.dim_style()),
        Span::styled(job.state.clone(), state_style(&job.state, theme)),
        Span::styled("  Tempo: ", theme.dim_style()),
        Span::styled(job.tempo.clone(), theme.dim_style()),
    ]));
    lines.extend(field("Doing", &job.detail, Style::new(), theme));
    let goal = if job.intent.trim().is_empty() {
        "—"
    } else {
        &job.intent
    };
    lines.extend(field("Goal", goal, Style::new(), theme));
    lines.push(Line::from(vec![
        Span::styled("Worktree: ", theme.dim_style()),
        Span::raw(worktree_label(
            job.worktree_branch.as_deref(),
            job.worktree_path.as_deref(),
        )),
    ]));
    if is_terminal(&job.state) {
        let style = if is_error_state(&job.state) {
            Style::new().fg(theme.err)
        } else {
            Style::new()
        };
        lines.extend(field("Result", &job.output_result, style, theme));
    }
    lines.push(timing_line(job, theme, now));
    lines
}

fn timing_line(job: &Job, theme: &Theme, now: i64) -> Line<'static> {
    let duration_val = job_duration(job, now)
        .map(human_duration)
        .unwrap_or_else(|| "—".to_string());
    let updated_val = job
        .updated_at
        .map(|u| format!("{} ago", human_duration((now - u).max(0) as u64)))
        .unwrap_or_else(|| "—".to_string());
    Line::from(vec![
        Span::styled("Duration: ", theme.dim_style()),
        Span::raw(duration_val),
        Span::styled("   Updated: ", theme.dim_style()),
        Span::styled(updated_val, theme.dim_style()),
    ])
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, job: &Job, theme: &Theme, now: i64) {
    let title = format!(" Job · {} ", job.name);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(theme.title());
    let p = Paragraph::new(Text::from(header_lines(job, theme, now))).block(block);
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
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(theme.title());

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
    // Scrolling past the last line just shows blank space, so stop there.
    let max_scroll = (n as u16).saturating_sub(inner.height);
    let scroll = scroll.min(max_scroll);

    let p = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, 0));
    f.render_widget(p, area);

    // A scrollbar over content that already fits is noise — and a full-height
    // thumb reads as "stuck", which is worse than no scrollbar at all.
    if max_scroll > 0 {
        let mut sb_state = ScrollbarState::new(n).position(scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        f.render_stateful_widget(scrollbar, inner, &mut sb_state);
    }
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
            worktree_branch: Some("feat/demo".into()),
            created_at: Some(1_000_000),
            updated_at: Some(1_000_060),
            ..Default::default()
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
                ratatui::layout::Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 24,
                },
                &job,
                &events,
                &theme,
                1_001_000,
                0,
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        assert!(
            s.contains("GovernourCockpit"),
            "expected job name in buffer"
        );
        assert!(
            s.contains("Worktree:"),
            "expected 'Worktree:' label in header"
        );
        assert!(
            s.contains("governour-cockpit"),
            "expected worktree folder name in header (folder wins over branch)"
        );
        assert!(
            s.contains("starting build"),
            "expected first event detail in buffer"
        );
        assert!(
            s.contains("tests passing"),
            "expected second event detail in buffer"
        );
        assert!(
            s.contains("Implement the governour cockpit detail view"),
            "the intent is the one field saying what the agent is doing"
        );
    }

    #[test]
    fn no_scrollbar_when_the_timeline_already_fits() {
        let job = mk_job("Job");
        let events = vec![mk_event("working", "one thing", "")];
        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            render(f, f.area(), &job, &events, &theme, 1_001_000, 0);
        })
        .unwrap();
        let s = buffer_text(term.backend().buffer());
        assert!(
            !s.contains('\u{2588}'),
            "a full-height thumb reads as stuck"
        );
    }

    #[test]
    fn scrolling_past_the_end_is_clamped() {
        let job = mk_job("Job");
        let events: Vec<JobEvent> = (0..40)
            .map(|i| mk_event("working", &format!("event {i}"), ""))
            .collect();
        let theme = Theme::default();
        let draw = |scroll: u16| {
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            term.draw(|f| {
                render(f, f.area(), &job, &events, &theme, 1_001_000, scroll);
            })
            .unwrap();
            buffer_text(term.backend().buffer())
        };
        // Way past the end must land on the same frame as the true end, not
        // on an empty pane.
        assert_eq!(draw(9_000), draw(40 - (24 - header_height(&job) - 2)));
    }

    #[test]
    fn duration_freezes_when_the_job_ends() {
        let mut j = mk_job("J");
        j.created_at = Some(1_000);
        j.updated_at = Some(1_300);
        assert_eq!(job_duration(&j, 5_000), Some(4_000), "live jobs run to now");
        j.state = "done".into();
        assert_eq!(
            job_duration(&j, 5_000),
            Some(300),
            "falls back to updated_at"
        );
        j.last_terminal_at = Some(1_200);
        assert_eq!(job_duration(&j, 5_000), Some(200));
        j.state = "failed".into();
        assert_eq!(job_duration(&j, 5_000), Some(200));
        j.created_at = None;
        assert_eq!(job_duration(&j, 5_000), None);
    }

    fn header_text(job: &Job) -> Vec<String> {
        header_lines(job, &Theme::default(), 1_001_000)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    }

    #[test]
    fn blocked_job_leads_with_needs_and_reply() {
        let mut j = mk_job("J");
        j.state = "blocked".into();
        j.needs = "which DB to use".into();
        j.suggested_reply = "use postgres".into();
        j.detail = "waiting on input".into();
        let lines = header_text(&j);
        assert_eq!(lines[0], "Needs: which DB to use");
        assert_eq!(lines[1], "Suggested reply: use postgres");
        assert!(lines.iter().any(|l| l == "Doing: waiting on input"));
        assert!(lines.iter().any(|l| l.starts_with("Goal: Implement")));
        assert_eq!(header_height(&j), lines.len() as u16 + 2);
    }

    #[test]
    fn done_job_shows_result_and_live_job_does_not() {
        let mut j = mk_job("J");
        j.output_result = "shipped PR #12".into();
        assert!(!header_text(&j).iter().any(|l| l.starts_with("Result")));
        j.state = "done".into();
        assert!(header_text(&j)
            .iter()
            .any(|l| l == "Result: shipped PR #12"));
        assert!(!header_text(&j).iter().any(|l| l.starts_with("Doing")));
    }
}
