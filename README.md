<p align="center">
  <img src="https://raw.githubusercontent.com/shaunbarney/claude-cockpit/main/assets/logo.png" alt="claude-cockpit" width="320">
</p>

<h1 align="center">claude-cockpit</h1>

<p align="center">
  A <a href="https://github.com/ClementTsang/bottom"><code>btm</code></a>-style terminal dashboard for <strong>Claude Code</strong> and your dev environment —
  git worktrees, background agents, token/USD cost, Docker, and dev processes in one responsive TUI.
</p>

<p align="center">
  <a href="https://github.com/shaunbarney/claude-cockpit/actions/workflows/ci.yml"><img src="https://github.com/shaunbarney/claude-cockpit/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/claude-cockpit"><img src="https://img.shields.io/crates/v/claude-cockpit.svg" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

---

`claude-cockpit` is a single, fast Rust TUI that answers "what's going on?" across everything you run while developing with Claude Code. It reads only local files (`~/.claude/…`) and standard CLIs (`git`, `docker`, `lsof`) — no network required for the core, no daemon, no telemetry.

The layout reflows to your terminal: a multi-column grid on a wide screen, a single column on a narrow one, and a "resize me" guard when there's truly no room. Drive it with the keyboard or the mouse.

## Widgets

| Widget | What it shows |
|--------|---------------|
| **Worktrees** | Every git worktree ranked ahead → dirty → clean, with committed/uncommitted churn and age. Drill in for the owning Claude job, changed files, an in-app diff, recent commits, and a merge-readiness verdict. |
| **Jobs** | Live Claude Code background agents from `~/.claude/jobs/*/state.json` — state, tempo, in-flight tasks, intent, age, stuck-detection. Drill in for the job's `timeline.jsonl` event feed. |
| **Cost** | Per-day / per-session / per-model token totals and **USD cost** from your transcripts (deduped by `message.id`), with a braille spend-trend and cache-hit %. |
| **Activity** | Cache-efficiency gauge (read vs write vs fresh) and a prompt-cadence sparkline from `~/.claude/history.jsonl`. |
| **Code** | Lines of code by language (via `tokei`). |
| **Docker** | Containers with health, CPU/mem, ports. Drill in for recent logs. |
| **Ports** | Dev-endpoint health (up/down, latency, owning PID) — configured or auto-discovered. |
| **Processes** | CPU / memory / uptime of `claude`, `node`, `python`, and other dev processes. |
| **Repo** | Whole-repo git health: branch, ahead/behind `origin/main`, stash count, dirty count, last fetch. |

## Install

With a recent Rust toolchain (`rustup`):

```bash
# from crates.io (after the first published release)
cargo install claude-cockpit

# straight from git (works today)
cargo install --git https://github.com/shaunbarney/claude-cockpit

# or from a local clone
git clone https://github.com/shaunbarney/claude-cockpit
cd claude-cockpit
cargo install --path .
```

Then, from inside any git repository:

```bash
claude-cockpit
```

## Usage

```
claude-cockpit            # launch the interactive cockpit (default)
claude-cockpit --worktrees   # one-shot: print the worktrees table and exit
claude-cockpit --code        # one-shot: worktrees + lines-of-code
claude-cockpit --watch       # one-shot render, refreshed in place
claude-cockpit --help
```

### Keys

| Key | Action |
|-----|--------|
| `Tab` / `←` `→` / `h` `l` | Move focus between widgets |
| `↑` `↓` / `j` `k` | Select a row, or scroll inside a detail/diff view |
| `Enter` / click | Drill into the focused row (worktree · job · container) |
| `e` | Expand the focused widget to full screen |
| `r` | Refresh now |
| `?` | Toggle the help overlay |
| `Esc` | Back out of a detail view / close help |
| `q` / `Ctrl-C` | Quit |

The mouse works too — click a widget to focus it, click a row to select it, and scroll to move the selection or a diff.

## Configuration

`claude-cockpit` works with zero config. To pin the dev endpoints it watches (instead of auto-discovering listeners) or to monitor extra processes, drop a `claude-cockpit.toml` in the repo root, or `~/.config/claude-cockpit/config.toml`:

```toml
# Dev endpoints to health-check in the Ports widget.
[[endpoints]]
label = "frontend"
port  = 3000

[[endpoints]]
label = "backend"
host  = "127.0.0.1"   # optional, defaults to 127.0.0.1
port  = 8080

# Extra process-name substrings to include in the Processes widget
# (on top of the built-in claude/node/python/cargo/… set).
processes = ["uvicorn", "myservice"]
```

See [`claude-cockpit.toml.example`](claude-cockpit.toml.example).

## Data sources

Everything is read locally and best-effort — a missing tool or file just yields an empty widget, never a crash:

- **Jobs / activity** — `~/.claude/jobs/*/state.json`, `~/.claude/jobs/*/timeline.jsonl`, `~/.claude/history.jsonl`
- **Cost** — `~/.claude/projects/**/*.jsonl` (incrementally scanned, cached by mtime). Prices are a vendored Claude table, refreshed from [LiteLLM](https://github.com/BerriAI/litellm) when online and cached to `~/.claude/.cockpit-prices.json`.
- **Git** — `git` CLI in the current repo
- **Docker** — `docker ps` / `docker stats` / `docker logs`
- **Ports / processes** — `lsof` + TCP connect; `sysinfo`

> **v1 is read-only.** The cockpit inspects; it never kills a job, stops a container, or touches your branches.

## Building from source

```bash
cargo build --release    # binary at target/release/claude-cockpit
cargo test               # unit + headless render tests
cargo clippy
```

## Acknowledgements

Inspired by [`bottom`](https://github.com/ClementTsang/bottom) (`btm`) and built on [`ratatui`](https://ratatui.rs). LOC counting by [`tokei`](https://github.com/XAMPPRocky/tokei); pricing data from [LiteLLM](https://github.com/BerriAI/litellm).

## License

MIT — see [LICENSE](LICENSE).
