---
name: deploy
description: Run the full CI gate locally (fmt, clippy, test) and only push to origin if it all passes — so green-local means green-CI. Use whenever asked to push, deploy, ship, or release claude-cockpit. Optionally publishes to crates.io.
---

# Deploy claude-cockpit

CI (`.github/workflows/ci.yml`) runs **three** gates that must all pass, and a
local `cargo test`/`cargo clippy` is NOT equivalent — CI uses stricter flags.
Reproduce CI **exactly** before every push. The most common miss is rustfmt
(easy to forget) and clippy's `-D warnings` + `--all-targets` (catches issues a
plain `cargo clippy` does not).

## The gate — run all three, in order, from the repo root

```bash
cargo fmt --all -- --check          # CI fails on ANY unformatted line
cargo clippy --all-targets -- -D warnings   # warnings are errors; includes tests
cargo test --all
```

If `fmt --check` fails, run `cargo fmt --all` to fix it, then re-run the gate.
Fix clippy/test failures in the code — never silence them with `#[allow]` unless
there's a documented reason. Do not push until **all three** exit 0.

## Push

Only after the gate is green:

```bash
git add -A
git commit -m "<type>: <summary>"   # end body with the Co-Authored-By trailer
git push origin main
```

This is the user's own repo and they push directly to `main` (no PR). Confirm
CI went green afterwards if `gh` is available:

```bash
gh run list --limit 1
gh run watch    # or: gh run view --log-failed  on failure
```

## Publish to crates.io (only when explicitly asked to "publish" / "release")

Publishing is **irreversible** — a version can be yanked but never re-uploaded.
First confirm `Cargo.toml` has `description`, `license`, `repository`, and a
bumped `version` (crates.io rejects a re-used version). Dry-run, then publish:

```bash
cargo publish --dry-run
cargo publish
```

The crates.io token is stored once via `cargo login` (in
`~/.cargo/credentials.toml`); never commit it or echo it into the repo.
