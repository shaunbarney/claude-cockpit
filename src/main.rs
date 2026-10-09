use std::io::IsTerminal;

use claude_cockpit::projects::{self, cli::ProjectCmd};
use claude_cockpit::{config, refresh, render, runloop};

use clap::{Parser, Subcommand};
use render::Mode;

/// Shortest accepted `--interval`, in seconds. Anything faster just burns CPU
/// re-running git and tokei.
const MIN_INTERVAL: f64 = 0.5;

/// claude-cockpit — a live, read-only terminal dashboard for Claude Code and your dev environment.
///
/// Run with no arguments to launch the interactive cockpit. The one-shot flags
/// (`--worktrees`, `--code`, `--watch`) print a static table instead, handy for
/// scripts. `--watch` on its own shows worktrees and repo LOC side by side.
#[derive(Parser)]
#[command(
    name = "claude-cockpit",
    about = "A read-only terminal dashboard for Claude Code — agents, cost, rate-limit headroom, worktrees, Docker, and tool usage",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    /// Open this project: a registered name, or any path inside a git repo.
    /// Without it the cockpit opens the repo you are in, else the project you
    /// had open last, else the project picker.
    #[arg(long, short = 'p', global = true)]
    project: Option<String>,
    /// Force the interactive cockpit (this is the default with no flags).
    #[arg(long, conflicts_with_all = ["worktrees", "code", "watch"])]
    tui: bool,
    /// One-shot: worktrees table only.
    #[arg(long, conflicts_with = "code")]
    worktrees: bool,
    /// One-shot: worktrees + repo LOC, stacked vertically.
    #[arg(long)]
    code: bool,
    /// Refresh the one-shot render in place continuously. Alone, it shows
    /// worktrees and repo LOC side by side; combine with `--worktrees` or
    /// `--code` to watch that layout instead.
    #[arg(long)]
    watch: bool,
    /// Refresh seconds for the one-shot `--watch` render (finite, >= 0.5).
    #[arg(long, default_value_t = 10.0, value_parser = parse_interval)]
    interval: f64,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage the projects the cockpit can switch between.
    #[command(subcommand, alias = "projects")]
    Project(ProjectCmd),
}

/// `--interval` must be a finite number of seconds, at least [`MIN_INTERVAL`].
fn parse_interval(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .trim()
        .parse()
        .map_err(|_| format!("`{s}` is not a number of seconds"))?;
    if !v.is_finite() || v < MIN_INTERVAL {
        return Err(format!(
            "must be a finite number of seconds >= {MIN_INTERVAL}"
        ));
    }
    Ok(v)
}

/// Print config problems (parse errors, unknown keys) to stderr.
fn report_config(root: &str) {
    for d in config::load_cached(root).1 {
        eprintln!("claude-cockpit: config: {d}");
    }
}

fn run_tui(root: &str) {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        eprintln!(
            "claude-cockpit: the interactive cockpit needs a terminal (stdin/stdout is not a TTY).\n\
             For scripts and pipes use a one-shot view: --worktrees, --code, or --watch."
        );
        std::process::exit(1);
    }
    if let Err(e) = runloop::run(root) {
        eprintln!("claude-cockpit: error: {e}");
        std::process::exit(1);
    }
}

fn run_oneshot(cli: &Cli, root: &str) {
    if root.is_empty() {
        eprintln!("claude-cockpit: not inside a git repository — cd into a repo and try again.");
        std::process::exit(1);
    }
    report_config(root);
    let mode = if cli.worktrees {
        Mode::Worktrees
    } else if cli.code {
        Mode::Code
    } else {
        Mode::Side
    };
    if cli.watch {
        render::watch(root, mode, cli.interval);
    } else {
        refresh::fetch_origin_bounded(root);
        println!("{}", render::build_frame(root, mode, ""));
    }
}

/// The project to open, as a repo root ("" = none: the TUI starts in the picker).
fn startup_root(arg: Option<&str>) -> String {
    let reg = match projects::store::load_registry() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("claude-cockpit: {e}");
            projects::Registry::default()
        }
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    match projects::resolve::startup(arg, &cwd, &reg, &projects::store::load_state()) {
        Ok(root) => root
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        Err(e) => {
            eprintln!("claude-cockpit: {e} — see `claude-cockpit project ls`");
            std::process::exit(1);
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if let Some(Cmd::Project(cmd)) = cli.command {
        std::process::exit(projects::cli::run(cmd));
    }
    let root = startup_root(cli.project.as_deref());

    let wants_oneshot = cli.worktrees || cli.code || cli.watch;
    if cli.tui || !wants_oneshot {
        run_tui(&root);
    } else {
        run_oneshot(&cli, &root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn interval_validation() {
        assert_eq!(parse_interval("2.5"), Ok(2.5));
        assert_eq!(parse_interval("0.5"), Ok(0.5));
        for bad in ["0", "0.1", "-1", "nan", "inf", "-inf", "abc", ""] {
            assert!(parse_interval(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn tui_conflicts_with_oneshot_flags() {
        for flag in ["--worktrees", "--code", "--watch"] {
            assert!(Cli::try_parse_from(["claude-cockpit", "--tui", flag]).is_err());
        }
        assert!(Cli::try_parse_from(["claude-cockpit", "--watch", "--code"]).is_ok());
    }

    #[test]
    fn project_subcommands_and_flag_parse() {
        let c = Cli::try_parse_from([
            "claude-cockpit",
            "project",
            "add",
            ".",
            "--name",
            "x",
            "--pin",
        ])
        .unwrap();
        assert!(matches!(
            c.command,
            Some(Cmd::Project(ProjectCmd::Add { pin: true, .. }))
        ));
        let c = Cli::try_parse_from(["claude-cockpit", "projects", "ls", "--all"]).unwrap();
        assert!(matches!(
            c.command,
            Some(Cmd::Project(ProjectCmd::List { all: true, .. }))
        ));
        let c = Cli::try_parse_from(["claude-cockpit", "-p", "sovra", "--worktrees"]).unwrap();
        assert_eq!(c.project.as_deref(), Some("sovra"));
        assert!(
            Cli::try_parse_from(["claude-cockpit", "project", "tag", "x", "-old", "+new"]).is_ok()
        );
    }
}
