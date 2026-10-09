# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`claude-cockpit` is a single-binary Rust TUI dashboard that surfaces git worktrees, Claude Code background agents, token/USD cost, rate-limit proximity, Docker, dev endpoints, and tool usage. It reads only local files (`~/.claude/…`, `~/.claude.json`, the repo's default compose files, and its own `projects.toml`/`state.toml`) and standard CLIs (`git`, `docker`, `lsof`/`ss`) — no daemon, no network for the core, **read-only** towards everything it watches (it never kills a job, stops a container, or touches branches — the only files it writes are its own project registry and state). Every data source is best-effort: a missing tool or file yields an empty widget, never a crash.

## Commands

```bash
cargo build                       # debug build
cargo build --release             # release binary at target/release/claude-cockpit (lto, opt-level 3)
cargo run                         # interactive TUI: the repo you're in, else last project, else the picker
cargo run -- -p <name|path>       # open a registered project (also with the one-shot flags)
cargo run -- --worktrees          # one-shot: worktrees table; also --code, --watch, --interval N
cargo run -- project ls --all     # project registry CLI (add/ls/rm/rename/pin/tag/discover/hide/prune)

cargo test                        # all unit + headless render tests (~310, colocated #[cfg(test)] mods)
cargo test focus_cycles_all       # single test by name
cargo test layout::tests          # a single module's tests

cargo clippy                      # lint (CI-relevant; keep clean)
cargo fmt                         # rustfmt
```

Run it from inside a repo with worktrees to see real data. When testing project features by hand, point `XDG_CONFIG_HOME`/`XDG_STATE_HOME` at a scratch dir so you don't rewrite the real registry.

## Architecture

### Two separate render stacks — don't conflate them
- **Interactive TUI**: `runloop.rs` (terminal + run loop) → `event.rs` (input semantics) → `render/dashboard.rs` (composition) → `render/header.rs` + `render/widgets/*` + `render/detail/*` + `render/footer.rs` + `render/overlay.rs`. Built on **ratatui 0.30** `Frame`s, styled via `theme.rs`.
- **One-shot CLI** (`--worktrees`/`--code`/`--watch`): `render.rs` (top level) builds **strings** with **comfy-table** + **console**, colored with the `VIOLET` const. This path never touches ratatui or `App`.

Adding a column or color to one path does **not** affect the other.

### Projects (`projects/`, `active.rs`)
The cockpit shows one **project** (a git repo; linked worktrees fold into their main repo via `--git-common-dir`) at a time. `projects/` is layered: `model` (pure `Registry` edits + validation) → `store` (atomic TOML load/save; registry in `$XDG_CONFIG_HOME`, last/recent/hidden in `$XDG_STATE_HOME` so a switch never rewrites the hand-edited file; a **broken registry is an error, never an empty default**, or the next save would wipe it) → `discover` (suggestions from `~/.claude.json` keys + transcript `cwd` fields + optional scan roots) / `overview` (one `ProjectRow` per project) / `resolve` (startup: `-p`, cwd repo, last, first pinned, else picker) → `picker` (pure picker state + keys, writes to injected `Files`) → `host` (glue: open/route/switch, the overview thread) and `cli` (clap `project` subcommands).

`ActiveProject` (root + alias + generation counter) is shared with the refresh threads, which read the root at the top of every tick. `host::switch` sets it, `App::reset_for_project` drops all per-project UI state, the repo-scoped slices of `DashboardData` are cleared, and `refresh::request_now()` wakes the slow loop. Every publish from a tick is guarded by "generation unchanged since the tick started", so a slow tick that began on the old project never overwrites the new one. The picker is modal: the run loop sends it every key and swallows mouse events while it is open.

