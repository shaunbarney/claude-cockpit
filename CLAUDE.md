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

cargo test                        # all unit + headless render tests (~175, colocated #[cfg(test)] mods)
cargo test focus_cycles_all       # single test by name
cargo test layout::tests          # a single module's tests

cargo clippy                      # lint (CI-relevant; keep clean)
cargo fmt                         # rustfmt
```

The TUI reads the **current working directory's** git repo, so run/test it from inside a repo with worktrees to see real data.

## Architecture

### Two separate render stacks — don't conflate them
- **Interactive TUI**: `runloop.rs` (terminal + run loop) → `event.rs` (input semantics) → `render/dashboard.rs` (composition) → `render/header.rs` + `render/widgets/*` + `render/detail/*` + `render/footer.rs` + `render/overlay.rs`. Built on **ratatui 0.30** `Frame`s, styled via `theme.rs`.
- **One-shot CLI** (`--worktrees`/`--code`/`--watch`): `render.rs` (top level) builds **strings** with **comfy-table** + **console**, colored with the `VIOLET` const. This path never touches ratatui or `App`.

Adding a column or color to one path does **not** affect the other.

### Threading & data flow
The UI thread owns `App` and runs `runloop::event_loop` (250 ms poll). It **redraws only when `DashboardData.rev` changes or input arrives** — every `publish_*` bumps `rev`, so an idle dashboard does no rendering work (background refreshes bump `rev` every 2–10 s, which keeps ages/countdowns ticking). `refresh::spawn` starts three background threads that gather collectors off the UI thread and publish into a shared `Arc<Mutex<DashboardData>>`:
- **slow (10 s)**: `gather_all` — worktrees, LOC, jobs, activity, docker, endpoints. `git fetch` is throttled to every 6th tick (~60 s), not every 10 s.
- **fast (2 s)**: jobs only — the one widget that benefits from a tight cadence (live agent state). Endpoint discovery stays on the slow thread (it walks the repo for docker files).
- **cost (10 s)**: usage scan (also computes `RateStats` for the Rate widget) + tool-use scan (Tools widget) + skill scan (Skills widget — `~/.claude/skills`/plugins + project, with usage from `Skill` tool-use blocks); refreshes LiteLLM prices once at startup.

Flow: `collect::*` (IO-bound domain snapshot) → `refresh::publish_<x>` swaps **only its own slice** of `DashboardData` and bumps `rev` → `App::sync` (diffs row signatures into `Flashes`, then refilters) → `render::dashboard::render` **clones the slices out of the mutex, drops the guard, then renders**.

The one exception to "no change, no redraw": while something is animating — a refresh spinner, or a row still fading after it changed — the loop ticks at `anim::FRAME` (80 ms). `Flashes::animating()` gates it, so an idle cockpit still costs nothing. Hold the lock as briefly as possible; never render while holding it. The manual `r` refresh (which calls `refresh_now`, i.e. `git fetch` + `gather_all`) is debounced with the `REFRESH_INFLIGHT` atomic.

### UI state machine (`app.rs`)
`View` = `Dashboard | Expanded(WidgetKind) | Detail(Detail)`, where `Detail` = `Worktree | Job(idx) | Container(idx) | Diff | …`. `App::back()` pops `Diff → Worktree → Dashboard`. Worktree detail is special: it keys off `last_wt_idx`/`wt_detail` rather than carrying an index, because drilling into a file diff needs the cached `WorktreeDetail`.

`Mode` = `Normal | Filter | Palette` decides whether a letter navigates or types. Without it, `/`-searching for a job called "quiet" would quit the app on the `q`.

**`App::visible` is the one thing to get right when touching tables.** It maps each `WidgetKind` to the *data* indices currently on screen, after filter and sort. Every row count, selection bound, and drill-in goes through `visible_count` / `data_index` / `selected_data_index` — never through `data.jobs.len()` or a raw selection index, or a filtered table will open the wrong row. Recompute with `App::refilter()` after any change to a filter or sort.

### Rows, filtering, sorting (`rows.rs` + `view.rs`)
`TableRow` gives a row type a stable `key()` (identity across refreshes), a `sig()` (changes when anything visible changes, driving the flash), `search()` text, and sortable columns. `view.rs` maps a `WidgetKind` onto its backing slice via one `on_rows!` macro, so add a `WidgetKind` variant and the compiler points at the arm to fill in.

Signatures must **quantise noisy fields** — `Container::sig` rounds CPU and buckets memory, because raw samples jitter every tick and would leave every row permanently flashing.

### Input (`event.rs`)
Pure `map_key(code, mods, mode) -> Action`, then `apply(app, action, root)` mutates `App`. The palette (`palette.rs`) is a registry of `Command`s built per focused widget — its sort entries come from that widget's own columns — so it doubles as the feature list and cannot drift from what the app can do. Mouse routing uses `FrameRects` (the per-frame hit map written by `dashboard.rs` each draw) to find the clicked widget/row. Note the intentional one-frame lag in `table_offset` (see the "Fix-7" comment in `dashboard.rs`) — clicks resolve against the previous frame's scroll offset and self-correct; do not "fix" it.

### Widget identity (`widget.rs`) and placement (`layout.rs`)
`WidgetKind` is the focus/identity backbone. The order of `WidgetKind::all()` defines the Tab cycle, which widgets appear per size band, **and the grid position** — reordering it moves panels, which resets everyone's spatial memory of the dashboard. Don't, without a reason.

`layout.rs` maps a `Rect` → `Band` (TooSmall/Compact/Medium/Wide) → widget set + column count → `place_with()`, which weighs each grid row by its hungriest member's content and distributes surplus in proportion to appetite. Only *heights* adapt; positions are fixed.

## Conventions

- **Colors live in `theme.rs`** for the TUI (widgets take a `&Theme`, never hardcode) and in `render.rs`'s `VIOLET`/comfy-table cells for the one-shot path. Keep those two in mind as the only color sources.
- **Name meaning, not colour**: use `theme.muted`/`ok`/`series[1]`, and `theme.level_span(Level::X)` for status. The palette resolves across three tiers (mono / 16 ANSI / truecolor) and `NO_COLOR` is honoured unconditionally, so **nothing may depend on colour alone** — every `Level` carries a distinct glyph too.
- **Column sets are chosen by measuring**, via `widgets::tight(area, band, full_widths, spacing)`. Don't key them off `Band`: the three-column grid leaves each widget ~65 columns even on a 200-wide terminal, so the band says "Wide" while the cell is cramped.
- **Every table needs an empty state.** `TableCtx::empty` is supplied by `dashboard::empty_msg`, which distinguishes "no containers — is the docker daemon up?" from "nothing matches X".
- **Cost correctness**: assistant messages are logged repeatedly while streaming — `collect/usage.rs` **dedupes by `message.id`** or cost roughly doubles. Don't remove that. Pricing is vendored (per-million → per-token) with cache-write=1.25×input, cache-read=0.1×input, refreshed from LiteLLM and cached to `~/.claude/.cockpit-prices.json`.
- Some simplifications are deliberate and commented (e.g. `churn_cell` uses a single cell color because comfy-table can't do per-segment coloring). Read the comment before changing such code.

### Adding a widget
1. Add a variant to `WidgetKind` and to `all()` (position sets focus order *and* grid slot) + `title()`.
2. Add a field to `DashboardData` (`app.rs`), a `collect/<x>.rs` gatherer, and a `publish_<x>` in `refresh.rs`; wire it into one of the refresh threads.
3. If it's a table: `impl TableRow` for the row type in `rows.rs`, then add the arm to `view.rs`'s `on_rows!` and `sort_columns` — that is all filtering, sorting, and change-flashing done.
4. Add `render/widgets/<x>.rs` taking a `TableCtx`, and an arm in `dashboard::draw_widget`. Add an empty-state message to `dashboard::empty_msg`.
5. For drill-in, add the arm to `event::drill` (using `selected_data_index`, never a raw index) and a `Detail` variant + `dashboard::draw_detail` route.
