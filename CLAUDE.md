# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`claude-cockpit` is a single-binary Rust TUI dashboard that surfaces git worktrees, Claude Code background agents, token/USD cost, rate-limit proximity, Docker, dev endpoints, and tool usage. It reads only local files (`~/.claude/…`, plus repo `docker-compose`/`Dockerfile`s for endpoint discovery) and standard CLIs (`git`, `docker`, `lsof`) — no daemon, no network for the core, **read-only** (it never kills a job, stops a container, or touches branches). Every data source is best-effort: a missing tool or file yields an empty widget, never a crash.

## Commands

```bash
cargo build                       # debug build
cargo build --release             # release binary at target/release/claude-cockpit (lto, opt-level 3)
cargo run                         # launch the interactive TUI (must be inside a git repo)
cargo run -- --worktrees          # one-shot: worktrees table; also --code, --watch, --interval N

cargo test                        # all unit + headless render tests (~61, colocated #[cfg(test)] mods)
cargo test focus_cycles_all       # single test by name
cargo test layout::tests          # a single module's tests

cargo clippy                      # lint (CI-relevant; keep clean)
cargo fmt                         # rustfmt
```

The TUI reads the **current working directory's** git repo, so run/test it from inside a repo with worktrees to see real data.

## Architecture

### Two separate render stacks — don't conflate them
- **Interactive TUI**: `event.rs` (run loop) → `render/dashboard.rs` (grid composition) → `render/widgets/*` + `render/detail/*`. Built on **ratatui** `Frame`s, styled via `theme.rs`.
- **One-shot CLI** (`--worktrees`/`--code`/`--watch`): `render.rs` (top level) builds **strings** with **comfy-table** + **console**, colored with the `VIOLET` const. This path never touches ratatui or `App`.

Adding a column or color to one path does **not** affect the other.

### Threading & data flow
The UI thread owns `App` and runs `event_loop` (250 ms poll). It **redraws only when `DashboardData.rev` changes or input arrives** — every `publish_*` bumps `rev`, so an idle dashboard does no rendering work (background refreshes bump `rev` every 2–10 s, which keeps ages/countdowns ticking). `refresh::spawn` starts three background threads that gather collectors off the UI thread and publish into a shared `Arc<Mutex<DashboardData>>`:
- **slow (10 s)**: `gather_all` — worktrees, LOC, jobs, activity, docker, endpoints, repo health. `git fetch` is throttled to every 6th tick (~60 s), not every 10 s.
- **fast (2 s)**: jobs only — the one widget that benefits from a tight cadence (live agent state). Endpoint discovery stays on the slow thread (it walks the repo for docker files).
- **cost (10 s)**: usage scan (also computes `RateStats` for the Rate widget) + tool-use scan (for the Tools widget); refreshes LiteLLM prices once at startup.

Flow: `collect::*` (IO-bound domain snapshot) → `refresh::publish_<x>` swaps **only its own slice** of `DashboardData` and bumps `rev` → `render::dashboard::render` **clones the slices out of the mutex, drops the guard, then renders**. Hold the lock as briefly as possible; never render while holding it. The manual `r` refresh (which calls `refresh_now`, i.e. `git fetch` + `gather_all`) is debounced with the `REFRESH_INFLIGHT` atomic.

### UI state machine (`app.rs`)
`View` = `Dashboard | Expanded(WidgetKind) | Detail(Detail)`, where `Detail` = `Worktree | Job(idx) | Container(idx) | Diff`. `App::back()` pops `Diff → Worktree → Dashboard`. Worktree detail is special: it keys off `last_wt_idx`/`wt_detail` rather than carrying an index, because drilling into a file diff needs the cached `WorktreeDetail`.

### Input (`event.rs`)
Pure `map_key(code, mods) -> Action`, then `apply(app, action, root)` mutates `App`. Mouse routing uses `FrameRects` (the per-frame hit map written by `dashboard.rs` each draw) to find the clicked widget/row. Note the intentional one-frame lag in `table_offset` (see the "Fix-7" comment in `dashboard.rs`) — clicks resolve against the previous frame's scroll offset and self-correct; do not "fix" it.

### Widget identity (`widget.rs`)
`WidgetKind` is the focus/identity backbone. The order of `WidgetKind::all()` defines the Tab cycle and which widgets appear per size band. `layout.rs` maps a `Rect` → `Band` (TooSmall/Compact/Medium/Wide) → widget set + column count → `place()` grid.

## Conventions

- **Colors live in `theme.rs`** for the TUI (widgets take a `&Theme`, never hardcode) and in `render.rs`'s `VIOLET`/comfy-table cells for the one-shot path. Keep those two in mind as the only color sources.
- **Cost correctness**: assistant messages are logged repeatedly while streaming — `collect/usage.rs` **dedupes by `message.id`** or cost roughly doubles. Don't remove that. Pricing is vendored (per-million → per-token) with cache-write=1.25×input, cache-read=0.1×input, refreshed from LiteLLM and cached to `~/.claude/.cockpit-prices.json`.
- Some simplifications are deliberate and commented (e.g. `churn_cell` uses a single cell color because comfy-table can't do per-segment coloring). Read the comment before changing such code.

### Adding a widget
1. Add a variant to `WidgetKind` and to `all()` (position sets focus order) + `title()`.
2. Add a field to `DashboardData` (`app.rs`), a `collect/<x>.rs` gatherer, and a `publish_<x>` in `refresh.rs`; wire it into one of the refresh threads.
3. Add `render/widgets/<x>.rs` and a match arm in `dashboard.rs`'s draw loop.
4. If it's a selectable table, add it to `is_table_widget`, `row_count`, and (for drill-in) the `Action::Drill` match in `event.rs`.
