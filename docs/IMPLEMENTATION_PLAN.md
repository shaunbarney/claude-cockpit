# Governour Cockpit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evolve `governour` from a two-tab worktree/LOC table into a `btm`/`btop`-style live developer cockpit that monitors Claude Code activity, git worktrees, token/USD cost, and the dev tooling (Docker, dev servers, processes) for the Sovra SRX repo, in one responsive TUI that reflows to any terminal size and is navigable by keyboard *and* mouse.

**Architecture:** A single Rust crate (`.claude/commands/scripts/governour`) on ratatui 0.29 + crossterm. A small **engine** (responsive grid layout, focus, view-stack, mouse hit-testing via a stored per-frame `Rect` registry, background data refresh) drives a set of **widgets**, each backed by a pure **collector** that reads local files / shells out to dev tools off the render thread. Data logic (parsing `state.json`, deduping transcript `*.jsonl`, computing cost, parsing `docker ps`) is pure and TDD-tested; rendering is thin and spec'd per widget. The legacy one-shot comfy-table renders (`render.rs`) stay intact for the slash-command path.

**Tech Stack:** Rust 2021, ratatui 0.29, crossterm (re-exported by ratatui), tokei (LOC), comfy-table (legacy one-shot), and new deps: `serde`/`serde_json` (JSON), `sysinfo` (process CPU/mem), `ureq` (blocking HTTP for the LiteLLM price table), `chrono` (timestamp parse + date bucketing), `dirs` (home dir).

**Defaults locked in discovery:** read-only v1 (no kill/stop/restart/merge actions); auto-refresh 10s for expensive collectors (git, LOC, cost) and ~2s for cheap ones (jobs, processes); the live feel of the job board comes from tailing `timeline.jsonl`. btm-style single dashboard with `e` to expand a focused widget fullscreen and Enter/click to drill into a row.

---

## Plan Conventions (read first)

This plan spans nine subsystems. To keep it actionable without ballooning to tens of thousands of speculative lines:

- **Every collector / parser / cost calculation is given complete code + a complete failing test.** These are the bug-prone cores and are followed strictly via TDD.
- **Rendering is given a precise spec** (exact ratatui widget, columns, constraints, colours, key bindings) rather than line-by-line code, because rendering is mechanical once the engine exists and is verified visually, not by unit tests. Where a render function has testable pure logic (e.g. choosing a size band, formatting a byte count), that logic is extracted into a pure helper *with* a test.
- **File-size rule (project CLAUDE.md): ≤300 lines/file, ≤40 lines/function.** Each task names exact files; split proactively.
- **Commit after every green step.** Conventional commits, scope `governour` (maps to repo scope `tools`/`config`; use `feat(governour): …`).

---

## Architecture Reference

### Module tree (target)

```
.claude/commands/scripts/governour/src/
  main.rs                 # CLI entry. `--tui` (default for `just governour`) → cockpit; legacy --worktrees/--code/--watch kept.
  theme.rs                # Theme struct (ratatui Style/Color), Sovra violet, status palette (ok/warn/err/dim).
  app.rs                  # App state: DashboardData, focus, View stack, per-widget UI state, FrameRects, theme.
  widget.rs               # WidgetKind enum + focus order + display titles + which size bands include it.
  layout.rs               # Responsive grid engine: size band, widget→Rect placement, "too small" guard, FrameRects.
  event.rs                # crossterm Key/Mouse → AppAction; AppAction applied to App.
  refresh.rs              # Background refresh: per-collector threads/cadence, Arc<Mutex<DashboardData>> publish.
  collect/
    mod.rs                # re-exports
    git.rs                # MOVED from src/git.rs: worktrees + repo health + worktree detail (files, commits, merge).
    loc.rs                # MOVED from src/loc.rs: tokei scan (data only; comfy-table table moves to legacy render).
    jobs.rs               # ~/.claude/jobs/*/state.json + timeline.jsonl → Job + JobEvent.
    usage.rs              # transcripts → UsageTotals (deduped by message.id), incremental cache.
    pricing.rs            # vendored Claude price map + LiteLLM fetch/cache → PriceTable.
    docker.rs             # `docker ps`/`stats`/`logs` parse → Container.
    ports.rs              # dev endpoint health via TcpStream + optional lsof PID.
    procs.rs              # sysinfo snapshot → Proc (claude/node/python filtered).
  render/
    mod.rs                # legacy comfy-table one-shot (existing build_frame/watch/Mode) + re-exports.
    legacy_tables.rs      # MOVED comfy-table worktree_table/loc_table/summary/join (the existing render.rs body).
    dashboard.rs          # compose the widget grid for the current band; dispatch to widget renderers.
    widgets/
      worktrees.rs jobs.rs cost.rs activity.rs code.rs docker.rs ports.rs procs.rs repo.rs
    detail/
      worktree.rs job.rs container.rs diff.rs
  graph.rs                # braille trend helpers (ring-buffer → ratatui Chart/Sparkline dataset).
  util.rs                 # home_dir(), human_bytes(), human_duration(), thousands() (moved), age_relative().
```

`src/git.rs` and `src/loc.rs` move under `collect/`. `src/render.rs`'s comfy-table body moves to `render/legacy_tables.rs`; `render/mod.rs` keeps `Mode`, `build_frame`, `watch`. `src/tui.rs` is **replaced** by the engine (`app.rs`/`layout.rs`/`event.rs`/`render/dashboard.rs`); its `App`/event-loop logic is superseded.

### Shared data model (defined in collectors; re-exported via `collect::mod`)