### Threading & data flow
The UI thread owns `App` and runs `runloop::event_loop` (250 ms poll). It **redraws only when `DashboardData.rev` changes or input arrives** — every `publish_*` bumps `rev`, so an idle dashboard does no rendering work (background refreshes bump `rev` every 2–10 s, which keeps ages/countdowns ticking). `refresh::spawn` starts three background threads that gather collectors off the UI thread and publish into a shared `Arc<Mutex<DashboardData>>`:
- **slow (10 s)**: `gather_all` — repo, worktrees, LOC (re-counted only when HEAD or the index changes), jobs, activity, docker, endpoints. `git fetch` runs at most every 60 s (bounded by a timeout); after a project switch the new project's local state is published first and the fetch follows.
- **fast (2 s)**: jobs only — the one widget that benefits from a tight cadence (live agent state). Jobs are scoped to the active repo (`jobs::filter_for_repo`) unless `refresh::set_jobs_all(true)`.
- **cost (10 s)**: usage scan (also computes `RateStats` for the Rate widget) + tool-use scan (Tools widget) + skill scan (Skills widget — `~/.claude/skills`/plugins + project, with usage from `Skill` tool-use blocks); refreshes LiteLLM prices once at startup. All three read transcripts incrementally (from the last byte offset per file).
- **overview (30 s, or on demand)**: `projects::host::spawn_overview` — one row per registered project/suggestion for the picker; `host::wake_overview()` regathers immediately.

Each collector call runs under `catch_unwind` (`refresh::guarded`); failures are recorded in `refresh::health()` instead of killing the thread, and locks tolerate poisoning. External commands go through `proc::output_with_timeout` so a hung `docker` can't stall a thread, and every `git` invocation goes through `util::git_cmd()`, which strips inherited `GIT_DIR`/`GIT_WORK_TREE` (under a hook or `rebase -x` they would redirect `-C <path>` calls — and the temp-repo test fixtures — at the enclosing repo).

Flow: `collect::*` (IO-bound domain snapshot) → `refresh::publish_<x>` swaps **only its own slice** of `DashboardData` and bumps `rev` → `App::sync` (diffs row signatures into `Flashes`, then refilters) → `render::dashboard::render` **clones the slices out of the mutex, drops the guard, then renders**.

The one exception to "no change, no redraw": while something is animating — a refresh spinner, or a row still fading after it changed — the loop ticks at `anim::FRAME` (80 ms). `Flashes::animating()` gates it, so an idle cockpit still costs nothing. Hold the lock as briefly as possible; never render while holding it. The manual `r` refresh wakes the slow thread through a condvar handshake (`refresh::request_now` / `refresh_now` waits for that tick) rather than gathering in parallel with it, so two gathers never race to publish.

### UI state machine (`app.rs`)
`View` = `Dashboard | Expanded(WidgetKind) | Detail(Detail)`. Row-backed details (`Job`, `Container`, `Ports`, `Repo`, `CostModel`) carry the row's **stable key** (`rows::TableRow::key` — job id, container id, `host:port`, `source/name`, model name), never a data index: refreshes reorder rows, so `render/detail/screen.rs` resolves the key against live data every frame and shows "no longer present" when it is gone. The worktree detail keys off `App::wt_path` + the cached `wt_detail` (the file diff needs it). `App::back()` pops `Diff → Worktree`, `CostModel → Cost`, and any other detail back to where it was opened from (`App::origin`: an Expanded widget, else the Dashboard).

Table selection also follows identity: `App::refilter` keeps the visible rows' keys alongside their indices and reselects the previously selected key after any refilter/refresh, clamping only when that row is gone. Filter/sort apply only in `Dashboard`/`Expanded` and only to `view::is_table` widgets (the palette hides those entries elsewhere).

Drill-ins that shell out (`git_detail::worktree_detail`, `file_diff`, `docker logs`) run on a worker thread via `App::spawn_load`; results come back over a channel drained by the run loop (`App::drain_loads`), tagged with a token so superseded requests are dropped. Detail screens render "loading…" meanwhile. Scrolling details report their maximum scroll into `App::detail_max` while rendering; input clamps against it.

