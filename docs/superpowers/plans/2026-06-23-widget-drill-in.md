# Widget Drill-In + Cost Overflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Cost widget's overflowing text and give every dashboard box a richer expanded view plus a box-relevant drill-in detail screen (Cost, Activity, Code, Ports, Procs, Repo — Worktrees/Jobs/Docker already drill in).

**Architecture:** Extend the existing `View::Detail(Detail::…)` state machine. Each box gets a new `Detail` variant, a full-screen renderer under `src/render/detail/`, a route arm in `dashboard.rs`, and an `Action::Drill` arm in `event.rs`. Multi-row boxes (Cost models, Ports, Procs) drill into a selected row; Activity/Code/Repo open a whole-box detail. Cost gains a second drill level (model → that model's day-by-day). Everything stays read-only and best-effort (missing data → "no data" message, never a crash).

**Tech Stack:** Rust, ratatui (`Frame`/`Block`/`Table`/`Paragraph`), crossterm, headless `TestBackend` render tests.

**Conventions to honor (from CLAUDE.md):**
- TUI colors come from `&Theme` only — never hardcode.
- Cost dedup-by-`message.id` in `collect/usage.rs` must stay intact.
- Run `cargo test`, `cargo clippy`, `cargo fmt` before each commit; keep clippy clean.
- No co-author trailers on commits.

---

## File Structure

**Modified:**
- `src/render/widgets/cost.rs` — responsive header + truncating model names + show-as-many-models-as-fit (overflow fix + richer expand).
- `src/collect/usage.rs` — `ModelUsage` gains per-model cache split; `UsageTotals` gains `by_model_day` (per-model daily history).
- `src/app.rs` — new `Detail` enum variants + `back()` arm for `CostModel → Cost`.
- `src/event.rs` — `Action::Drill` arms for every box; `open_cost_detail`/`open_cost_model` helpers; Up/Down selection for `Detail::Cost`.
- `src/render/dashboard.rs` — `DetailRoute` arms + full-screen routing for the six new detail views.
- `src/render/detail/mod.rs` — register six new detail modules.

**Created (one file per new detail view):**
- `src/render/detail/cost.rs` — all-models breakdown (`render`) + single-model day-by-day (`render_model`).
- `src/render/detail/activity.rs` — prompt cadence history + cache efficiency.
- `src/render/detail/code.rs` — all languages with % of codebase.
- `src/render/detail/ports.rs` — single endpoint detail.
- `src/render/detail/procs.rs` — single process detail (full untruncated cmd).
- `src/render/detail/repo.rs` — repo health expanded.

---

## Task 1: Cost widget — fix overflow + richer expand

**Files:**
- Modify: `src/render/widgets/cost.rs`

This task only touches the widget's internal rendering (header construction + model table). No navigation changes. The overflow is the single-line header `Total $X · Today $Y · cache-hit Z%` being wider than a narrow grid cell; the fix is a responsive header that picks the widest variant that fits, plus model-name shortening and showing more model rows when there's vertical room.

- [ ] **Step 1: Write a failing narrow-width render test**

Add this test inside the existing `#[cfg(test)] mod tests` in `src/render/widgets/cost.rs` (after `renders_cost_and_total`):

```rust
    #[test]
    fn narrow_width_does_not_panic_and_shows_total() {
        let totals = UsageTotals {
            by_day: vec![DayUsage {
                day: "2026-06-23".into(),
                cost_usd: 0.50,
                tokens: 1000,
            }],
            by_model: vec![ModelUsage {
                model: "claude-opus-4-8-some-very-long-model-name".into(),
                cost_usd: 1.23,
                input: 100_000,
                output: 50_000,
                cache_write: 0,
                cache_read: 0,
            }],
            total_cost_usd: 1.23,
            cache_read: 10_000,
            cache_write: 5_000,
            fresh_input: 85_000,
            ..Default::default()
        };

        let mut term = Terminal::new(TestBackend::new(28, 10)).unwrap();
        term.draw(|f| {
            render(
                f,
                Rect { x: 0, y: 0, width: 28, height: 10 },
                Some(&totals),
                &Theme::default(),
                false,
                Band::Compact,
                "2026-06-23",
            );
        })
        .unwrap();

        let s = buffer_text(term.backend().buffer());
        // The compact header keeps the total visible even at 28 cols.
        assert!(s.contains("1.23"), "expected total cost visible when narrow");
        // The block border column must not be overwritten by header text.
        let buf = term.backend().buffer();
        assert_eq!(buf.cell((27, 1)).map(|c| c.symbol().to_string()), Some("│".to_string()),
            "right border must be intact (no overflow)");
    }
```

> Note: this test uses `..Default::default()` and the new `cache_write`/`cache_read` fields on `ModelUsage`, which Task 2 adds. If executing Task 1 before Task 2, this test will not compile yet. **Execute Task 2 first, then Task 1** — reorder in your TodoWrite. (Both are listed in dependency order: do Task 2's struct change, then return here.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib widgets::cost::tests::narrow_width_does_not_panic_and_shows_total -- --nocapture`
Expected: FAIL — the current header overwrites the right border at 28 cols (or compile error until the responsive header exists).

- [ ] **Step 3: Add the responsive-header + truncate helpers and rewrite the header/model selection**

In `src/render/widgets/cost.rs`, add these two helpers above `fn render` (after the `use` lines):

```rust
/// Strip a leading "claude-" prefix so model names fit narrow columns.
fn short_model(name: &str) -> String {
    name.strip_prefix("claude-").unwrap_or(name).to_string()
}

/// Pick the widest header variant that fits `width` columns.
fn header_line(
    total: f64,
    today_cost: f64,
    cache_pct: u64,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let full = format!("Total ${total:.2}  ·  Today ${today_cost:.2}  ·  cache-hit {cache_pct}%");
    let med = format!("${total:.2} · today ${today_cost:.2} · {cache_pct}%");
    let w = width as usize;
    if full.chars().count() <= w {
        Line::from(vec![
            Span::styled(format!("Total ${total:.2}"), Style::new().fg(theme.accent)),
            Span::raw(format!("  ·  Today ${today_cost:.2}  ·  cache-hit {cache_pct}%")),
        ])
    } else if med.chars().count() <= w {
        Line::from(vec![
            Span::styled(format!("${total:.2}"), Style::new().fg(theme.accent)),
            Span::raw(format!(" · today ${today_cost:.2} · {cache_pct}%")),
        ])
    } else {
        Line::from(Span::styled(
            format!("${total:.2}"),
            Style::new().fg(theme.accent),
        ))
    }
}
```

Then, in `fn render`, replace the existing `// Header line.` block (the `let header = Line::from(vec![ … ]);`) — note the header is now built per-branch because it needs the inner column width. Delete the old `let header = …` binding entirely.

Replace the model-selection line `let top_models: Vec<&ModelUsage> = totals.by_model.iter().take(3).collect();` with a band-aware count:

```rust
    // Show as many models as vertically fit; the small dashboard cell keeps it tight,
    // the full-screen Expanded view shows the whole breakdown.
    let compact = band == Band::Compact;
    let max_models = if compact {
        3
    } else {
        (inner.height as usize).saturating_sub(4).clamp(1, 20)
    };
    let top_models: Vec<&ModelUsage> = totals.by_model.iter().take(max_models).collect();
```

> The existing `let compact = band == Band::Compact;` line further down is now redundant — remove the later duplicate so `compact` is declared once, above the `if compact { … }` block.

In both the `if compact` and `else` branches, replace `f.render_widget(Paragraph::new(header), cuts[0]);` with a freshly-built responsive header:

```rust
        let header = header_line(
            totals.total_cost_usd,
            today_cost,
            cache_pct,
            cuts[0].width,
            theme,
        );
        f.render_widget(Paragraph::new(header), cuts[0]);
```

Finally, in `fn render_model_table`, shorten the model name and loosen its column so it never forces the box wider than the cell. Change the row's first cell from `m.model.clone()` to `short_model(&m.model)`, and change the first column constraint from `Constraint::Min(20)` to `Constraint::Min(8)`:

```rust
fn render_model_table(f: &mut Frame, area: Rect, models: &[&ModelUsage], theme: &Theme) {
    let rows: Vec<Row> = models
        .iter()
        .map(|m| {
            Row::new(vec![
                short_model(&m.model),
                format!("${:.2}", m.cost_usd),
                format!("{} out", thousands(m.output)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        &[
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(12),
        ],
    )
    .header(Row::new(["Model", "Cost", "Output"]).style(theme.dim_style()))
    .column_spacing(1);

    f.render_widget(table, area);
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib widgets::cost::tests`
Expected: PASS (both `renders_cost_and_total` and `narrow_width_does_not_panic_and_shows_total`).

- [ ] **Step 5: Lint, format, commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/widgets/cost.rs
git commit -m "fix(cost): responsive header + truncate model names, show more models when expanded"
```

---

## Task 2: usage.rs — per-model cache split + per-model daily history

**Files:**
- Modify: `src/collect/usage.rs`
- Modify: `src/render/widgets/cost.rs` (test literal), `src/render/widgets/activity.rs` (test literal)

The Cost drill-in needs (a) per-model cache-write/cache-read counts for the full token split, and (b) a per-model day-by-day history. Extend the data model and `totalize`.

- [ ] **Step 1: Write a failing aggregation test**

Add to the `#[cfg(test)] mod tests` in `src/collect/usage.rs`:

```rust
    #[test]
    fn per_model_cache_split_and_daily() {
        let recs = vec![
            UsageRecord {
                day: "2026-06-22".into(),
                model: "claude-x".into(),
                input: 100,
                output: 10,
                cache_write: 5,
                cache_read: 50,
            },
            UsageRecord {
                day: "2026-06-23".into(),
                model: "claude-x".into(),
                input: 200,
                output: 20,
                cache_write: 0,
                cache_read: 80,
            },
        ];
        let mut map = HashMap::new();
        map.insert(
            "claude-x".to_string(),
            ModelPrice { input: 1e-6, output: 5e-6, cache_write: 0.0, cache_read: 0.0 },
        );
        let totals = totalize(&recs, &PriceTable(map));

        let m = &totals.by_model[0];
        assert_eq!(m.cache_write, 5);
        assert_eq!(m.cache_read, 130);

        let days = totals.by_model_day.get("claude-x").expect("model day history");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].day, "2026-06-22"); // ascending
        assert_eq!(days[1].day, "2026-06-23");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib collect::usage::tests::per_model_cache_split_and_daily`
Expected: FAIL to compile — `ModelUsage` has no `cache_write`/`cache_read`, `UsageTotals` has no `by_model_day`.

- [ ] **Step 3: Extend the structs**

In `src/collect/usage.rs`, change `ModelUsage` (around line 28) to:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub model: String,
    pub cost_usd: f64,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}
```

Add a field to `UsageTotals` (around line 36). Use the fully-qualified `std::collections::HashMap` because the `use` for `HashMap` lives below the struct:

```rust
#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub by_day: Vec<DayUsage>,     // ascending by day
    pub by_model: Vec<ModelUsage>, // descending by cost
    pub by_model_day: std::collections::HashMap<String, Vec<DayUsage>>, // model -> ascending days
    pub total_cost_usd: f64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub fresh_input: u64,
}
```

- [ ] **Step 4: Extend `totalize`**

In `fn totalize`, change the `model` accumulator to carry the full split and add a `model_day` accumulator. Replace the body from `let mut model: HashMap…` through the `totals.by_model.sort_by(…)` block with:

```rust
    let mut model: HashMap<String, (f64, u64, u64, u64, u64)> = HashMap::new();
    let mut model_day: HashMap<String, HashMap<String, (f64, u64)>> = HashMap::new();
    let mut totals = UsageTotals::default();
    for r in records {
        let cost = record_cost(r, prices);
        totals.total_cost_usd += cost;
        totals.cache_read += r.cache_read;
        totals.cache_write += r.cache_write;
        totals.fresh_input += r.input;
        let toks = r.input + r.output + r.cache_write + r.cache_read;
        let d = day.entry(r.day.clone()).or_default();
        d.0 += cost;
        d.1 += toks;
        let m = model.entry(r.model.clone()).or_default();
        m.0 += cost;
        m.1 += r.input;
        m.2 += r.output;
        m.3 += r.cache_write;
        m.4 += r.cache_read;
        let md = model_day
            .entry(r.model.clone())
            .or_default()
            .entry(r.day.clone())
            .or_default();
        md.0 += cost;
        md.1 += toks;
    }
    totals.by_day = day
        .into_iter()
        .map(|(day, (cost_usd, tokens))| DayUsage { day, cost_usd, tokens })
        .collect();
    totals.by_day.sort_by(|a, b| a.day.cmp(&b.day));
    totals.by_model = model
        .into_iter()
        .map(|(model, (cost_usd, input, output, cache_write, cache_read))| ModelUsage {
            model,
            cost_usd,
            input,
            output,
            cache_write,
            cache_read,
        })
        .collect();
    totals.by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    totals.by_model_day = model_day
        .into_iter()
        .map(|(model, days)| {
            let mut v: Vec<DayUsage> = days
                .into_iter()
                .map(|(day, (cost_usd, tokens))| DayUsage { day, cost_usd, tokens })
                .collect();
            v.sort_by(|a, b| a.day.cmp(&b.day));
            (model, v)
        })
        .collect();
    totals