```rust
// collect/git.rs (Worktree already exists; add these)
pub struct RepoHealth { pub branch: String, pub ahead: u32, pub behind: u32,
    pub stash: u32, pub dirty: u32, pub last_fetch_secs: Option<u64> }
pub struct WorktreeDetail { pub wt: Worktree, pub branch: String, pub path: String,
    pub committed_files: Vec<FileChange>, pub uncommitted_files: Vec<FileChange>,
    pub commits: Vec<CommitRow>, pub merge: MergeStatus }
pub struct FileChange { pub path: String, pub added: u32, pub deleted: u32, pub staged: bool }
pub struct CommitRow { pub short: String, pub subject: String, pub age: String, pub added: u32, pub deleted: u32 }
pub enum MergeStatus { Clean, Conflicts(Vec<String>), Behind(u32), UpToDate }

// collect/jobs.rs
pub struct Job { pub id: String, pub name: String, pub state: String, pub tempo: String,
    pub intent: String, pub tasks: u32, pub queued: u32, pub cwd: String,
    pub worktree_path: Option<String>, pub worktree_branch: Option<String>,
    pub created_at: Option<i64>, pub updated_at: Option<i64> }   // epoch secs
pub struct JobEvent { pub at: i64, pub state: String, pub detail: String, pub text: String }

// collect/usage.rs
pub struct UsageRecord { pub day: String, pub model: String, pub input: u64, pub output: u64,
    pub cache_write: u64, pub cache_read: u64 }                  // already deduped
pub struct UsageTotals { pub by_day: Vec<DayUsage>, pub by_model: Vec<ModelUsage>,
    pub total_cost_usd: f64, pub cache_read: u64, pub cache_write: u64, pub fresh_input: u64 }
pub struct DayUsage { pub day: String, pub cost_usd: f64, pub tokens: u64 }
pub struct ModelUsage { pub model: String, pub cost_usd: f64, pub input: u64, pub output: u64 }

// collect/pricing.rs
pub struct ModelPrice { pub input: f64, pub output: f64, pub cache_write: f64, pub cache_read: f64 } // USD per token
pub struct PriceTable(pub std::collections::HashMap<String, ModelPrice>);

// collect/docker.rs
pub struct Container { pub id: String, pub name: String, pub state: String, pub status: String,
    pub health: Option<String>, pub cpu_pct: f32, pub mem_used: u64, pub mem_limit: u64,
    pub ports: Vec<String>, pub restarts: u32 }

// collect/ports.rs
pub struct Endpoint { pub label: String, pub host: String, pub port: u16,
    pub up: bool, pub latency_ms: Option<u64>, pub pid: Option<u32> }

// collect/procs.rs
pub struct Proc { pub pid: u32, pub name: String, pub cmd: String,
    pub cpu_pct: f32, pub mem_bytes: u64, pub uptime_secs: u64 }
```

### App / engine model

```rust
// app.rs
pub struct DashboardData {
    pub worktrees: Vec<Worktree>, pub repo: Option<RepoHealth>, pub loc: Vec<LocRow>,
    pub jobs: Vec<Job>, pub usage: Option<UsageTotals>,
    pub containers: Vec<Container>, pub endpoints: Vec<Endpoint>, pub procs: Vec<Proc>,
    pub cpu_history: std::collections::HashMap<u32, RingBuffer>, // pid → cpu samples
}

pub enum View { Dashboard, Expanded(WidgetKind), Detail(Detail) }
pub enum Detail { Worktree(usize), Job(usize), Container(usize), Diff(DiffView) }
pub struct DiffView { pub title: String, pub lines: Vec<String>, pub scroll: u16 }

pub struct App {
    pub data: std::sync::Arc<std::sync::Mutex<DashboardData>>,
    pub view: View,
    pub focus: WidgetKind,
    pub ui: WidgetUiState,     // per-widget TableState + sort + scroll
    pub rects: FrameRects,     // last frame's hit map (widget kind → Rect, table row rows → Rect)
    pub theme: Theme,
    pub should_quit: bool,
}
```

`FrameRects` is the immediate-mode hit map: every `draw()` records `Vec<(WidgetKind, Rect)>` plus, for the focused/expanded table, the inner content `Rect` and current `TableState.offset()` so a click row can be resolved (`row_index = (click.y - inner.y) + offset`).

### Responsive strategy (the "dynamic sizing")

`layout::band(area) -> Band` where `Band ∈ { TooSmall, Compact, Medium, Wide }`:
- `TooSmall`: width < 70 or height < 18 → render a centered "Resize terminal (need ≥70×18)" guard and nothing else.
- `Compact`: width 70–109 → single column; only the highest-priority widgets render (Worktrees, Jobs, Cost summary), each a compact variant (no graphs, fewer columns). Other widgets reachable via `e`/number-jump fullscreen.
- `Medium`: width 110–169 → 2 columns; graphs enabled at reduced height.
- `Wide`: width ≥ 170 → multi-column ratio grid showing all nine widgets.

Within a band, panes use `Constraint::Fill(weight)` + `Min(h)` (never raw `Percentage`/`Length` for the body) so the grid breathes. `frame.area()` is re-read every draw; `Event::Resize` forces an immediate redraw. Per-widget fullscreen (`e`) ignores the band and gives the focused widget the whole area.

### Background refresh

`refresh::spawn(data: Arc<Mutex<DashboardData>>)` starts threads:
- **fast (2s):** jobs, procs, ports — cheap.
- **slow (10s):** worktrees+repo, loc, docker — moderate.
- **cost (10s, incremental):** usage scan with a per-file cache `HashMap<PathBuf, (mtime, len, Vec<UsageRecord>)>`; only re-parse files whose mtime/len changed; recompute totals from the union. Price table fetched once at startup (vendored fallback), refreshed hourly.

Each thread locks the mutex only to publish its slice (fast swap), never while doing IO. The UI thread reads a clone/snapshot each draw. `r` triggers an immediate refresh of all slices.

---

## Phase 0 — Worktree base + dependencies + crate builds

**Goal:** the worktree contains the existing governour crate and compiles with the new deps. Ships nothing user-visible; unblocks everything.

### Task 0.1: Base the worktree branch on local main

The governour crate exists only on **local `main`** (unpushed); this worktree branched from `origin/main` which predates it.

- [ ] **Step 1: Confirm the crate is absent and present on local main**

Run: `ls .claude/commands/scripts/governour/src 2>/dev/null || echo MISSING` → expect MISSING
Run: `git cat-file -e main:.claude/commands/scripts/governour/src/main.rs && echo OK` → expect OK

- [ ] **Step 2: Merge local main into the worktree branch** ⚠️ **requires the user's explicit git approval (standing rule).**

```bash
git merge --no-edit main
```
Expected: fast-forward or clean merge; `ls .claude/commands/scripts/governour/src` now lists `git.rs loc.rs main.rs render.rs tui.rs`.

- [ ] **Step 3: Verify baseline builds and tests pass**

Run: `cd .claude/commands/scripts/governour && cargo test`
Expected: PASS (existing 9 tests green).

### Task 0.2: Add dependencies

**Files:** Modify `.claude/commands/scripts/governour/Cargo.toml`

- [ ] **Step 1: Add deps to `[dependencies]`**

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sysinfo = "0.32"
ureq = { version = "2", default-features = false, features = ["tls"] }
chrono = { version = "0.4", default-features = false, features = ["clock"] }
dirs = "5"
```

- [ ] **Step 2: Verify it resolves and builds**

Run: `cargo build`
Expected: compiles (new deps downloaded). Commit.

```bash
git add Cargo.toml Cargo.lock
git commit -m "build(governour): add serde, sysinfo, ureq, chrono, dirs"
```

### Task 0.3: Create `util.rs` with tested formatters

**Files:** Create `src/util.rs`; Modify `src/main.rs` (add `mod util;`)

- [ ] **Step 1: Write failing tests**

```rust
// src/util.rs tests
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bytes_human() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
    #[test] fn duration_human() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(90), "1m");
        assert_eq!(human_duration(3 * 3600 + 120), "3h");
    }
    #[test] fn thousands_sep() { assert_eq!(thousands(12345), "12,345"); }
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test util::` → FAIL (undefined fns).

- [ ] **Step 3: Implement**

```rust
//! Shared formatters and path helpers.
use std::path::PathBuf;