### Input (`event.rs`)
Pure `map_key(code, mods) -> Action`, then `apply(app, action, root)` mutates `App`. Actions that must touch the refresh threads (a project switch) are queued on `App` (`pending_switch`) and applied by the run loop, which owns `ActiveProject`. Mouse routing uses `FrameRects` (the per-frame hit map written each draw: widget rects, plus the worktree detail's file table) to find the clicked widget/row; clicking an already-selected row drills in, and the wheel scrolls the widget under the pointer. The help overlay and palette are modal. On the dashboard, Tab skips widgets the current size band does not place (`App::placed`); in Expanded, Tab cycles every widget and the view follows; palette "Go to" a hidden widget opens it Expanded.

### Panics
`runloop.rs` replaces ratatui's panic hook: the terminal (incl. mouse capture) is restored only when the **UI thread** panics. A collector-thread panic must not tear the terminal down under a UI that keeps drawing.

### Widget identity (`widget.rs`) and placement (`layout.rs`)
`WidgetKind` is the focus/identity backbone. The order of `WidgetKind::all()` defines the Tab cycle, which widgets appear per size band, **and the grid position** — reordering it moves panels, which resets everyone's spatial memory of the dashboard. Don't, without a reason.

`layout.rs` maps a `Rect` → `Band` (TooSmall/Compact/Medium/Wide) → widget set + column count → `place_with()`, which weighs each grid row by its hungriest member's content and distributes surplus in proportion to appetite. Only *heights* adapt; positions are fixed.

## Conventions

- **Colors live in `theme.rs`** for the TUI (widgets take a `&Theme`, never hardcode) and in `render.rs`'s `VIOLET`/comfy-table cells for the one-shot path. Keep those two in mind as the only color sources.
- **Name meaning, not colour**: use `theme.muted`/`ok`/`series[1]`, and `theme.level_span(Level::X)` for status. The palette resolves across three tiers (mono / 16 ANSI / truecolor) and `NO_COLOR` is honoured unconditionally, so **nothing may depend on colour alone** — every `Level` carries a distinct glyph too.
- **Column sets are chosen by measuring**, via `widgets::tight(area, band, full_widths, spacing)`. Don't key them off `Band`: the three-column grid leaves each widget ~65 columns even on a 200-wide terminal, so the band says "Wide" while the cell is cramped.
- **Every table needs an empty state.** `TableCtx::empty` is supplied by `dashboard::empty_msg`, which distinguishes "no containers — is the docker daemon up?" from "nothing matches X".
- **Cost correctness**: Claude Code writes one transcript line per content block, all sharing a `message.id`, and early lines carry partial `output_tokens`. `collect/usage.rs` **dedupes by `message.id` across all files** (resumed sessions repeat ids) **keeping the per-field maximum** — keeping the first line undercounts output ~45%, not deduping doubles cost. Tools dedupe by `tool_use.id` (the tool block is rarely on the first line). Pricing is vendored (per-million → per-token, with date/`[1m]` suffixes aliased to the base model) with 5-minute cache writes at 1.25×input, 1-hour writes (`cache_creation.ephemeral_1h_input_tokens`) at 2×, cache reads at 0.1×, refreshed from LiteLLM and cached to `~/.claude/.cockpit-prices.json`; unknown models are listed as unpriced, never silently $0. Day buckets and "today" use the **local** date.
- **Config** is `<repo root>/claude-cockpit.toml`, else `$XDG_CONFIG_HOME/claude-cockpit/config.toml` — first found wins, no merging. Unknown keys and parse errors are reported (`config::load_with_diagnostics`), never silently ignored.
- Some simplifications are deliberate and commented (e.g. `churn_cell` uses a single cell color because comfy-table can't do per-segment coloring). Read the comment before changing such code.

### Adding a widget
1. Add a variant to `WidgetKind` and to `all()` (position sets focus order *and* grid slot) + `title()`.
2. Add a field to `DashboardData` (`app.rs`), a `collect/<x>.rs` gatherer, and a `publish_<x>` in `refresh.rs`; wire it into one of the refresh threads.
3. If it's a table: `impl TableRow` for the row type in `rows.rs`, then add the arm to `view.rs`'s `on_rows!` and `sort_columns` — that is all filtering, sorting, and change-flashing done.
4. Add `render/widgets/<x>.rs` taking a `TableCtx`, and an arm in `dashboard::draw_widget`. Add an empty-state message to `dashboard::empty_msg`.
5. For drill-in, add the arm to `event::drill` (using `selected_data_index`, never a raw index) and a `Detail` variant + `dashboard::draw_detail` route.
