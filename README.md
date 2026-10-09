<p align="center">
  <img src="https://raw.githubusercontent.com/shaunbarney/claude-cockpit/main/assets/logo.png" alt="claude-cockpit" width="320">
</p>

<h1 align="center">claude-cockpit</h1>

<p align="center">
  A live, read-only terminal dashboard for <strong>Claude Code</strong> and your dev environment —
  git worktrees, background agents, token/USD cost, rate-limit proximity, Docker, dev endpoints, code stats, and tool &amp; skill usage in one responsive TUI.
</p>

<p align="center">
  <a href="https://github.com/shaunbarney/claude-cockpit/actions/workflows/ci.yml"><img src="https://github.com/shaunbarney/claude-cockpit/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/claude-cockpit"><img src="https://img.shields.io/crates/v/claude-cockpit?logo=rust&color=2e5ee8" alt="crates.io"></a>
  <a href="https://docs.rs/claude-cockpit"><img src="https://img.shields.io/docsrs/claude-cockpit?logo=docsdotrs" alt="docs.rs"></a>
  <a href="LICENSE"><img src="https://img.shields.io/crates/l/claude-cockpit?color=blue" alt="License: MIT"></a>
</p>

---

`claude-cockpit` is a single, fast Rust TUI that answers "what's going on?" across everything you run while developing with Claude Code. It reads only local files (`~/.claude/…` and files in your repo) and standard CLIs (`git`, `docker`, `lsof`/`ss`) — no network required for the core, no daemon, no telemetry.

The layout reflows to your terminal: a multi-column grid on a wide screen, a single column on a narrow one, and a "resize me" guard when there's truly no room. Grid rows are sized by how much each widget has to show, so an empty box doesn't claim the same space as a busy table — but widgets never move, so "Docker is bottom-right" stays true.

Drive it with the keyboard or the mouse: `/` filters any table as you type, `:` opens a command palette over everything the cockpit can do, and `s` sorts. Rows that change between refreshes flash and fade, so a job going blocked is something you see happen rather than notice later.

## Widgets

| Widget | What it shows |
|--------|---------------|
| **Worktrees** | Every git worktree ranked ahead → dirty → clean, with committed/uncommitted churn and age. Drill in for the owning Claude job, changed files, an in-app diff, recent commits, and a merge-readiness verdict. |
| **Jobs** | Live Claude Code background agents from `~/.claude/jobs/*/state.json` that belong to **this project** (their worktree, working directory or launch directory is inside the repo) — state, tempo, in-flight tasks, intent, age. Blocked jobs read *needs input*; a working job that has gone quiet reads *stalled*. Worktrees with no owning job show as `no agent` rows, so every worktree is accounted for. Drill in for the job's `timeline.jsonl` event feed. |
| **Cost** | Total / today / per-model token totals and **USD cost** from your transcripts (deduped by `message.id`), with a braille spend-trend and cache-hit %. Drill in for a per-day and per-model breakdown. |
| **Rate** | Rate-limit proximity: a rolling **5-hour prompt** gauge (the unit the Claude Code subscription limit actually uses) vs your plan cap, an **OTPM** output-tokens-per-minute burn gauge, a reset countdown, and a token-rate sparkline. Set `plan` in config for a true %; otherwise it auto-scales to your busiest observed window. |
| **Code** | Total lines by language (code + comments + blanks, à la `wc -l`; via `tokei`), counted over **git-tracked files only** (`git ls-files`) — so gitignored paths, build output, and sibling worktrees don't inflate it. Per-language icons, colours, and a size bar. |
| **Docker** | **This repo's** containers (Compose `working_dir` inside the repo or one of its worktrees, or a matching Compose project name) with health, CPU/mem, ports. Exited containers are hidden unless nothing is running. If the repo has no containers it shows every container on the host, with a dim "showing all" note. Drill in for recent logs (stdout + stderr interleaved, opened at the newest line). |
| **Ports** | Dev-endpoint health (up/down, latency, owner). Shows your configured endpoints plus the ports this repo's **running** containers actually publish; see [Configuration](#configuration) for the full order. The **Owner** column is the container that publishes the port, otherwise the listening PID. |
| **Tools** | Most-used tools across your recent sessions (Bash, Edit, Read, Grep, MCP tools…) over the last 7 days, with usage counts and bars — parsed from `tool_use` blocks in the transcripts. |
| **Skills** | Your Claude Code skills — personal (`~/.claude/skills`), project, and plugin — ranked by how often you've actually invoked them, with source tag and a usage bar. Drill into a skill for its source, path, usage, and full description. |