```

(Leave the `let mut day: HashMap…` declaration above this block as-is.)

- [ ] **Step 5: Fix the two existing `ModelUsage`/`UsageTotals` test literals**

Find every literal construction: `rg -n "ModelUsage \{|UsageTotals \{" src`. Two test sites build them: `src/render/widgets/cost.rs` (`renders_cost_and_total`) and `src/render/widgets/activity.rs` (`renders_activity_and_cache`). In each `ModelUsage { … }` literal add `cache_write: 0, cache_read: 0,` and in each `UsageTotals { … }` literal add `..Default::default()` as the final field (so future fields don't break them). Example for `cost.rs`:

```rust
            by_model: vec![ModelUsage {
                model: "claude-sonnet-4-6".into(),
                cost_usd: 1.23,
                input: 100_000,
                output: 50_000,
                cache_write: 0,
                cache_read: 0,
            }],
            total_cost_usd: 1.23,
            cache_read: 10_000,
            cache_write: 5_000,
            fresh_input: 85_000,
            ..Default::default()
        };
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib`
Expected: PASS (new aggregation test green; all prior tests still green).

- [ ] **Step 7: Lint, format, commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/collect/usage.rs src/render/widgets/cost.rs src/render/widgets/activity.rs
git commit -m "feat(usage): per-model cache split + per-model daily history in UsageTotals"
```

