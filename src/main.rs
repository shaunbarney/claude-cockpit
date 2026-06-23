mod app;
mod collect;
mod event;
mod layout;
mod refresh;
mod render;
mod theme;
mod util;
mod widget;

use clap::Parser;
use render::Mode;

/// claude-cockpit — a btm-style terminal dashboard for Claude Code and your dev environment.
///
/// Run with no arguments to launch the interactive cockpit. The one-shot flags
/// (`--worktrees`, `--code`, `--watch`) print a static table instead, handy for scripts.
#[derive(Parser)]
#[command(
    name = "claude-cockpit",
    about = "A btm-style terminal dashboard for Claude Code and your dev environment",
    version
)]
struct Cli {
    /// Force the interactive cockpit (this is the default with no flags).
    #[arg(long)]
    tui: bool,
    /// One-shot: worktrees table only.
    #[arg(long, conflicts_with = "code")]
    worktrees: bool,
    /// One-shot: worktrees + repo LOC, stacked vertically.
    #[arg(long)]
    code: bool,
    /// Refresh the one-shot render in place continuously.
    #[arg(long)]
    watch: bool,
    /// Refresh seconds for the one-shot `--watch` render.
    #[arg(long, default_value_t = 10.0)]
    interval: f64,
}

fn main() {
    let cli = Cli::parse();
    let root = collect::git::repo_root();

    // Default (no one-shot flag) launches the interactive cockpit.
    let wants_oneshot = cli.worktrees || cli.code || cli.watch;
    if cli.tui || !wants_oneshot {
        if let Err(e) = event::run(&root) {
            eprintln!("claude-cockpit: error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let mode = if cli.worktrees {
        Mode::Worktrees
    } else if cli.code {
        Mode::Code
    } else {
        Mode::Side
    };

    if cli.watch {
        render::watch(&root, mode, cli.interval);
    } else {
        collect::git::fetch_origin(&root);
        println!("{}", render::build_frame(&root, mode, ""));
    }
}