## Install

With Rust 1.88 or newer (`rustup`):

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

Then run it from inside any git repository — or register the repos you work on once and switch between them (see [Projects](#projects)):

```bash
claude-cockpit                      # the repo you're in
claude-cockpit project add ~/code/api --pin
claude-cockpit -p api               # open a registered project from anywhere
```

## Usage

```
claude-cockpit                 # launch the interactive cockpit (default)
claude-cockpit --tui           # force the interactive cockpit explicitly
claude-cockpit --worktrees     # one-shot: print the worktrees table and exit
claude-cockpit --code          # one-shot: worktrees + repo lines, stacked
claude-cockpit --watch         # worktrees + repo lines side by side, refreshed in place
claude-cockpit --watch --code  # …or watch any one-shot layout (--worktrees / --code)
claude-cockpit --watch --interval 5   # …refreshing every 5s (default 10, minimum 0.5)
claude-cockpit -p <name|path>  # open a registered project (works with the one-shot flags too)
claude-cockpit project …       # manage projects — see Projects below
claude-cockpit --help
```

`--tui` can't be combined with the one-shot flags. The interactive cockpit needs a real terminal; in a pipe or script it exits with an error pointing at the one-shot views. The one-shot views need a git repository and exit 1 outside one. In watch mode `git fetch` runs at most once a minute, however short the interval.

### Keys

**Move**

| Key | Action |
|-----|--------|
| `Tab` / `⇧Tab` | Cycle focus between widgets |
| `←` `→` / `h` `l` / `↑` `↓` | Move focus to the widget in that direction |
| `j` / `k` | Select a row, or scroll inside a detail/diff view |
| `g` / `G` | Jump to the first / last row |
| `PgUp` / `PgDn`, `Ctrl-D` / `Ctrl-U` | Page through rows |

**Act**

| Key | Action |
|-----|--------|
| `Enter` / click | Drill into the focused row (worktree · job · container · cost · ports · skill) |
| `e` | Expand the focused widget to full screen |
| `r` | Refresh now |
| `P` | Project picker — switch, add and manage projects |
| `1`–`9` | Switch to pinned project 1–9 |
| `a` | On Jobs or Docker: toggle between this project and everything on the machine |
| `Esc` | Clear the filter, back out of a detail view, or close an overlay |
| `q` / `Ctrl-C` | Quit |

**Find**

| Key | Action |
|-----|--------|
| `/` | Filter the focused widget's rows as you type. `Enter` keeps the filter, `Esc` drops it |
| `:` or `Ctrl-K` | Command palette — fuzzy-search every action, including jumping to any widget |
| `s` / `S` | Cycle the sort column / reverse it |
| `?` | Toggle the key help |

If a collector fails or your config has problems (a parse error, an unknown key), the header shows **⚠ N** — run *Show problems* from the palette for details.

Filtering is fuzzy and searches more than the name column: a job matches on its state and intent, a container on its health and published ports. The footer shows the live match count, and the filter and sort in force.

The mouse works too — click a widget to focus it, click a row to select it, and scroll to move the selection or a diff.

## Projects

The cockpit shows one project — a git repository and all of its worktrees — at a time. Register the repos you care about and switch between them without leaving the TUI.

**In the TUI**, press `P` for the project picker:

```
╭ Projects · 2 registered · 3 suggested ──────────────────────────────────────────────────╮
│    Project               Branch        Dirty  Trees  Jobs    Active  Last $   Path      │
│▶1  ★ sovra               main          1      1      ◉1 ◐2   11s     $0.30    ~/work/…  │
│    ● cockpit             main          1      6      —       2m      —        ~/claude… │
│    + cognirouteslm       main          clean  —      —       6d      $0.00    ~/work/…  │
╰ Enter open · 1-9 pinned · a add · n add path · d remove · p pin · r rename · t tags ────╯
```

| Key | In the picker |
|-----|---------------|
| `Enter` | Open the selected project |
| `1`–`9` | Open a pinned project (also works straight from the dashboard) |
| `a` | Add the selected suggestion as a project |
| `n` | Add a repository by path (`Tab` completes directories; any path inside a repo works) |
| `d` | Unregister a project (asks first; the repository itself is never touched), or hide a suggestion |
| `p` / `r` / `t` | Pin, rename, edit tags (`+work -old`) |
| `/` | Filter by name, path or tag |

Each row shows the branch, dirty files, worktrees, running (◉) and blocked (◐) agents, the last Claude Code activity anywhere in the repo, and the cost of the last session started there. **Suggestions** (`+`, dimmed) are repos Claude Code has already worked in — read from `~/.claude.json` and the `cwd` recorded in your transcripts, with worktree sessions folded into their repo — so the list fills itself; nothing is added until you press `a`. The command palette (`:`) also offers *Switch to project …* for every registered project.

**From the shell:**

```bash
claude-cockpit project add [PATH] [--name N] [--tag T]... [--pin]   # default: the repo you're in
claude-cockpit project ls [--all] [--tag T] [--json]                # --all adds suggestions
claude-cockpit project rm <name|path>
claude-cockpit project rename <name> <new-name>
claude-cockpit project pin|unpin <name>
claude-cockpit project tag <name> +work -old
claude-cockpit project discover [--add-all]                         # list (or register) suggestions
claude-cockpit project hide <path>                                  # stop suggesting a repo
claude-cockpit project prune                                        # drop projects whose folder is gone
```

**Which project opens:** `-p <name|path>` if given; else the repo you launched from (registered or not); else the project you had open last; else your first pinned project; else the picker.

**Where it's stored:** your list is `$XDG_CONFIG_HOME/claude-cockpit/projects.toml` (default `~/.config/…`) — plain TOML with `~`-relative paths, safe to edit by hand or keep in your dotfiles. If it fails to parse, the cockpit refuses to write it rather than overwrite your list. What the cockpit remembers on its own (last project, recents, hidden suggestions) lives separately in `$XDG_STATE_HOME/claude-cockpit/state.toml` (default `~/.local/state/…`), so switching projects never rewrites the file you edit.

```toml
[discovery]
claude_history = true                       # suggest repos from Claude Code history
roots = [{ path = "~/code", depth = 2 }]    # also suggest git repos found under these folders

[[project]]
name = "sovra"
path = "~/work/SovraSRXProduction"
tags = ["work"]
pinned = true
```

## Configuration

`claude-cockpit` works with zero config.

### Where the config lives

The first of these files that exists is used, **as a whole**. Files are not merged, so a repo config completely replaces your personal one:

1. `<repo root>/claude-cockpit.toml`, found from the git repository you run in, not the current directory
2. `$XDG_CONFIG_HOME/claude-cockpit/config.toml`, which defaults to `~/.config/claude-cockpit/config.toml`

A config that fails to parse is ignored (defaults apply) and the error is reported. Unknown keys are ignored with a warning, which catches typos and keys put under the wrong `[table]`. One-shot runs print these messages to stderr.

### Ports: where endpoints come from

1. Any explicit `[[endpoints]]`, in the order you wrote them.
2. With `discover_endpoints = true` (the default): the host ports published by this repo's **running** containers, labelled by Compose service. Only containers whose Compose `working_dir` is inside the repo (or one of its worktrees) count, so another project's container on the same port won't show up as yours.
3. If none of the repo's containers are running: ports declared in the repo's **default** Compose files, found with `git ls-files`, so gitignored and vendored directories are skipped. These are `compose.y[a]ml`, `docker-compose.y[a]ml` and their `.override` files, plus anything they `include:`. `${VAR}` / `${VAR:-default}` resolve from your environment, then the `.env` (or the include's `env_file`) next to the file. Variant files such as `docker-compose.prod.yml` and Dockerfile `EXPOSE` are never used, because they don't describe what's bound on your machine.
4. If all of that is empty: every listening TCP port reachable on localhost.

Duplicates are dropped, treating `localhost`, `127.0.0.1` and `::1` as the same host. Ownership comes from one `lsof` snapshot per refresh, plus `ss -ltnp` on Linux. A port published by Docker is attributed to its **container**, because the process that holds it (`docker-proxy`) is root-owned and usually invisible. Your own processes show their PID. A PID shows as `—` when it belongs to another user and you're not root, or when nothing is listening.

### Example

```toml
# Top-level keys must come BEFORE the first [table] header. TOML puts any key
# written after `[theme]` inside the theme table.

# Auto-discover endpoints from this repo's running containers (and, failing
# that, its default compose files), in addition to any [[endpoints]] below.
# Default: true.
discover_endpoints = true

# Colour. Omit the whole section to auto-detect: NO_COLOR in the environment
# forces monochrome, COLORTERM=truecolor enables the 24-bit palette, and
# anything else uses the 16 ANSI colours (so it follows your terminal theme).
[theme]
# mode   = "true"      # mono | ansi | true
# accent = "#7C5CFF"   # only applies on the truecolor tier

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
- **Cost** — `~/.claude/projects/**/*.jsonl`, read incrementally (only bytes appended since the last scan). Streaming writes one line per content block with the same `message.id`, so usage is deduped by id **across all files**, keeping the final (largest) figures; 1-hour cache writes are priced at 2× input, 5-minute writes at 1.25×. Prices are a vendored Claude table, refreshed from [LiteLLM](https://github.com/BerriAI/litellm) when online and cached to `~/.claude/.cockpit-prices.json`.
- **Rate** — output-token usage from those same transcripts (OTPM) + your prompts in them (human turns in main sessions; tool results, meta lines and local-only slash commands like `/model` don't count) for the rolling 5-hour window
- **Code** — [`tokei`](https://github.com/XAMPPRocky/tokei) over `git ls-files` (git-tracked files only)
- **Git / Worktrees** — `git` CLI in the active project; ahead/behind and merge checks compare against `origin/<default branch>` (detected from `origin/HEAD`)
- **Docker** — `docker ps` (with Compose labels) / `docker stats` / `docker logs`, each with a timeout so a hung daemon can't stall the cockpit
- **Ports** — published ports of the repo's running containers, then the repo's default compose files; `lsof` (+ `ss` on Linux) for ownership; a TCP connect for health
- **Tools** — `tool_use` blocks in `~/.claude/projects/**/*.jsonl`, deduped by `tool_use.id`
- **Skills** — `SKILL.md` files in `~/.claude/skills` (including synced sub-folders), `<repo>/.claude/skills`, and the skills of plugins listed in `~/.claude/plugins/installed_plugins.json`, plus `.claude/commands`; usage from `Skill` tool-use blocks and typed `/commands`
- **Projects** — `projects.toml` / `state.toml` (above); suggestions and per-project activity from `~/.claude.json` and transcript `cwd` fields

> **v1 is read-only.** The cockpit inspects; it never kills a job, stops a container, or touches your branches.

## Building from source

```bash
cargo build --release    # binary at target/release/claude-cockpit
cargo test               # unit + headless render tests
cargo clippy
```

## Acknowledgements

Built on [`ratatui`](https://ratatui.rs). LOC counting by [`tokei`](https://github.com/XAMPPRocky/tokei); pricing data from [LiteLLM](https://github.com/BerriAI/litellm).

## License

MIT — see [LICENSE](LICENSE).