---

## Task 3: Cost drill-in — all models + single-model day-by-day

**Files:**
- Create: `src/render/detail/cost.rs`
- Modify: `src/render/detail/mod.rs`, `src/app.rs`, `src/event.rs`, `src/render/dashboard.rs`

- [ ] **Step 1: Write the failing detail-renderer test**

Create `src/render/detail/cost.rs` with ONLY the test first is impractical; instead create the full module (Step 3) and put this test at the bottom. Write the test block now so you know the target API:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::usage::{DayUsage, ModelUsage, UsageTotals};
    use ratatui::widgets::TableState;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn sample() -> UsageTotals {
        UsageTotals {
            by_model: vec![ModelUsage {
                model: "claude-opus-4-8".into(),
                cost_usd: 9.99,
                input: 1000,
                output: 2000,
                cache_write: 300,
                cache_read: 7000,
            }],
            total_cost_usd: 9.99,
            cache_read: 7000,
            cache_write: 300,
            fresh_input: 1000,
            ..Default::default()
        }
    }

    #[test]
    fn renders_all_models_table() {
        let totals = sample();
        let mut st = TableState::default();
        st.select(Some(0));
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| render(f, f.area(), &totals, &Theme::default(), &mut st, "2026-06-23"))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("opus-4-8"), "expected shortened model name");
        assert!(s.contains("Hit"), "expected cache-hit column header");
    }

    #[test]
    fn renders_single_model_daily() {
        let m = sample().by_model[0].clone();
        let days = vec![DayUsage { day: "2026-06-22".into(), cost_usd: 4.0, tokens: 500 }];
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| render_model(f, f.area(), &m, &days, &Theme::default(), 0))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("opus-4-8"), "expected model name in header");
        assert!(s.contains("2026-06-22"), "expected day in daily list");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib detail::cost::tests` (after wiring the module in Step 4 it will compile; before that it won't be discovered). Expected once compiling: FAIL until the renderers exist.

- [ ] **Step 3: Write the detail renderer**

Put this at the TOP of `src/render/detail/cost.rs` (above the test module from Step 1):

```rust
//! Full-screen Cost detail: all models with full token split, drill into one model.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::collect::usage::{DayUsage, ModelUsage, UsageTotals};
use crate::theme::Theme;
use crate::util::thousands;

/// Strip a leading "claude-" prefix for compactness.
fn short_model(name: &str) -> String {
    name.strip_prefix("claude-").unwrap_or(name).to_string()
}

/// cache-read share of (input + cache-write + cache-read), as a percent. Output is
/// excluded to match the dashboard's cache-hit semantics.
fn cache_hit(input: u64, cw: u64, cr: u64) -> u64 {
    let total = input + cw + cr;
    if total == 0 {
        0
    } else {
        cr * 100 / total
    }
}

/// All-models breakdown with a selectable model table.
pub fn render(
    f: &mut Frame,
    area: Rect,
    totals: &UsageTotals,
    theme: &Theme,
    state: &mut TableState,
    today: &str,
) {
    let chunks = Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).split(area);
    render_header(f, chunks[0], totals, theme, today);
    render_models(f, chunks[1], &totals.by_model, theme, state);
}

