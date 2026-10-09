//! `claude-cockpit project …` — manage the registry from the shell.

use std::path::Path;

use clap::Subcommand;

use super::discover::{github_names, suggestions, Source};
use super::model::{ProjectError, Registry};
use super::paths::{contract_tilde, expand_tilde, repo_root_of};
use super::store::{load_registry, load_state, save_registry, save_state};

#[derive(Debug, Subcommand)]
pub enum ProjectCmd {
    /// Register a repository (default: the one you are in).
    Add {
        /// Any path inside the repository. Worktrees resolve to their main repo.
        path: Option<String>,
        /// Alias; defaults to the GitHub repo name or the folder name.
        #[arg(long, short)]
        name: Option<String>,
        /// Tag to attach (repeatable).
        #[arg(long = "tag", short)]
        tags: Vec<String>,
        /// Pin it (sorted first, gets a 1–9 hotkey in the cockpit).
        #[arg(long)]
        pin: bool,
    },
    /// List registered projects.
    #[command(alias = "ls")]
    List {
        /// Also list suggestions from Claude Code history and scan roots.
        #[arg(long, short)]
        all: bool,
        /// Only projects with this tag.
        #[arg(long, short)]
        tag: Option<String>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Unregister a project (the repository itself is untouched).
    #[command(alias = "rm")]
    Remove { project: String },
    /// Give a project a new alias.
    Rename { project: String, new_name: String },
    /// Pin a project.
    Pin { project: String },
    /// Unpin a project.
    Unpin { project: String },
    /// Edit tags: `+work`, `-old`, or a bare `tag` to add.
    Tag {
        project: String,
        #[arg(required = true, allow_hyphen_values = true)]
        edits: Vec<String>,
    },
    /// Show repos you have worked in that are not registered yet.
    Discover {
        /// Register every suggestion instead of just listing them.
        #[arg(long)]
        add_all: bool,
    },
    /// Hide a suggestion so discovery stops offering it.
    Hide { path: String },
    /// Remove projects whose directory no longer exists.
    Prune,
}

/// Run a subcommand; returns the process exit code.
pub fn run(cmd: ProjectCmd) -> i32 {
    match dispatch(cmd) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("claude-cockpit: {e}");
            1
        }
    }
}

fn dispatch(cmd: ProjectCmd) -> Result<(), ProjectError> {
    let mut reg = load_registry()?;
    match cmd {
        ProjectCmd::List { all, tag, json } => return list(&reg, all, tag.as_deref(), json),
        ProjectCmd::Discover { add_all } => return discover(&mut reg, add_all),
        ProjectCmd::Hide { path } => return hide(&path),
        ProjectCmd::Add {
            path,
            name,
            tags,
            pin,
        } => add(&mut reg, path, name, tags, pin)?,
        ProjectCmd::Remove { project } => {
            let p = reg.remove(&project)?;
            println!("removed {} ({})", p.name, contract_tilde(&p.path));
        }
        ProjectCmd::Rename { project, new_name } => {
            reg.rename(&project, &new_name)?;
            println!("renamed to {new_name}");
        }
        ProjectCmd::Pin { project } => reg.set_pinned(&project, true)?,
        ProjectCmd::Unpin { project } => reg.set_pinned(&project, false)?,
        ProjectCmd::Tag { project, edits } => reg.edit_tags(&project, &edits)?,
        ProjectCmd::Prune => {
            for p in reg.prune() {
                println!("pruned {} ({})", p.name, contract_tilde(&p.path));
            }
        }
    }
    save_registry(&reg)
}

fn add(
    reg: &mut Registry,
    path: Option<String>,
    name: Option<String>,
    tags: Vec<String>,
    pin: bool,
) -> Result<(), ProjectError> {
    let raw = path.unwrap_or_else(|| ".".into());
    let dir = expand_tilde(&raw);
    let root = repo_root_of(&dir).ok_or_else(|| ProjectError::NotARepo(raw.clone()))?;
    let name = match name {
        Some(n) => n,
        None => {
            let names = github_names(&dirs::home_dir().unwrap_or_default().join(".claude.json"));
            reg.unique_name(&super::discover::default_name(
                &super::paths::canonical(&root),
                &names,
            ))
        }
    };
    let added = reg.add(&root, Some(&name))?.name.clone();
    if !tags.is_empty() {
        reg.edit_tags(&added, &tags)?;
    }
    reg.set_pinned(&added, pin)?;
    let mut st = load_state();
    st.unhide(&root);
    let _ = save_state(&st);
    println!("added {added} ({})", contract_tilde(&root));
    Ok(())
}

fn list(reg: &Registry, all: bool, tag: Option<&str>, json: bool) -> Result<(), ProjectError> {
    let projects: Vec<_> = reg
        .ordered()
        .into_iter()
        .filter(|p| tag.is_none_or(|t| p.tags.iter().any(|x| x == t)))
        .collect();
    let sugg = if all {
        suggestions(reg, &load_state())
    } else {
        Vec::new()
    };
    if json {
        let rows: Vec<_> = projects
            .iter()
            .map(|p| serde_json::json!({"name": p.name, "path": p.path, "tags": p.tags, "pinned": p.pinned, "exists": p.exists(), "registered": true}))
            .chain(sugg.iter().map(|s| serde_json::json!({"name": s.name, "path": s.path, "last_active": s.last_active, "registered": false})))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
        return Ok(());
    }
    if projects.is_empty() && sugg.is_empty() {
        println!("no projects yet — `claude-cockpit project add [path]`, or `project discover`");
    }
    let width = projects
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    for p in &projects {
        let mark = if p.pinned { "★" } else { " " };
        let gone = if p.exists() { "" } else { "  (missing)" };
        let tags = if p.tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", p.tags.join(", "))
        };
        println!(
            "{mark} {:width$}  {}{tags}{gone}",
            p.name,
            contract_tilde(&p.path)
        );
    }
    if !sugg.is_empty() {
        println!("\nsuggested (add with `claude-cockpit project add <path>`):");
        for s in &sugg {
            println!("  {:width$}  {}", s.name, contract_tilde(&s.path));
        }
    }
    Ok(())
}

fn discover(reg: &mut Registry, add_all: bool) -> Result<(), ProjectError> {
    let sugg = suggestions(reg, &load_state());
    if sugg.is_empty() {
        println!("nothing new to suggest");
        return Ok(());
    }
    for s in &sugg {
        let from = match s.source {
            Source::ClaudeHistory => "claude history",
            Source::ScanRoot => "scan root",
        };
        let ago = s
            .last_active
            .map(|t| {
                format!(
                    ", active {} ago",
                    crate::util::human_duration((chrono::Utc::now().timestamp() - t).max(0) as u64)
                )
            })
            .unwrap_or_default();
        println!("  {:24} {}  ({from}{ago})", s.name, contract_tilde(&s.path));
        if add_all {
            let name = reg.unique_name(&s.name);
            reg.add(&s.path, Some(&name))?;
        }
    }
    if add_all {
        save_registry(reg)?;
        println!("added {} project(s)", sugg.len());
    }
    Ok(())
}

fn hide(raw: &str) -> Result<(), ProjectError> {
    let dir = expand_tilde(raw);
    let root = repo_root_of(&dir).unwrap_or_else(|| dir.clone());
    let mut st = load_state();
    st.hide(Path::new(&root));
    save_state(&st)?;
    println!("hidden {}", contract_tilde(&root));
    Ok(())
}