/// Claude home (`~/.claude`), respecting $HOME / dirs.
pub fn claude_home() -> Option<PathBuf> { dirs::home_dir().map(|h| h.join(".claude")) }

/// Bytes → "1.5 KB" / "5.0 MB" (binary units, 1 decimal above KB).
pub fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 { return format!("{n} B"); }
    let mut v = n as f64; let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 { v /= 1024.0; i += 1; }
    format!("{v:.1} {}", U[i])
}

/// Seconds → compact "45s" / "1m" / "3h" / "2d".
pub fn human_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// 12345 → "12,345".
pub fn thousands(n: u64) -> String {
    let s = n.to_string(); let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 { out.push(','); }
        out.push(*c as char);
    }
    out
}
```

- [ ] **Step 4: Run, expect pass** — `cargo test util::` → PASS.

- [ ] **Step 5: Commit** — `git commit -am "feat(governour): util formatters (bytes, duration, thousands)"`

---

## Phase 1 — Foundation: theme, engine, responsive dashboard (worktrees + code)

**Goal:** Replace `tui.rs` with the engine. `just governour` opens a responsive dashboard that renders the **worktrees** and **code (LOC)** widgets in a reflowing grid, with focus (arrows/click), `e` expand, `r` refresh, `q` quit, a "too small" guard, and background refresh. This is the load-bearing phase; everything else plugs in.

### Task 1.1: Theme

**Files:** Create `src/theme.rs`; Modify `src/main.rs` (`mod theme;`)

- [ ] **Step 1: Failing test**

```rust
#[cfg(test)]
mod tests { use super::*;
  #[test] fn status_colors_distinct() {
    let t = Theme::default();
    assert_ne!(t.ok, t.err); assert_ne!(t.warn, t.err);
    assert_eq!(t.accent, ratatui::style::Color::Rgb(0x7C,0x5C,0xFF));
  }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement**

```rust
//! Colour theme. Single source of truth — widgets never hardcode colours.
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone)]
pub struct Theme {
    pub accent: Color, pub ok: Color, pub warn: Color, pub err: Color,
    pub dim: Color, pub fg: Color, pub focus_border: Color,
}
impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: Color::Rgb(0x7C, 0x5C, 0xFF), ok: Color::Green, warn: Color::Yellow,
            err: Color::Red, dim: Color::DarkGray, fg: Color::Reset,
            focus_border: Color::Rgb(0x7C, 0x5C, 0xFF),
        }
    }
}
impl Theme {
    pub fn title(&self) -> Style { Style::new().fg(self.accent).add_modifier(Modifier::BOLD) }
    pub fn dim_style(&self) -> Style { Style::new().fg(self.dim) }
}
```

- [ ] **Step 4: Run, expect pass. Step 5: Commit** — `feat(governour): theme tokens`.

### Task 1.2: WidgetKind registry

**Files:** Create `src/widget.rs`; Modify `src/main.rs` (`mod widget;`)

- [ ] **Step 1: Failing test**

```rust
#[cfg(test)]
mod tests { use super::*;
  #[test] fn focus_cycles_all() {
    let all = WidgetKind::all();
    assert_eq!(all.first().copied(), Some(WidgetKind::Worktrees));
    let mut k = WidgetKind::Worktrees;
    for _ in 0..all.len() { k = k.next(); }
    assert_eq!(k, WidgetKind::Worktrees); // wraps
  }
  #[test] fn titles_present() {
    assert_eq!(WidgetKind::Cost.title(), "Cost");
  }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** — `WidgetKind` enum `{ Worktrees, Jobs, Cost, Activity, Code, Docker, Ports, Procs, Repo }` with `all() -> &'static [WidgetKind]`, `next()/prev()` (wrap), `title() -> &str`. (Pure; ≤40 lines.)

- [ ] **Step 4: pass. Step 5: commit** — `feat(governour): WidgetKind registry + focus order`.

### Task 1.3: Size band selection (pure)

**Files:** Create `src/layout.rs`; Modify `src/main.rs` (`mod layout;`)

- [ ] **Step 1: Failing test**

```rust
#[cfg(test)]
mod tests { use super::*; use ratatui::layout::Rect;
  fn r(w:u16,h:u16)->Rect{Rect{x:0,y:0,width:w,height:h}}
  #[test] fn bands() {
    assert_eq!(band(r(60,40)), Band::TooSmall);
    assert_eq!(band(r(100,10)), Band::TooSmall);
    assert_eq!(band(r(90,30)),  Band::Compact);
    assert_eq!(band(r(130,40)), Band::Medium);
    assert_eq!(band(r(200,50)), Band::Wide);
  }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement**

```rust
use ratatui::layout::Rect;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band { TooSmall, Compact, Medium, Wide }
pub fn band(a: Rect) -> Band {
    if a.width < 70 || a.height < 18 { return Band::TooSmall; }
    match a.width { 70..=109 => Band::Compact, 110..=169 => Band::Medium, _ => Band::Wide }
}
```

- [ ] **Step 4: pass. Step 5: commit** — `feat(governour): responsive size bands`.

### Task 1.4: Grid placement (pure) + FrameRects

**Files:** Modify `src/layout.rs`

- [ ] **Step 1: Failing test** — given a `Wide` area and the list of widgets for that band, `place(area, &kinds)` returns one `Rect` per kind, all within `area`, non-overlapping by row, and covering the full width.

```rust
#[test] fn place_covers_area() {
  let a = Rect{x:0,y:0,width:200,height:60};
  let kinds = vec![WidgetKind::Worktrees, WidgetKind::Jobs, WidgetKind::Cost, WidgetKind::Code];
  let placed = place(a, &kinds, 2 /*cols*/);
  assert_eq!(placed.len(), 4);
  for (_, rc) in &placed { assert!(rc.right() <= a.right() && rc.bottom() <= a.bottom()); }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** `place(area, kinds, cols) -> Vec<(WidgetKind, Rect)>`: chunk `kinds` into rows of `cols`, split `area` vertically by `Constraint::Fill(1)` per row, split each row horizontally by `Constraint::Fill(1)` per cell. Also add `widgets_for(band) -> Vec<WidgetKind>` (Compact = `[Worktrees, Jobs, Cost]`; Medium = first 6; Wide = `all()`) and `cols_for(band)` (Compact=1, Medium=2, Wide=3). `FrameRects { widgets: Vec<(WidgetKind, Rect)>, table_inner: Option<Rect>, table_offset: usize }` with `widget_at(pos) -> Option<WidgetKind>` using `rect.contains(pos)`.