fn render_header(f: &mut Frame, area: Rect, t: &UsageTotals, theme: &Theme, today: &str) {
    let today_cost = t
        .by_day
        .iter()
        .find(|d| d.day == today)
        .map(|d| d.cost_usd)
        .unwrap_or(0.0);
    let hit = cache_hit(t.fresh_input, t.cache_write, t.cache_read);
    let lines = vec![
        Line::from(vec![
            Span::raw("Total "),
            Span::styled(
                format!("${:.2}", t.total_cost_usd),
                Style::new().fg(theme.accent),
            ),
            Span::raw(format!("   Today ${today_cost:.2}   cache-hit {hit}%")),
        ]),
        Line::from(vec![
            Span::styled("tokens  ", theme.dim_style()),
            Span::raw(format!(
                "fresh {}  ·  cache-write {}  ·  cache-read {}",
                thousands(t.fresh_input),
                thousands(t.cache_write),
                thousands(t.cache_read),
            )),
        ]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Cost ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn render_models(
    f: &mut Frame,
    area: Rect,
    models: &[ModelUsage],
    theme: &Theme,
    state: &mut TableState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Models ")
        .title_style(theme.title());

    if models.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no usage data", theme.dim_style())).block(block),
            area,
        );
        return;
    }

    let rows: Vec<Row> = models
        .iter()
        .map(|m| {
            let hit = cache_hit(m.input, m.cache_write, m.cache_read);
            Row::new(vec![
                Cell::from(Span::styled(
                    short_model(&m.model),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Cell::from(format!("${:.2}", m.cost_usd)),
                Cell::from(thousands(m.input)),
                Cell::from(thousands(m.output)),
                Cell::from(thousands(m.cache_write)),
                Cell::from(thousands(m.cache_read)),
                Cell::from(format!("{hit}%")),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(12),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(5),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(["Model", "Cost", "In", "Out", "C-Write", "C-Read", "Hit"])
                .style(theme.dim_style()),
        )
        .column_spacing(1)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, state);
}

/// Single-model detail: token split + cost, then a scrollable day-by-day list.
pub fn render_model(
    f: &mut Frame,
    area: Rect,
    model: &ModelUsage,
    days: &[DayUsage],
    theme: &Theme,
    scroll: u16,
) {
    let chunks = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);

    let hit = cache_hit(model.input, model.cache_write, model.cache_read);
    let header = vec![
        Line::from(vec![
            Span::raw("Model: "),
            Span::styled(short_model(&model.model), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::raw("Cost:  "),
            Span::styled(
                format!("${:.2}", model.cost_usd),
                Style::new().fg(theme.accent),
            ),
        ]),
        Line::from(Span::raw(format!(
            "Tokens: in {}  out {}  c-write {}  c-read {}",
            thousands(model.input),
            thousands(model.output),
            thousands(model.cache_write),
            thousands(model.cache_read),
        ))),
        Line::from(Span::raw(format!("Cache-hit: {hit}%"))),
    ];
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(" Model ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(header)).block(hblock), chunks[0]);

    let n = days.len();
    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Daily ({n} days) "))
        .title_style(theme.title());

    if days.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no daily data", theme.dim_style())).block(bblock),
            chunks[1],
        );
        return;
    }

    let lines: Vec<Line> = days
        .iter()
        .rev()
        .map(|d| {
            Line::from(vec![
                Span::styled(d.day.clone(), theme.dim_style()),
                Span::raw(format!("   ${:.2}   ", d.cost_usd)),
                Span::styled(format!("{} tok", thousands(d.tokens)), theme.dim_style()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(bblock)
            .scroll((scroll, 0)),
        chunks[1],
    );
}
```

- [ ] **Step 4: Register the module**

In `src/render/detail/mod.rs`, add (keep alphabetical-ish with existing):

```rust
pub mod cost;
```

- [ ] **Step 5: Add the `Detail` variants and `back()` arm**

In `src/app.rs`, extend the `Detail` enum:

```rust
pub enum Detail {
    Worktree,
    Job(usize),
    Container(usize),
    Diff(DiffView),
    Cost,
    CostModel(usize),
}
```

In `App::back()`, add an arm before `_ => View::Dashboard,`:

```rust
            View::Detail(Detail::CostModel(_)) => View::Detail(Detail::Cost),
```

- [ ] **Step 6: Wire drill + selection in `event.rs`**

Add two helpers near `open_container_detail` in `src/event.rs`:

```rust
fn open_cost_detail(app: &mut App) {
    if app.data.lock().unwrap().usage.is_none() {
        return;
    }
    app.detail_table = ratatui::widgets::TableState::default();
    app.detail_table.select(Some(0));
    app.detail_scroll = 0;
    app.view = View::Detail(Detail::Cost);
}

fn open_cost_model(app: &mut App) {
    let Some(sel) = app.detail_table.selected() else {
        return;
    };
    let n = app
        .data
        .lock()
        .unwrap()
        .usage
        .as_ref()
        .map(|u| u.by_model.len())
        .unwrap_or(0);
    if sel < n {
        app.view = View::Detail(Detail::CostModel(sel));
        app.detail_scroll = 0;
    }
}
```

In `Action::Drill`, add the Cost focus arm inside the `View::Dashboard | View::Expanded(_) => match app.focus { … }` block (alongside the existing arms):

```rust
                WidgetKind::Cost => open_cost_detail(app),
```

And add a new top-level arm (next to `View::Detail(Detail::Worktree) => open_file_diff(app),`):

```rust
            View::Detail(Detail::Cost) => open_cost_model(app),
```

In `Action::Up`, add an arm before `View::Detail(_) =>`:

```rust
            View::Detail(Detail::Cost) => {
                let s = app.detail_table.selected().unwrap_or(0);
                app.detail_table.select(Some(s.saturating_sub(1)));
            }
```

In `Action::Down`, add the matching arm before `View::Detail(_) =>`:

```rust
            View::Detail(Detail::Cost) => {
                let n = app
                    .data
                    .lock()
                    .unwrap()
                    .usage
                    .as_ref()
                    .map(|u| u.by_model.len())
                    .unwrap_or(0);
                if n > 0 {
                    let s = app.detail_table.selected().unwrap_or(0);
                    app.detail_table.select(Some((s + 1).min(n - 1)));
                }
            }
```

- [ ] **Step 7: Route both views in `dashboard.rs`**

In `src/render/dashboard.rs`, add an import near the top:

```rust
use crate::collect::usage::DayUsage;
```

Add two variants to the local `DetailRoute` enum:

```rust
        Cost,
        CostModel(usize),
```

Add the matching classifier arms in the `let route = match &app.view { … }` block (before `_ => DetailRoute::None,`):

```rust
        View::Detail(Detail::Cost) => DetailRoute::Cost,
        View::Detail(Detail::CostModel(i)) => DetailRoute::CostModel(*i),
```

Add the render arms in the `match route { … }` block (before `DetailRoute::None => {}`):

```rust
        DetailRoute::Cost => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let usage = { app.data.lock().unwrap().usage.clone() };
            match usage {
                Some(u) => crate::render::detail::cost::render(
                    f,
                    outer[0],
                    &u,
                    &app.theme,
                    &mut app.detail_table,
                    &today,
                ),
                None => f.render_widget(
                    Paragraph::new("no usage data — Esc to go back").style(app.theme.dim_style()),
                    outer[0],
                ),
            }
            let line = Line::from("  Esc back · ↑/↓ select · Enter model · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
        DetailRoute::CostModel(idx) => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let usage = { app.data.lock().unwrap().usage.clone() };
            match usage.as_ref().and_then(|u| u.by_model.get(idx)) {
                Some(m) => {
                    let empty: Vec<DayUsage> = Vec::new();
                    let days = usage
                        .as_ref()
                        .and_then(|u| u.by_model_day.get(&m.model))
                        .map(|v| v.as_slice())
                        .unwrap_or(&empty);
                    crate::render::detail::cost::render_model(
                        f,
                        outer[0],
                        m,
                        days,
                        &app.theme,
                        app.detail_scroll,
                    );
                }
                None => f.render_widget(
                    Paragraph::new("model no longer present — Esc to go back")
                        .style(app.theme.dim_style()),
                    outer[0],
                ),
            }
            let line = Line::from("  Esc back · ↑/↓ scroll · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
```

> Borrow note: `m` borrows `usage`, and `days` also borrows `usage`, so both reads happen before the render call while `usage` is still alive — this compiles because `usage` (the cloned `Option<UsageTotals>`) outlives the block.

- [ ] **Step 8: Run tests**

Run: `cargo test --lib detail::cost::tests && cargo test --lib`
Expected: PASS.

- [ ] **Step 9: Lint, format, commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/detail/cost.rs src/render/detail/mod.rs src/app.rs src/event.rs src/render/dashboard.rs
git commit -m "feat(cost): drill into all-models breakdown and single-model daily history"
```

---

## Task 4: Activity drill-in

**Files:**
- Create: `src/render/detail/activity.rs`
- Modify: `src/render/detail/mod.rs`, `src/app.rs`, `src/event.rs`, `src/render/dashboard.rs`

Whole-box detail: cache efficiency + total prompts + busiest day + full daily cadence list (scrollable).

- [ ] **Step 1: Create the renderer with its test**

Create `src/render/detail/activity.rs`:

```rust
//! Full-screen Activity detail: cache efficiency + full prompt-cadence history.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::usage::UsageTotals;
use crate::theme::Theme;
use crate::util::thousands;

pub fn render(
    f: &mut Frame,
    area: Rect,
    cache: Option<&UsageTotals>,
    cadence: &[(String, u32)],
    theme: &Theme,
    scroll: u16,
) {
    let chunks = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).split(area);

    // Header: cache efficiency + prompt summary.
    let total_prompts: u32 = cadence.iter().map(|(_, n)| n).sum();
    let busiest = cadence
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(d, n)| format!("{d} ({n})"))
        .unwrap_or_else(|| "—".to_string());

    let cache_line = match cache {
        Some(t) if t.cache_read + t.cache_write + t.fresh_input > 0 => {
            let total = t.cache_read + t.cache_write + t.fresh_input;
            let hit = t.cache_read * 100 / total;
            Line::from(vec![
                Span::raw("cache-hit "),
                Span::styled(format!("{hit}%"), Style::new().fg(theme.accent)),
                Span::raw(format!(
                    "   read {}  write {}  fresh {}",
                    thousands(t.cache_read),
                    thousands(t.cache_write),
                    thousands(t.fresh_input),
                )),
            ])
        }
        _ => Line::from(Span::styled("no usage yet", theme.dim_style())),
    };

    let header = vec![
        cache_line,
        Line::from(vec![
            Span::styled("prompts ", theme.dim_style()),
            Span::raw(total_prompts.to_string()),
            Span::styled("   busiest ", theme.dim_style()),
            Span::raw(busiest),
            Span::styled("   active days ", theme.dim_style()),
            Span::raw(cadence.len().to_string()),
        ]),
    ];
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(" Activity ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(header)).block(hblock), chunks[0]);

    // Body: full daily cadence, newest first, with a simple bar.
    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(" Daily prompts ")
        .title_style(theme.title());
    if cadence.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no prompt history", theme.dim_style())).block(bblock),
            chunks[1],
        );
        return;
    }
    let max = cadence.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    let lines: Vec<Line> = cadence
        .iter()
        .rev()
        .map(|(day, n)| {
            let bar_len = (*n as usize * 24 / max as usize).max(if *n > 0 { 1 } else { 0 });
            let bar: String = "▇".repeat(bar_len);
            Line::from(vec![
                Span::styled(day.clone(), theme.dim_style()),
                Span::raw("  "),
                Span::styled(bar, Style::new().fg(theme.accent)),
                Span::raw(format!(" {n}")),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(bblock)
            .scroll((scroll, 0)),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_cadence_history() {
        let cadence = vec![
            ("2026-06-22".to_string(), 3u32),
            ("2026-06-23".to_string(), 8u32),
        ];
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| render(f, f.area(), None, &cadence, &Theme::default(), 0))
            .unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("Activity"), "expected title");
        assert!(s.contains("2026-06-23"), "expected a day row");
        assert!(s.contains("busiest"), "expected busiest summary");
    }
}
```

- [ ] **Step 2: Register, add variant, wire drill, route**

`src/render/detail/mod.rs`: add `pub mod activity;`

`src/app.rs` `Detail` enum: add `Activity,`

`src/event.rs` `Action::Drill` focus block: add

```rust
                WidgetKind::Activity => {
                    if app.data.lock().unwrap().usage.is_some()
                        || !app.data.lock().unwrap().activity.is_empty()
                    {
                        app.view = View::Detail(Detail::Activity);
                        app.detail_scroll = 0;
                    }
                }
```

`src/render/dashboard.rs`: add `Activity` to `DetailRoute`, classifier arm `View::Detail(Detail::Activity) => DetailRoute::Activity,`, and render arm:

```rust
        DetailRoute::Activity => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let (usage, activity) = {
                let d = app.data.lock().unwrap();
                (d.usage.clone(), d.activity.clone())
            };
            crate::render::detail::activity::render(
                f,
                outer[0],
                usage.as_ref(),
                &activity,
                &app.theme,
                app.detail_scroll,
            );
            let line = Line::from("  Esc back · ↑/↓ scroll · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
```

- [ ] **Step 3: Test, lint, commit**

```bash
cargo test --lib detail::activity::tests && cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/detail/activity.rs src/render/detail/mod.rs src/app.rs src/event.rs src/render/dashboard.rs
git commit -m "feat(activity): drill into full prompt-cadence history + cache summary"
```

---

## Task 5: Code drill-in

**Files:**
- Create: `src/render/detail/code.rs`
- Modify: `src/render/detail/mod.rs`, `src/app.rs`, `src/event.rs`, `src/render/dashboard.rs`

Whole-box detail: all languages with files, code lines, and % of total code; a TOTAL row.

- [ ] **Step 1: Create the renderer with its test**

Create `src/render/detail/code.rs`:

```rust
//! Full-screen Code detail: every language with files, code lines, and % of total.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::collect::loc::{totals, LocRow};
use crate::theme::Theme;
use crate::util::thousands;

pub fn render(f: &mut Frame, area: Rect, loc: &[LocRow], theme: &Theme) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    let sums = totals(loc);
    let header = Line::from(vec![
        Span::styled("files ", theme.dim_style()),
        Span::raw(thousands(sums.files as u64)),
        Span::styled("   code ", theme.dim_style()),
        Span::styled(thousands(sums.code as u64), Style::new().fg(theme.accent)),
        Span::styled("   languages ", theme.dim_style()),
        Span::raw(loc.len().to_string()),
    ]);
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(" Code ")
        .title_style(theme.title());
    f.render_widget(Paragraph::new(header).block(hblock), chunks[0]);

    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(" Languages ")
        .title_style(theme.title());
    if loc.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no code counted", theme.dim_style())).block(bblock),
            chunks[1],
        );
        return;
    }

    let total_code = sums.code.max(1);
    let mut rows: Vec<Row> = loc
        .iter()
        .map(|r| {
            let pct = r.code * 100 / total_code;
            Row::new(vec![
                Cell::from(Span::styled(
                    r.language.clone(),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Cell::from(thousands(r.files as u64)),
                Cell::from(thousands(r.code as u64)),
                Cell::from(format!("{pct}%")),
            ])
        })
        .collect();
    rows.push(
        Row::new(vec![
            Cell::from(Span::styled("TOTAL", Style::new().add_modifier(Modifier::BOLD))),
            Cell::from(thousands(sums.files as u64)),
            Cell::from(thousands(sums.code as u64)),
            Cell::from("100%".to_string()),
        ])
        .style(Style::new().add_modifier(Modifier::BOLD)),
    );

    let widths = [
        Constraint::Min(12),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(6),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new(["Language", "Files", "Code", "%"]).style(theme.dim_style()))
        .column_spacing(1)
        .block(bblock);
    f.render_widget(table, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_language_table_and_total() {
        let loc = vec![
            LocRow { language: "Rust".into(), files: 10, code: 900 },
            LocRow { language: "TOML".into(), files: 1, code: 100 },
        ];
        let mut term = Terminal::new(TestBackend::new(80, 16)).unwrap();
        term.draw(|f| render(f, f.area(), &loc, &Theme::default())).unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("Rust"), "expected language row");
        assert!(s.contains("TOTAL"), "expected total row");
    }
}
```

- [ ] **Step 2: Register, add variant, wire drill, route**

`src/render/detail/mod.rs`: add `pub mod code;`

`src/app.rs` `Detail` enum: add `Code,`

`src/event.rs` `Action::Drill` focus block: add

```rust
                WidgetKind::Code => {
                    if !app.data.lock().unwrap().loc.is_empty() {
                        app.view = View::Detail(Detail::Code);
                        app.detail_scroll = 0;
                    }
                }
```

`src/render/dashboard.rs`: add `Code` to `DetailRoute`, classifier `View::Detail(Detail::Code) => DetailRoute::Code,`, render arm:

```rust
        DetailRoute::Code => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let loc = { app.data.lock().unwrap().loc.clone() };
            crate::render::detail::code::render(f, outer[0], &loc, &app.theme);
            let line = Line::from("  Esc back · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
```

- [ ] **Step 3: Test, lint, commit**

```bash
cargo test --lib detail::code::tests && cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/detail/code.rs src/render/detail/mod.rs src/app.rs src/event.rs src/render/dashboard.rs
git commit -m "feat(code): drill into full per-language breakdown with code share"
```

---

## Task 6: Ports drill-in (selected endpoint)

**Files:**
- Create: `src/render/detail/ports.rs`
- Modify: `src/render/detail/mod.rs`, `src/app.rs`, `src/event.rs`, `src/render/dashboard.rs`

- [ ] **Step 1: Create the renderer with its test**

Create `src/render/detail/ports.rs`:

```rust
//! Full-screen detail for a single dev endpoint.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::ports::Endpoint;
use crate::theme::Theme;

pub fn render(f: &mut Frame, area: Rect, ep: &Endpoint, theme: &Theme) {
    let status = if ep.up {
        Span::styled("up", Style::new().fg(theme.ok))
    } else {
        Span::styled("down", Style::new().fg(theme.err))
    };
    let latency = ep
        .latency_ms
        .map(|ms| format!("{ms}ms"))
        .unwrap_or_else(|| "—".to_string());
    let pid = ep
        .pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "—".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled("service  ", theme.dim_style()),
            Span::styled(ep.label.clone(), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("address  ", theme.dim_style()),
            Span::raw(format!("{}:{}", ep.host, ep.port)),
        ]),
        Line::from(vec![Span::styled("status   ", theme.dim_style()), status]),
        Line::from(vec![
            Span::styled("latency  ", theme.dim_style()),
            Span::raw(latency),
        ]),
        Line::from(vec![
            Span::styled("pid      ", theme.dim_style()),
            Span::raw(pid),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Port · {} ", ep.label))
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_endpoint_detail() {
        let ep = Endpoint {
            label: "api".into(),
            host: "127.0.0.1".into(),
            port: 8080,
            up: true,
            latency_ms: Some(5),
            pid: Some(99),
        };
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| render(f, f.area(), &ep, &Theme::default())).unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("api"), "expected label");
        assert!(s.contains("127.0.0.1:8080"), "expected address");
    }
}
```

- [ ] **Step 2: Register, add variant, wire drill, route**

`src/render/detail/mod.rs`: add `pub mod ports;`

`src/app.rs` `Detail` enum: add `Ports(usize),`

`src/event.rs` `Action::Drill` focus block: add

```rust
                WidgetKind::Ports => {
                    if let Some(idx) = app
                        .ui
                        .get(&WidgetKind::Ports)
                        .and_then(|u| u.table.selected())
                    {
                        let n = app.data.lock().unwrap().endpoints.len();
                        if idx < n {
                            app.view = View::Detail(Detail::Ports(idx));
                            app.detail_scroll = 0;
                        }
                    }
                }
```

`src/render/dashboard.rs`: add `Ports(usize)` to `DetailRoute`, classifier `View::Detail(Detail::Ports(i)) => DetailRoute::Ports(*i),`, render arm:

```rust
        DetailRoute::Ports(idx) => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let ep = { app.data.lock().unwrap().endpoints.get(idx).cloned() };
            match ep {
                Some(ep) => crate::render::detail::ports::render(f, outer[0], &ep, &app.theme),
                None => f.render_widget(
                    Paragraph::new("endpoint no longer present — Esc to go back")
                        .style(app.theme.dim_style()),
                    outer[0],
                ),
            }
            let line = Line::from("  Esc back · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
```

- [ ] **Step 3: Test, lint, commit**

```bash
cargo test --lib detail::ports::tests && cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/detail/ports.rs src/render/detail/mod.rs src/app.rs src/event.rs src/render/dashboard.rs
git commit -m "feat(ports): drill into a single endpoint's health detail"
```

---

## Task 7: Procs drill-in (selected process)

**Files:**
- Create: `src/render/detail/procs.rs`
- Modify: `src/render/detail/mod.rs`, `src/app.rs`, `src/event.rs`, `src/render/dashboard.rs`

Shows the full untruncated command line (the dashboard table truncates it).

- [ ] **Step 1: Create the renderer with its test**

Create `src/render/detail/procs.rs`:

```rust
//! Full-screen detail for a single dev process, including its full command line.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::collect::procs::Proc;
use crate::theme::Theme;
use crate::util::{human_bytes, human_duration};

pub fn render(f: &mut Frame, area: Rect, p: &Proc, theme: &Theme, scroll: u16) {
    let chunks = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).split(area);

    let cpu_style = if p.cpu_pct >= 50.0 {
        Style::new().fg(theme.warn)
    } else {
        Style::new()
    };
    let header = vec![
        Line::from(vec![
            Span::styled("name   ", theme.dim_style()),
            Span::styled(p.name.clone(), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("pid    ", theme.dim_style()),
            Span::raw(p.pid.to_string()),
        ]),
        Line::from(vec![
            Span::styled("cpu    ", theme.dim_style()),
            Span::styled(format!("{:.1}%", p.cpu_pct), cpu_style),
            Span::styled("   mem ", theme.dim_style()),
            Span::raw(human_bytes(p.mem_bytes)),
            Span::styled("   up ", theme.dim_style()),
            Span::raw(human_duration(p.uptime_secs)),
        ]),
    ];
    let hblock = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Process · {} ", p.name))
        .title_style(theme.title());
    f.render_widget(Paragraph::new(Text::from(header)).block(hblock), chunks[0]);

    let cmd = if p.cmd.is_empty() {
        "—".to_string()
    } else {
        p.cmd.clone()
    };
    let bblock = Block::default()
        .borders(Borders::ALL)
        .title(" Command ")
        .title_style(theme.title());
    f.render_widget(
        Paragraph::new(cmd)
            .block(bblock)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_process_and_command() {
        let p = Proc {
            pid: 4242,
            name: "claude".into(),
            cmd: "claude --dangerously-skip-permissions run".into(),
            cpu_pct: 12.0,
            mem_bytes: 256 * 1024 * 1024,
            uptime_secs: 65,
        };
        let mut term = Terminal::new(TestBackend::new(80, 14)).unwrap();
        term.draw(|f| render(f, f.area(), &p, &Theme::default(), 0)).unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("claude"), "expected process name");
        assert!(s.contains("--dangerously-skip-permissions"), "expected full cmd");
    }
}
```

- [ ] **Step 2: Register, add variant, wire drill, route**

`src/render/detail/mod.rs`: add `pub mod procs;`

`src/app.rs` `Detail` enum: add `Procs(usize),`

`src/event.rs` `Action::Drill` focus block: add

```rust
                WidgetKind::Procs => {
                    if let Some(idx) = app
                        .ui
                        .get(&WidgetKind::Procs)
                        .and_then(|u| u.table.selected())
                    {
                        let n = app.data.lock().unwrap().procs.len();
                        if idx < n {
                            app.view = View::Detail(Detail::Procs(idx));
                            app.detail_scroll = 0;
                        }
                    }
                }
```

`src/render/dashboard.rs`: add `Procs(usize)` to `DetailRoute`, classifier `View::Detail(Detail::Procs(i)) => DetailRoute::Procs(*i),`, render arm:

```rust
        DetailRoute::Procs(idx) => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let p = { app.data.lock().unwrap().procs.get(idx).cloned() };
            match p {
                Some(p) => crate::render::detail::procs::render(
                    f,
                    outer[0],
                    &p,
                    &app.theme,
                    app.detail_scroll,
                ),
                None => f.render_widget(
                    Paragraph::new("process no longer present — Esc to go back")
                        .style(app.theme.dim_style()),
                    outer[0],
                ),
            }
            let line = Line::from("  Esc back · ↑/↓ scroll · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
```

- [ ] **Step 3: Test, lint, commit**

```bash
cargo test --lib detail::procs::tests && cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/detail/procs.rs src/render/detail/mod.rs src/app.rs src/event.rs src/render/dashboard.rs
git commit -m "feat(procs): drill into a process showing the full command line"
```

---

## Task 8: Repo drill-in

**Files:**
- Create: `src/render/detail/repo.rs`
- Modify: `src/render/detail/mod.rs`, `src/app.rs`, `src/event.rs`, `src/render/dashboard.rs`

Whole-box detail expanding the same `RepoHealth` fields (no new git calls — recent-commit history is explicitly out of scope).

- [ ] **Step 1: Create the renderer with its test**

Create `src/render/detail/repo.rs`:

```rust
//! Full-screen Repo health detail (branch, sync, stash, dirty, last fetch).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::collect::git::RepoHealth;
use crate::theme::Theme;
use crate::util::human_duration;

pub fn render(f: &mut Frame, area: Rect, repo: Option<&RepoHealth>, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Repo ")
        .title_style(theme.title());

    let Some(r) = repo else {
        f.render_widget(
            Paragraph::new("not a git repo").style(theme.dim_style()).block(block),
            area,
        );
        return;
    };

    let ahead_style = if r.ahead > 0 {
        Style::new().fg(theme.ok)
    } else {
        theme.dim_style()
    };
    let behind_style = if r.behind > 0 {
        Style::new().fg(theme.warn)
    } else {
        theme.dim_style()
    };
    let dirty_style = if r.dirty > 0 {
        Style::new().fg(theme.warn)
    } else {
        theme.dim_style()
    };
    let fetch = match r.last_fetch_secs {
        Some(secs) => format!("{} ago", human_duration(secs)),
        None => "never".to_string(),
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("branch   ", theme.dim_style()),
            Span::styled(r.branch.clone(), Style::new().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("ahead    ", theme.dim_style()),
            Span::styled(r.ahead.to_string(), ahead_style),
            Span::styled("   behind ", theme.dim_style()),
            Span::styled(r.behind.to_string(), behind_style),
        ]),
        Line::from(vec![
            Span::styled("stash    ", theme.dim_style()),
            Span::raw(r.stash.to_string()),
            Span::styled("   dirty ", theme.dim_style()),
            Span::styled(r.dirty.to_string(), dirty_style),
        ]),
        Line::from(vec![
            Span::styled("fetched  ", theme.dim_style()),
            Span::raw(fetch),
        ]),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_repo_detail() {
        let repo = RepoHealth {
            branch: "main".into(),
            ahead: 2,
            behind: 0,
            stash: 1,
            dirty: 3,
            last_fetch_secs: Some(120),
        };
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| render(f, f.area(), Some(&repo), &Theme::default())).unwrap();
        let s = text(term.backend().buffer());
        assert!(s.contains("main"), "expected branch");
        assert!(s.contains("fetched"), "expected fetch line");
    }
}
```

- [ ] **Step 2: Register, add variant, wire drill, route**

`src/render/detail/mod.rs`: add `pub mod repo;`

`src/app.rs` `Detail` enum: add `Repo,`

`src/event.rs` `Action::Drill` focus block: add

```rust
                WidgetKind::Repo => {
                    if app.data.lock().unwrap().repo.is_some() {
                        app.view = View::Detail(Detail::Repo);
                        app.detail_scroll = 0;
                    }
                }
```

At this point every `WidgetKind` is matched in the focus block (Worktrees, Jobs, Docker, Cost, Activity, Code, Ports, Procs, Repo). Remove the now-unreachable `_ => {}` arm from that inner match if clippy flags it; otherwise leave it.

`src/render/dashboard.rs`: add `Repo` to `DetailRoute`, classifier `View::Detail(Detail::Repo) => DetailRoute::Repo,`, render arm:

```rust
        DetailRoute::Repo => {
            let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let repo = { app.data.lock().unwrap().repo.clone() };
            crate::render::detail::repo::render(f, outer[0], repo.as_ref(), &app.theme);
            let line = Line::from("  Esc back · q quit");
            f.render_widget(Paragraph::new(line).style(app.theme.dim_style()), outer[1]);
            app.rects = FrameRects::default();
            return;
        }
```

- [ ] **Step 3: Test, lint, commit**

```bash
cargo test --lib detail::repo::tests && cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt
git add src/render/detail/repo.rs src/render/detail/mod.rs src/app.rs src/event.rs src/render/dashboard.rs
git commit -m "feat(repo): drill into expanded repo-health detail"
```

---

## Task 9: Navigation unit tests + final sweep

**Files:**
- Modify: `src/event.rs` (tests), `src/render/dashboard.rs` (help text if needed)

- [ ] **Step 1: Add drill/back navigation tests**

`apply` is private but reachable from the `#[cfg(test)] mod tests` in the same file. Locate that module in `src/event.rs` (add one if absent: `#[cfg(test)] mod tests { use super::*; ... }`). Add:

```rust
    #[test]
    fn cost_drill_and_back() {
        let mut app = App::new(Theme::default());
        // Seed minimal usage so the drill is allowed.
        {
            let mut d = app.data.lock().unwrap();
            d.usage = Some(crate::collect::usage::UsageTotals {
                by_model: vec![crate::collect::usage::ModelUsage {
                    model: "claude-x".into(),
                    cost_usd: 1.0,
                    input: 1,
                    output: 1,
                    cache_write: 0,
                    cache_read: 0,
                }],
                ..Default::default()
            });
        }
        app.focus = WidgetKind::Cost;
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::Cost)));
        // Enter on the selected model drills one level deeper.
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::CostModel(0))));
        // Back pops CostModel -> Cost, then Cost -> Dashboard.
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Detail(Detail::Cost)));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }

    #[test]
    fn repo_drill_opens_detail() {
        let mut app = App::new(Theme::default());
        {
            let mut d = app.data.lock().unwrap();
            d.repo = Some(crate::collect::git::RepoHealth {
                branch: "main".into(),
                ahead: 0,
                behind: 0,
                stash: 0,
                dirty: 0,
                last_fetch_secs: None,
            });
        }
        app.focus = WidgetKind::Repo;
        apply(&mut app, Action::Drill, ".");
        assert!(matches!(app.view, View::Detail(Detail::Repo)));
        apply(&mut app, Action::Back, ".");
        assert!(matches!(app.view, View::Dashboard));
    }
```

> If `apply`, `Action`, `View`, `Detail`, `WidgetKind`, `App`, `Theme` aren't already in scope in the test module, add the needed `use super::*;` / `use crate::...` lines. `apply` takes `(&mut App, Action, &str)`.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test`
Expected: PASS — all ~61 prior tests plus the new render + nav tests.

- [ ] **Step 3: Verify the help overlay still describes the controls**

Open `src/render/dashboard.rs` `draw_help`. Confirm it already lists `Enter drill in` and `e expand` (it does). No change needed unless you want to note that drill now works on every widget — optional. If you edit it, keep the popup `H`/`W` constants consistent with the line count.

- [ ] **Step 4: Manual smoke check (optional but recommended)**

Run from inside this repo: `cargo run`. Tab to each widget, press `Enter` to drill, `Esc` to back, `e` to expand. Confirm the Cost box no longer overflows at small terminal widths (shrink the terminal). On Cost detail, `↑/↓` selects a model and `Enter` opens its daily history.

- [ ] **Step 5: Final lint/format/commit**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt --check && cargo test
git add -A
git commit -m "test: navigation coverage for widget drill-in; final sweep"
```

---

## Self-Review Notes

**Spec coverage:**
- Cost overflow → Task 1 (responsive header + name truncation + looser column). ✓
- Cost "all models, full token split" + drill into a model's day-by-day → Tasks 2 + 3. ✓
- Activity / Code / Ports / Procs / Repo drill-in (every box, full pass) → Tasks 4–8. ✓
- Worktrees / Jobs / Docker unchanged → not touched. ✓
- Read-only, best-effort (missing data → message, never crash) → every route has a `None`/empty fallback. ✓

**Type consistency:**
- `ModelUsage` fields `cache_write`/`cache_read` defined in Task 2, consumed in Tasks 1 & 3. ✓
- `UsageTotals.by_model_day: HashMap<String, Vec<DayUsage>>` defined in Task 2, consumed in Task 3's `CostModel` route. ✓
- Detail renderer fns: `cost::render` + `cost::render_model`; `activity::render`; `code::render`; `ports::render`; `procs::render`; `repo::render` — names match their route call sites. ✓
- `Detail` variants `Cost`, `CostModel(usize)`, `Activity`, `Code`, `Ports(usize)`, `Procs(usize)`, `Repo` are referenced identically in `app.rs`, `event.rs`, and `dashboard.rs`. ✓
- `back()` handles `CostModel → Cost`; all other new variants fall through to `Dashboard`. ✓

**Execution-order dependency:** Task 2 (struct change) must land before Task 1's new test compiles and before Task 3. Do them in order 2 → 1 → 3 → 4 → 5 → 6 → 7 → 8 → 9, or interleave 1 after 2. Every other task is independent and leaves the tree green.

**Out of scope (confirmed):** killing/stopping anything; per-file LOC; repo commit-log history; the one-shot CLI path (`render.rs`/comfy-table) — TUI only; mouse double-click-to-drill (keyboard `Enter` is the drill mechanism, matching existing widgets).
