# Contributing to claude-cockpit

Thanks for your interest in improving `claude-cockpit`! This is a small, focused
Rust TUI — contributions that keep it fast, dependency-light, and read-only are
very welcome.

## Development setup

You need Rust 1.88 or newer (via [`rustup`](https://rustup.rs)):

```bash
git clone https://github.com/shaunbarney/claude-cockpit
cd claude-cockpit
cargo build
cargo run            # launch the TUI against the current repo
```

Run it from inside a git repository with some worktrees to see real data.

## Before you open a PR

CI runs four checks on stable, plus an MSRV build. Please make sure they pass
locally first:

```bash
cargo fmt --all                                   # format (CI checks with --check)
cargo clippy --locked --all-targets -- -D warnings  # lint, warnings are errors
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features  # docs build cleanly
cargo test --locked                               # unit + headless render tests
```

The MSRV job runs `cargo check --locked --all-targets` on the `rust-version`
from `Cargo.toml` (currently 1.88, which ratatui 0.30 requires). If you raise
it, update the toolchain in `.github/workflows/ci.yml` too.

A green `cargo fmt --check`, a clippy run with zero warnings, a clean doc build,
and passing tests are required to merge.

## Design principles

Keep these in mind — they're the spirit of the project:

- **Read-only.** The cockpit inspects; it never kills a job, stops a container,
  or touches your branches. v1 is strictly observational.
- **Best-effort, never crashes.** Every data source is optional. A missing tool
  or file (`docker`, `lsof`, `~/.claude/…`) yields an empty widget, not a panic.
  Shell out through `crate::proc` so every external command has a timeout.
- **Local-first.** No daemon, no telemetry, no network for the core. The only
  network call is a best-effort price refresh that falls back to a vendored table.
- **Fast and light.** Prefer the standard library and the existing dependency set
  over pulling in new crates.

## Architecture & adding a widget

The big-picture architecture (the two render stacks, the threading/data-flow
model, the view state machine) and a step-by-step "adding a widget" checklist
live in [`CLAUDE.md`](CLAUDE.md). Read that before making structural changes.

## Tests

Tests are colocated in `#[cfg(test)] mod tests` blocks next to the code they
cover, including headless render tests that draw into a `ratatui` `TestBackend`
and assert on the buffer. New behaviour should come with a test in the same style.

## Commits & PRs

- Use clear, conventional commit messages (`feat:`, `fix:`, `docs:`, `style:`, …).
- Keep PRs small and focused; describe the change and why.