- [ ] **Step 4: pass. Step 5: commit** — `feat(governour): grid placement + frame hit map`.

### Task 1.5: App state skeleton + DashboardData

**Files:** Create `src/app.rs`; Modify `src/main.rs` (`mod app;`)

- [ ] **Step 1: Failing test**

```rust
#[cfg(test)]
mod tests { use super::*;
  #[test] fn expand_and_back() {
    let mut app = App::new(Theme::default());
    assert!(matches!(app.view, View::Dashboard));
    app.expand_focused();
    assert!(matches!(app.view, View::Expanded(_)));
    app.back();
    assert!(matches!(app.view, View::Dashboard));
  }
  #[test] fn focus_next_wraps() {
    let mut app = App::new(Theme::default());
    let start = app.focus;
    for _ in 0..WidgetKind::all().len() { app.focus_next(); }
    assert_eq!(app.focus, start);
  }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** `DashboardData` (all `Vec::new()`/`None` default), `View`, `Detail`, `App` with `new(theme)`, `focus_next/prev`, `expand_focused`, `back` (Diff→Detail→Expanded→Dashboard pop semantics; for Phase 1 only Expanded↔Dashboard), `should_quit`. `Arc<Mutex<DashboardData>>` for `data`.

- [ ] **Step 4: pass. Step 5: commit** — `feat(governour): App state + view stack`.

### Task 1.6: Port worktrees + LOC collectors under `collect/`

**Files:** Create `src/collect/mod.rs`, move `src/git.rs`→`src/collect/git.rs`, `src/loc.rs`→`src/collect/loc.rs` (data parts); Modify `src/main.rs` (`mod collect;`, drop `mod git; mod loc;`); Modify `src/render.rs` imports.

- [ ] **Step 1:** Move files with `git mv`; update `mod` paths. Keep `comfy-table` `loc_table` in `render/legacy_tables.rs` (next task) — for now leave `loc.rs`'s comfy-table fn where it compiles.

- [ ] **Step 2: Run existing tests** — `cargo test` → the moved `parse_shortstat`, `rank`, `totals` tests still PASS.

- [ ] **Step 3: Commit** — `refactor(governour): move git/loc into collect/`.

### Task 1.7: Background refresh scaffold (worktrees + loc only)

**Files:** Create `src/refresh.rs`; Modify `src/main.rs` (`mod refresh;`)

- [ ] **Step 1: Failing test** — `refresh::publish_worktrees` swaps the slice under lock without touching other slices.

```rust
#[test] fn publish_swaps_only_its_slice() {
  let data = std::sync::Arc::new(std::sync::Mutex::new(DashboardData::default()));
  { data.lock().unwrap().jobs.push(/* a dummy Job */); }
  publish_worktrees(&data, vec![/* one Worktree */]);
  let d = data.lock().unwrap();
  assert_eq!(d.worktrees.len(), 1);
  assert_eq!(d.jobs.len(), 1); // untouched
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** `publish_worktrees`, `publish_loc` (lock, assign field, drop). `spawn(root, data)` starts a slow thread (10s) calling `git::fetch_origin` + `gather_worktrees` + `loc_rows` then publishing; thread loop checks an `Arc<AtomicBool>` stop flag. Provide `refresh_now(root, data)` for `r`.

- [ ] **Step 4: pass. Step 5: commit** — `feat(governour): background refresh scaffold`.

### Task 1.8: Widget renderers — worktrees + code (ratatui)

**Files:** Create `src/render/dashboard.rs`, `src/render/widgets/worktrees.rs`, `src/render/widgets/code.rs`; Modify `src/render/mod.rs`.

- [ ] **Step 1:** No unit test (visual). Implement `widgets::worktrees::render(f, area, &[Worktree], &Theme, focused, &mut TableState)`:
  - `Block::bordered().title(" Worktrees ")`; border style = `theme.focus_border` when `focused`, else dim.
  - `Table` columns: `● | Worktree | Ahead | Dirty | +/- committed | +/- uncommitted | Age` (port the colour logic from legacy `worktree_table`: red dot/ahead when `ahead>0`, yellow when `dirty>0`, green clean; green churn).
  - Widths `[Length(2), Min(14), Length(6), Length(6), Length(13), Length(13), Length(12)]`; `.row_highlight_style(Style::reversed())`; render via `render_stateful_widget`.
  - In `Compact` band, drop the two churn columns (keep dot/name/ahead/dirty/age).
- [ ] **Step 2:** Implement `widgets::code::render(f, area, &[LocRow], &Theme, focused)`: `Table` `Language | Files | Code` + TOTAL row (accent on total code), from `loc::totals`.
- [ ] **Step 3:** Implement `dashboard::render(f, app)`: compute `band(area)`; if `TooSmall` render centered guard `Paragraph` (use `Flex::Center` both axes) and return; else `widgets_for(band)`/`place(...)`, record `app.rects`, and for each `(kind, rect)` dispatch to the widget renderer (Phase 1: only Worktrees + Code implemented; others render a dim "… " placeholder block titled with `kind.title()`). Draw a 1-line footer (summary counts + key hints) like the legacy `summary_line`.
- [ ] **Step 4:** `cargo build` + manual smoke (run `cargo run -- --tui`, resize terminal: grid reflows, narrow shows guard). Commit — `feat(governour): dashboard grid with worktrees + code widgets`.

### Task 1.9: Event loop with key + mouse

**Files:** Create `src/event.rs`; rewrite `src/tui.rs` → thin `run()` wrapper (or fold into `main.rs`); Modify `src/main.rs`.

- [ ] **Step 1: Failing test** — pure key→action mapping.

