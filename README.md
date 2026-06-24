<p align="center">
  <img src="https://raw.githubusercontent.com/shaunbarney/claude-cockpit/main/assets/logo.png" alt="claude-cockpit" width="320">
</p>

<h1 align="center">claude-cockpit</h1>

<p align="center">
  A <a href="https://github.com/ClementTsang/bottom"><code>btm</code></a>-style terminal dashboard for <strong>Claude Code</strong> and your dev environment —
  git worktrees, background agents, token/USD cost, rate-limit proximity, Docker, dev endpoints, code stats, and tool usage in one responsive TUI.
</p>

<p align="center">
  <a href="https://github.com/shaunbarney/claude-cockpit/actions/workflows/ci.yml"><img src="https://github.com/shaunbarney/claude-cockpit/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/claude-cockpit"><img src="https://img.shields.io/crates/v/claude-cockpit.svg" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

---

`claude-cockpit` is a single, fast Rust TUI that answers "what's going on?" across everything you run while developing with Claude Code. It reads only local files (`~/.claude/…` and files in your repo) and standard CLIs (`git`, `docker`, `lsof`) — no network required for the core, no daemon, no telemetry.

The layout reflows to your terminal: a multi-column grid on a wide screen, a single column on a narrow one, and a "resize me" guard when there's truly no room. Drive it with the keyboard or the mouse.

## Widgets

| Widget | What it shows |
|--------|---------------|
| **Worktrees** | Every git worktree ranked ahead → dirty → clean, with committed/uncommitted churn and age. Drill in for the owning Claude job, changed files, an in-app diff, recent commits, and a merge-readiness verdict. |
| **Jobs** | Live Claude Code background agents from `~/.claude/jobs/*/state.json` — state, tempo, in-flight tasks, intent, age, stuck-detection. Drill in for the job's `timeline.jsonl` event feed. |
| **Cost** | Total / today / per-model token totals and **USD cost** from your transcripts (deduped by `message.id`), with a braille spend-trend and cache-hit %. Drill in for a per-day and per-model breakdown. |
| **Rate** | Rate-limit proximity: a rolling **5-hour prompt** gauge (the unit the Claude Code subscription limit actually uses) vs your plan cap, an **OTPM** output-tokens-per-minute burn gauge, a reset countdown, and a token-rate sparkline. Set `plan` in config for a true %; otherwise it auto-scales to your busiest observed window. |
| **Code** | Total lines by language (code + comments + blanks, à la `wc -l`; via `tokei`), counted over **git-tracked files only** (`git ls-files`) — so gitignored paths, build output, and sibling worktrees don't inflate it. Per-language icons, colours, and a size bar. |
| **Docker** | Containers with health, CPU/mem, ports. Drill in for recent logs. |
| **Ports** | Dev-endpoint health (up/down, latency, owning PID). Auto-discovers ports from your `docker-compose` / `Dockerfile` files, plus anything in config; falls back to live TCP listeners. |
| **Tools** | Most-used tools across your recent sessions (Bash, Edit, Read, Grep, MCP tools…) over the last 7 days, with usage counts and bars — parsed from `tool_use` blocks in the transcripts. |
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
claude-cockpit                 # launch the interactive cockpit (default)
claude-cockpit --tui           # force the interactive cockpit explicitly
claude-cockpit --worktrees     # one-shot: print the worktrees table and exit
claude-cockpit --code          # one-shot: worktrees + repo lines, stacked
claude-cockpit --watch         # one-shot render, refreshed in place
claude-cockpit --watch --interval 5   # …refreshing every 5s (default 10)
claude-cockpit --help
```

### Keys

| Key | Action |
|-----|--------|
| `Tab` / `←` `→` / `h` `l` | Move focus between widgets |
| `↑` `↓` / `j` `k` | Select a row, or scroll inside a detail/diff view |
| `Enter` / click | Drill into the focused row/widget (worktree · job · container · cost · ports · repo) |
| `e` | Expand the focused widget to full screen |
| `r` | Refresh now |
| `?` | Toggle the help overlay |
| `Esc` | Back out of a detail view / close help |
| `q` / `Ctrl-C` | Quit |

The mouse works too — click a widget to focus it, click a row to select it, and scroll to move the selection or a diff.

## Configuration

`claude-cockpit` works with zero config. The **Ports** widget populates itself in this order:

1. Any explicit `[[endpoints]]` from your config.
2. **Auto-discovered docker ports** — it scans the repo (bounded depth, skipping `node_modules`/`target`/`vendor`/…) for `docker-compose.yml` / `compose.yaml` (published host ports, short *and* long syntax) and `Dockerfile`s (`EXPOSE`), labelling each by its service or directory.
3. If both are empty, live local TCP listeners via `lsof`.

To pin endpoints, set rate-limit caps, or turn docker discovery off, drop a `claude-cockpit.toml` in the repo root, or `~/.config/claude-cockpit/config.toml`:

```toml
# Auto-discover endpoints from docker-compose / Dockerfile port mappings
# in the repo, in addition to any [[endpoints]] below. Default: true.
discover_endpoints = true

# Dev endpoints to health-check in the Ports widget.
[[endpoints]]
label = "frontend"
port  = 3000

[[endpoints]]
label = "backend"
host  = "127.0.0.1"   # optional, defaults to 127.0.0.1
port  = 8080

# Rate widget caps. The Claude Code subscription limit is measured in PROMPTS
# per rolling 5h, so set your `plan` for a true gauge (the plan can't be
# detected locally — run `/status` in Claude Code if unsure). Anything omitted
# auto-scales to your own busiest observed window.
[rate_limit]
plan           = "max5x"   # pro | max5x | max20x  → 5h prompt cap (~20/100/400)
# prompts_5h   = 100       # explicit prompt cap (overrides `plan`)
output_per_min = 100000    # output tokens/min (OTPM) — the tightest API limit
```

See [`claude-cockpit.toml.example`](claude-cockpit.toml.example).

## Data sources

Everything is read locally and best-effort — a missing tool or file just yields an empty widget, never a crash:

- **Jobs** — `~/.claude/jobs/*/state.json`, `~/.claude/jobs/*/timeline.jsonl`
- **Cost** — `~/.claude/projects/**/*.jsonl` (incrementally scanned, cached by mtime). Prices are a vendored Claude table, refreshed from [LiteLLM](https://github.com/BerriAI/litellm) when online and cached to `~/.claude/.cockpit-prices.json`.
- **Rate** — output-token usage from those same transcripts (OTPM) + prompt timestamps from `~/.claude/history.jsonl` (the rolling 5-hour prompt window)
- **Code** — [`tokei`](https://github.com/XAMPPRocky/tokei) over `git ls-files` (git-tracked files only)
- **Git / Repo / Worktrees** — `git` CLI in the current repo
- **Docker** — `docker ps` / `docker stats` / `docker logs`
- **Ports** — `docker-compose`/`Dockerfile` port mappings in the repo, then `lsof` + TCP connect
- **Tools** — `tool_use` blocks in `~/.claude/projects/**/*.jsonl` (deduped by `message.id`)

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