```rust
#[cfg(test)]
mod tests { use super::*; use ratatui::crossterm::event::{KeyCode, KeyModifiers};
  #[test] fn maps_keys() {
    assert!(matches!(map_key(KeyCode::Char('q'), KeyModifiers::NONE), Action::Quit));
    assert!(matches!(map_key(KeyCode::Char('e'), KeyModifiers::NONE), Action::Expand));
    assert!(matches!(map_key(KeyCode::Tab, KeyModifiers::NONE), Action::FocusNext));
    assert!(matches!(map_key(KeyCode::Char('c'), KeyModifiers::CONTROL), Action::Quit));
    assert!(matches!(map_key(KeyCode::Enter, KeyModifiers::NONE), Action::Drill));
  }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** `enum Action { Quit, Refresh, FocusNext, FocusPrev, Up, Down, Expand, Drill, Back, None }`; `map_key(code, mods) -> Action` (q/Esc/Ctrl-C→Quit but Esc→Back when not Dashboard — resolve Esc in the apply step; `r`→Refresh; Tab/→/l→FocusNext; BackTab/←/h→FocusPrev; ↑/k→Up; ↓/j→Down; e→Expand; Enter→Drill). `map_mouse(MouseEvent, &FrameRects) -> Action`: `Down(Left)` → if inside a widget set focus + (if that widget is the focused table) compute row from `(row - inner.y) + offset` and select; double semantics deferred (single click selects, Enter/`e` acts). ScrollUp/Down → Up/Down. `run(root)`: `ratatui::init()`, `execute!(stdout, EnableMouseCapture)`, spawn refresh, loop `draw` + `poll(timeout)`; apply actions; on teardown `DisableMouseCapture` + `ratatui::restore()` (guard against panic).

- [ ] **Step 4: pass; manual smoke** — clicking a worktree row selects it; Tab cycles focus; `e` expands Worktrees fullscreen; Esc returns; mouse wheel scrolls selection. Commit — `feat(governour): key + mouse event loop`.

### Task 1.10: Wire `--tui` to the engine; keep legacy modes

**Files:** Modify `src/main.rs`, `src/render/mod.rs` (move comfy-table body to `render/legacy_tables.rs`).

- [ ] **Step 1:** `git mv` the comfy-table render body into `render/legacy_tables.rs`; `render/mod.rs` re-exports `Mode`, `build_frame`, `watch`. `main.rs`: `--tui` → `event::run(&root)`; `--worktrees/--code/--watch` unchanged (legacy one-shot). Default (no flags) keeps current `Side` one-shot for the slash command; `just governour` recipe already passes `--tui` (verify in `Justfile`).
- [ ] **Step 2:** `cargo test` (all green) + `cargo run -- --worktrees` (legacy still works) + `cargo run -- --tui` (cockpit). Commit — `feat(governour): route --tui to cockpit engine, keep legacy renders`.
- [ ] **Step 3:** Update `Justfile` `governour` recipe if needed to pass `--tui`; verify `just governour` launches the cockpit.

**Phase 1 acceptance:** responsive 2-widget dashboard, focus + mouse + expand + refresh + too-small guard, legacy renders intact, all tests green.

---

## Phase 2 — Jobs / Agents live board (+ timeline drill-in)

**Goal:** A `Jobs` widget listing Claude Code background jobs with live state, plus a drill-in showing the job's `timeline.jsonl` event feed.

### Task 2.1: Parse `state.json` (pure, TDD)

**Files:** Create `src/collect/jobs.rs`; Modify `src/collect/mod.rs`.

- [ ] **Step 1: Failing test**

```rust
#[cfg(test)]
mod tests { use super::*;
  const SAMPLE: &str = r#"{"state":"working","tempo":"active","inFlight":{"tasks":2,"queued":1},
    "intent":"do the thing","name":"Governour","cwd":"/repo",
    "worktreePath":"/repo/.claude/worktrees/x","worktreeBranch":"worktree-x",
    "createdAt":"2026-06-22T13:36:49.894Z","updatedAt":"2026-06-22T13:39:17.364Z"}"#;
  #[test] fn parses_core_fields() {
    let j = parse_state("abc123", SAMPLE).unwrap();
    assert_eq!(j.id, "abc123"); assert_eq!(j.name, "Governour");
    assert_eq!(j.state, "working"); assert_eq!(j.tasks, 2); assert_eq!(j.queued, 1);
    assert_eq!(j.worktree_branch.as_deref(), Some("worktree-x"));
    assert!(j.updated_at.unwrap() > j.created_at.unwrap());
  }
  #[test] fn tolerates_missing_optional() {
    let j = parse_state("z", r#"{"state":"idle"}"#).unwrap();
    assert_eq!(j.tasks, 0); assert!(j.worktree_path.is_none()); assert!(j.name.is_empty()||j.name=="z");
  }
}
```

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** `parse_state(id, json) -> Option<Job>` using `serde_json::Value` (defensive: `.get().and_then()`), ISO→epoch via `chrono::DateTime::parse_from_rfc3339(...).timestamp()`. ≤40 lines; extract `iso_to_epoch(&str)->Option<i64>` helper (own test).

- [ ] **Step 4: pass. Step 5: commit** — `feat(governour): parse job state.json`.

### Task 2.2: Gather jobs + sort

**Files:** Modify `src/collect/jobs.rs`.

- [ ] **Step 1: Failing test** — `rank_job` orders working+active first, then blocked, then idle/done; within a bucket by most-recent `updated_at`.

```rust
#[test] fn ranks_active_first() {
  let mut v = vec![mk("done", "idle", 100), mk("working","active",50), mk("blocked","blocked",80)];
  v.sort_by_key(rank_job);
  assert_eq!(v[0].state, "working");
}
```

- [ ] **Step 2: fail. Step 3:** implement `gather_jobs() -> Vec<Job>` (scan `claude_home()/jobs/*/state.json`, parse each, skip unreadable), `rank_job`. **Step 4: pass. Step 5: commit** — `feat(governour): gather + rank jobs`.

### Task 2.3: Parse `timeline.jsonl` (pure, TDD)

**Files:** Modify `src/collect/jobs.rs`.

- [ ] **Step 1: Failing test**

```rust
#[test] fn parses_timeline_lines() {
  let s = "{\"at\":1,\"state\":\"working\",\"detail\":\"a\",\"text\":\"x\"}\nbad line\n{\"at\":2,\"state\":\"done\",\"detail\":\"b\",\"text\":\"y\"}";
  let ev = parse_timeline(s);
  assert_eq!(ev.len(), 2);              // bad line skipped
  assert_eq!(ev[1].state, "done");
}
```

- [ ] **Step 2: fail. Step 3:** `parse_timeline(&str)->Vec<JobEvent>` (skip unparseable lines), `read_timeline(job_id, tail: usize)->Vec<JobEvent>`. **Step 4: pass. Step 5: commit** — `feat(governour): parse job timeline feed`.

### Task 2.4: Refresh + widget + drill-in

**Files:** Modify `src/refresh.rs` (fast thread: `publish_jobs`), Create `src/render/widgets/jobs.rs`, `src/render/detail/job.rs`; Modify `src/render/dashboard.rs`, `src/app.rs` (Detail::Job), `src/event.rs` (Drill on Jobs focus → push Detail::Job).

- [ ] **Step 1:** `publish_jobs`; fast thread (2s) calls `gather_jobs`.
- [ ] **Step 2:** `widgets::jobs::render`: `Table` cols `● | Name | State | Tasks | Intent | Age` — dot colour by state (active→accent, working→ok, blocked→warn, done→dim); `Age` from `updated_at` via `human_duration(now - updated_at)`; stuck = blocked & age>5m → warn row. Compact band: drop `Tasks` + truncate intent.
- [ ] **Step 3:** `detail::job::render`: header (name, state, intent, worktree branch, duration) + scrollable event list from `read_timeline(id, 200)` (newest last), each line `human_time · state · detail/text`. Scroll via `Paragraph.scroll` + `Scrollbar`.
- [ ] **Step 4:** Drill wiring; `now()` epoch passed in from the draw call (avoid `Date.now()`-style nondeterminism in tests — `now` is an arg to render helpers).
- [ ] **Step 5:** manual smoke (this job appears as "Governour/working/active"); commit — `feat(governour): jobs board + timeline drill-in`.

---

## Phase 3 — Worktree drill-in detail (activity · files+diff · commits · merge)

**Goal:** Enter/click a worktree row → full-screen detail with the four sections from discovery; the file list opens a scrollable in-app diff.

### Task 3.1: Per-worktree git detail (pure parsing + gather)

**Files:** Modify `src/collect/git.rs`.

- [ ] **Step 1: Failing tests** for pure parsers:

```rust
#[test] fn parses_numstat() {
  let s = "12\t3\tsrc/a.rs\n0\t5\tsrc/b.rs\n-\t-\tbin.png";
  let f = parse_numstat(s, /*staged*/false);
  assert_eq!(f.len(), 3);
  assert_eq!(f[0], FileChange{path:"src/a.rs".into(),added:12,deleted:3,staged:false});
  assert_eq!(f[2].added, 0); // binary "-" → 0
}
#[test] fn parses_commit_log() {
  let s = "abc123\u{1f}fix: thing\u{1f}2 hours ago\n";
  let c = parse_commit_log(s); // uses unit-separator format
  assert_eq!(c[0].short, "abc123"); assert_eq!(c[0].subject, "fix: thing");
}
```

- [ ] **Step 2: fail. Step 3:** implement `parse_numstat`, `parse_commit_log` (git `log --format=%h%x1f%s%x1f%cr`), `merge_status(path)->MergeStatus` (via `git merge-tree`/`rev-list --count main..` + `merge-base`), and `worktree_detail(path, wt)->WorktreeDetail` (committed = `diff --numstat main...HEAD`, uncommitted = `diff --numstat HEAD` + staged via `diff --numstat --cached`, commits = `log -20 main..HEAD`). **Step 4: pass. Step 5: commit** — `feat(governour): per-worktree git detail`.

### Task 3.2: Diff fetch + viewer

**Files:** Modify `src/collect/git.rs` (`file_diff(path, file, staged)->Vec<String>` via `git diff [--cached] -- <file>`), Create `src/render/detail/diff.rs`, `src/render/detail/worktree.rs`; Modify `src/app.rs` (Detail::Worktree, Detail::Diff), `src/event.rs`.

- [ ] **Step 1:** `detail::worktree::render`: 4 stacked sections (responsive: 2×2 grid when wide, stacked when narrow):
  - **Activity:** attribute the owning job by matching `Job.worktree_path == detail.path` (fall back to `worktree_branch == detail.branch`); show name/state/intent/age, or "no active job".
  - **Changed files:** `Table` grouped staged/unstaged + committed, cols `file | +adds | -dels`; selectable.
  - **Commits:** list from `commits`.
  - **Merge:** the `MergeStatus` rendered as a coloured verdict line.
- [ ] **Step 2:** `detail::diff::render`: `Paragraph` of diff lines, green `+`/red `-`/dim `@@`, `.scroll` + `Scrollbar`; Enter/click on a changed-file row pushes `Detail::Diff` (lines from `file_diff`).
- [ ] **Step 3:** `back()` pops Diff→Worktree→Dashboard; manual smoke. Commit — `feat(governour): worktree detail + in-app diff viewer`.

---

## Phase 4 — Token usage & cost (collector + pricing + widget + braille graph)

**Goal:** A `Cost` widget showing total USD, today's USD, a braille spend-trend, and per-model breakdown; pricing from vendored map + LiteLLM refresh.

### Task 4.1: Pricing (vendored + fetch)

**Files:** Create `src/collect/pricing.rs`.

- [ ] **Step 1: Failing test**

```rust
#[test] fn vendored_has_claude_models() {
  let t = vendored();
  assert!(t.0.contains_key("claude-opus-4-8"));
  assert!(t.0["claude-opus-4-8"].output > t.0["claude-opus-4-8"].input);
}
#[test] fn parses_litellm_entry() {
  let j = r#"{"claude-x":{"input_cost_per_token":1e-6,"output_cost_per_token":5e-6,
    "cache_creation_input_token_cost":1.25e-6,"cache_read_input_token_cost":1e-7}}"#;
  let t = parse_litellm(j);
  assert!((t.0["claude-x"].output - 5e-6).abs() < 1e-12);
}
```

- [ ] **Step 2: fail. Step 3:** `vendored()->PriceTable` (Opus 4.8, Sonnet 4.6, Haiku 4.5, Fable 5 — current per-token prices as constants, with a comment + dated source), `parse_litellm(&str)->PriceTable`, `fetch_litellm()->Option<PriceTable>` (ureq GET the raw LiteLLM json, 3s timeout, cache to `claude_home()/.governour-prices.json`), `load()->PriceTable` (cached→vendored merge; refresh attempted but never blocks). **Step 4: pass. Step 5: commit** — `feat(governour): pricing table (vendored + LiteLLM)`.

### Task 4.2: Usage scan with dedup (pure core, TDD)

**Files:** Create `src/collect/usage.rs`.

- [ ] **Step 1: Failing test** — the dedup-by-`message.id` gotcha is the headline risk; test it explicitly.

```rust
#[test] fn dedups_by_message_id_then_sums() {
  // same message.id twice (streaming dupes) must count once
  let lines = vec![
    asst("m1","claude-opus-4-8",100,10,0,0),
    asst("m1","claude-opus-4-8",100,10,0,0), // dupe
    asst("m2","claude-opus-4-8",200,20,0,0),
  ].join("\n");
  let recs = parse_session(&lines, "2026-06-22");
  let in_tokens: u64 = recs.iter().map(|r| r.input).sum();
  assert_eq!(recs.len(), 2);          // dupe collapsed
  assert_eq!(in_tokens, 300);
}
#[test] fn costs_with_table() {
  let recs = vec![UsageRecord{day:"d".into(),model:"claude-x".into(),input:1000,output:100,cache_write:0,cache_read:0}];
  let totals = totalize(&recs, &PriceTable(map!{"claude-x"=>ModelPrice{input:1e-6,output:5e-6,cache_write:0.0,cache_read:0.0}}));
  assert!((totals.total_cost_usd - (1000.0*1e-6 + 100.0*5e-6)).abs() < 1e-12);
}
```

- [ ] **Step 2: fail. Step 3:** implement `parse_session(jsonl, day_from_path) -> Vec<UsageRecord>` (iterate lines, take `type=="assistant"` with `message.usage`, key by `message.id` in a `HashSet`, derive `day` from each line's `timestamp` (date part), accumulate per (day,model)); `totalize(&[UsageRecord], &PriceTable) -> UsageTotals` (sum per day + per model, cost = Σ tokens×price, cache read/write/fresh). **Step 4: pass. Step 5: commit** — `feat(governour): usage parse with message-id dedup + costing`.

### Task 4.3: Incremental scan + refresh

**Files:** Modify `src/collect/usage.rs`, `src/refresh.rs`.

- [ ] **Step 1:** `scan_all(cache: &mut UsageCache) -> UsageTotals`: glob `claude_home()/projects/**/*.jsonl`; for each file compare `(mtime,len)` to cache; re-parse changed; union all `UsageRecord`s; `totalize` with `pricing::load()`. `UsageCache` = `HashMap<PathBuf,(SystemTime,u64,Vec<UsageRecord>)>`.
- [ ] **Step 2:** cost thread (10s) owns the cache, calls `scan_all`, `publish_usage`. Manual smoke (totals look sane vs a quick `ccusage`-style sanity check). Commit — `feat(governour): incremental usage scan + publish`.

### Task 4.4: Cost widget + braille trend

**Files:** Create `src/graph.rs`, `src/render/widgets/cost.rs`; Modify `src/render/dashboard.rs`.

- [ ] **Step 1: Failing test** for `graph::spark_points(&[f64], width) -> Vec<(f64,f64)>` (maps a series to chart points, clamps to width). **Step 2: fail. Step 3:** implement.
- [ ] **Step 4:** `widgets::cost::render`: top line `Total $X · Today $Y · cache-hit Z%`; a braille `Chart` of `by_day` cost (last N days, `Marker::Braille`, accent line); below, a small per-model `Table` `Model | $ | out-tok`. Compact band: numbers only, no chart. Expand (`e`): full per-day table + per-model + per-branch. Commit — `feat(governour): cost widget with braille spend trend`.

---

## Phase 5 — Cache efficiency + activity timeline + Code polish

**Goal:** An `Activity` widget: cache read/write/fresh ratio bar + a prompt-cadence sparkline from `history.jsonl`; tidy the Code widget.

### Task 5.1: history.jsonl cadence (pure)

**Files:** Create `src/collect/activity.rs` (or extend `usage.rs`).

- [ ] **Step 1: Failing test** — `bucket_by_day(timestamps_ms: &[i64], days: usize) -> Vec<(String,u32)>` returns per-day prompt counts for the last `days`.
- [ ] **Step 2: fail. Step 3:** implement parse of `claude_home()/history.jsonl` (`{timestamp}` ms) → counts; cache efficiency reuses `UsageTotals` (read/write/fresh). **Step 4: pass. Step 5: commit** — `feat(governour): activity cadence + cache efficiency data`.

### Task 5.2: Activity widget

**Files:** Create `src/render/widgets/activity.rs`.

- [ ] **Step 1:** render a `Gauge`/segmented bar for cache read% vs write% vs fresh%, plus a `Sparkline` of daily prompt counts; numbers labelled. Commit — `feat(governour): activity + cache-efficiency widget`.

---

## Phase 6 — Docker containers (+ logs drill-in)

**Goal:** A `Docker` widget listing containers with health + CPU/mem braille; drill-in shows recent logs.

### Task 6.1: Parse `docker ps`/`stats` (pure, TDD)

**Files:** Create `src/collect/docker.rs`.

- [ ] **Step 1: Failing test**

```rust
#[test] fn parses_ps_json_line() {
  let l = r#"{"ID":"abc","Names":"sovra-backend","State":"running","Status":"Up 2 hours (healthy)","Ports":"0.0.0.0:8080->8080/tcp"}"#;
  let c = parse_ps_line(l).unwrap();
  assert_eq!(c.name, "sovra-backend"); assert_eq!(c.health.as_deref(), Some("healthy"));
  assert!(c.ports.iter().any(|p| p.contains("8080")));
}
#[test] fn parses_stats_json_line() {
  let l = r#"{"Container":"abc","CPUPerc":"12.5%","MemUsage":"256MiB / 2GiB"}"#;
  let (id,cpu,used,limit) = parse_stats_line(l).unwrap();
  assert_eq!(id,"abc"); assert!((cpu-12.5).abs()<0.01); assert!(used>0 && limit>used);
}
```

- [ ] **Step 2: fail. Step 3:** `parse_ps_line`, `parse_stats_line` (parse `%`, `MiB/GiB`→bytes, extract `(healthy)` from Status), `gather_containers()` (shell `docker ps --all --no-trunc --format '{{json .}}'` + `docker stats --no-stream --format '{{json .}}'`, join by id; empty if `docker` missing — never panic). **Step 4: pass. Step 5: commit** — `feat(governour): docker ps/stats parsing`.

### Task 6.2: Docker widget + logs drill-in

**Files:** Modify `src/refresh.rs` (slow thread: `publish_containers`), Create `src/render/widgets/docker.rs`, `src/render/detail/container.rs`; Modify `dashboard.rs`, `app.rs`, `event.rs`.

- [ ] **Step 1:** widget `Table` `● | Name | Health | CPU% | Mem | Ports | Up`; dot by state/health (running+healthy→ok, unhealthy→err, exited→dim). Maintain `cpu_history` ring buffer per container for an inline braille cell/expand chart.
- [ ] **Step 2:** drill-in: `docker logs --tail 200 <id>` into a scrollable `Paragraph`. "Docker not running" empty-state when no daemon. Commit — `feat(governour): docker widget + logs drill-in`.

---

## Phase 7 — Dev servers / ports

**Goal:** A `Ports` widget showing :3000/:8080/:3001/Supabase health, latency, owning PID.

### Task 7.1: Endpoint check (pure split + IO)

**Files:** Create `src/collect/ports.rs`.

- [ ] **Step 1: Failing test** — `default_endpoints()` includes frontend:3000, backend:8080, wa-bridge:3001; `parse_lsof_pid("p12345\n...")` extracts 12345.
- [ ] **Step 2: fail. Step 3:** `check(&Endpoint)->Endpoint` (TcpStream `connect_timeout` 500ms → up + latency; PID via `lsof -ti :<port>` shell-out, best-effort), `gather_endpoints()`. **Step 4: pass. Step 5: commit** — `feat(governour): dev endpoint health checks`.

### Task 7.2: Ports widget

**Files:** Modify `src/refresh.rs` (fast: `publish_endpoints`), Create `src/render/widgets/ports.rs`.

- [ ] **Step 1:** `Table` `● | Service | Addr | Latency | PID`; up→ok, down→err. Commit — `feat(governour): ports widget`.

---

## Phase 8 — Process resources (sysinfo)

**Goal:** A `Procs` widget: CPU/mem/uptime of claude/node/python processes, btm-style, with a per-process CPU braille history on expand.

### Task 8.1: Process snapshot (filter + sort)

**Files:** Create `src/collect/procs.rs`.

- [ ] **Step 1: Failing test** — `is_dev_proc("claude")`, `is_dev_proc("node")`, `is_dev_proc("python3.12")` true; `is_dev_proc("Finder")` false; `rank_proc` sorts by CPU desc.
- [ ] **Step 2: fail. Step 3:** `gather_procs(&mut sysinfo::System)->Vec<Proc>` (refresh processes, filter `is_dev_proc`, map to `Proc`, sort), holding `System` in the fast refresh thread (sysinfo needs two samples for CPU%). **Step 4: pass. Step 5: commit** — `feat(governour): process snapshot via sysinfo`.

### Task 8.2: Procs widget + CPU history

**Files:** Modify `src/refresh.rs` (fast: `publish_procs` + push CPU into `cpu_history`), Create `src/render/widgets/procs.rs`.

- [ ] **Step 1:** `Table` `PID | Name | CPU% | Mem | Up` + sortable headers (click header → toggle sort key/dir; store in `WidgetUiState`). Expand: braille CPU chart of the selected pid from `cpu_history`. Commit — `feat(governour): process widget + CPU history`.

---

## Phase 9 — Repo health, sortable headers everywhere, polish

**Goal:** `Repo` widget; finish cross-widget niceties (clickable sort headers, footer help, theme switch); docs.

### Task 9.1: Repo health (pure + gather)

**Files:** Modify `src/collect/git.rs`.

- [ ] **Step 1: Failing test** — `parse_ahead_behind("3\t1")` → (ahead 3, behind 1) from `git rev-list --left-right --count origin/main...HEAD`. **Step 2: fail. Step 3:** `repo_health(root)->RepoHealth` (branch, ahead/behind vs origin, `stash list` count, dirty count, last-fetch age from `.git/FETCH_HEAD` mtime). **Step 4: pass. Step 5: commit** — `feat(governour): repo health`.

### Task 9.2: Repo widget + footer + help overlay

**Files:** Create `src/render/widgets/repo.rs`; Modify `dashboard.rs`.

- [ ] **Step 1:** key-value panel (branch, ↑ahead/↓behind, stash, dirty, last fetch). Footer shows context keys per focus + `?` opens a help overlay (Paragraph modal) listing all bindings. Commit — `feat(governour): repo widget + help overlay`.

### Task 9.3: Docs + final checks

**Files:** Create/Modify `.claude/commands/scripts/governour/README.md`; Modify `docs/` governour reference if present; verify `Justfile` recipes.

- [ ] **Step 1:** Document the cockpit (widgets, keys, data sources, refresh cadence, "read-only v1", price-table note). Update `governour-check` slash-command doc if it describes the old TUI.
- [ ] **Step 2:** `cargo test` (all green), `cargo clippy -- -D warnings`, `cargo fmt --check`. Manual: 80×24, 130×40, 220×60, and a too-small window.
- [ ] **Step 3:** Commit — `docs(governour): cockpit usage + data sources`.

---

## Self-Review

**1. Spec coverage** (recap → task):
- Worktrees + drill-in (activity/files+diff/commits/merge) → Phase 1 (widget) + Phase 3. ✓
- Jobs/Agents live board + timeline → Phase 2. ✓
- Token usage & cost (deduped, braille, per-model/branch) → Phase 4. ✓
- Cache efficiency + activity timeline + Code/LOC → Phase 5 (+ Code in Phase 1). ✓
- Docker containers + logs → Phase 6. ✓
- Dev servers/ports → Phase 7. ✓
- Claude & node process resources → Phase 8. ✓
- Repo/git health → Phase 9. ✓
- btm-style responsive dashboard, size bands, too-small guard, `e` expand, Enter/click drill, mouse hit-testing, braille graphs, theme → Phases 1, 4, 8/9. ✓
- Pricing vendored + LiteLLM refresh → Phase 4.1. ✓
- Read-only v1, 10s/2s cadence → refresh design + defaults. ✓
- Legacy one-shot renders preserved → Phase 1.10. ✓

**2. Placeholder scan:** no "TBD"/"handle edge cases"/"write tests for the above"; every collector/parser has concrete code + a concrete test; render tasks carry exact columns/colours/constraints. The "others render a dim placeholder block" in Task 1.8 is an intentional, working stub (each later phase replaces one), not an unfinished spec.

**3. Type consistency:** `WidgetKind`, `Job`, `Worktree`, `WorktreeDetail`, `UsageRecord`/`UsageTotals`, `PriceTable`/`ModelPrice`, `Container`, `Endpoint`, `Proc`, `View`/`Detail`, `FrameRects`, `Theme`, `Action` are defined once (Architecture / their first task) and referenced consistently. `publish_*` names match between `refresh.rs` definition and per-phase use. `parse_state`/`parse_timeline`/`parse_session`/`totalize`/`parse_ps_line`/`parse_stats_line`/`parse_numstat`/`parse_commit_log` names are stable across their tasks.

**Risks / watch-items for the implementer:**
- **Usage dedup** is the #1 correctness risk — Task 4.2 tests it directly; do not skip.
- **sysinfo CPU%** needs two refreshes spaced in time — keep `System` resident in the refresh thread, don't recreate per tick.
- **Mouse row mapping** must add `TableState.offset()` and subtract border+header — store `table_inner`/`offset` in `FrameRects` every draw.
- **Never block the UI thread on IO** — all collectors run in refresh threads; `docker`/`lsof` absence must yield empty data, never a panic.
- **Time in tests** — pass `now: i64` into age/duration render helpers so tests are deterministic (no wall-clock calls in pure code).
