//! On-disk skill and command discovery.
//!
//! Sources, in precedence order (earlier wins on a name clash, matching how
//! Claude Code resolves them): personal (`~/.claude/skills/**/SKILL.md`, which
//! includes synced skills nested under `skills/synced/<id>/`), personal
//! commands (`~/.claude/commands/*.md`), project skills and commands
//! (`<root>/.claude/…`), then installed plugins. Plugins come from
//! `~/.claude/plugins/installed_plugins.json` install paths (plus synced
//! plugins under `plugins/synced/<id>/<plugin>/`) — never a blind walk of
//! `~/.claude/plugins`, which also holds marketplace checkouts and `.trash/`.
//! Plugin skills are keyed `<plugin>:<name>`, the form the `Skill` tool uses.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{parse_frontmatter, Skill};

/// How deep to look for nested `SKILL.md` files (e.g. `synced/<id>/<name>/`).
const SKILL_DEPTH: usize = 4;

fn new_skill(name: String, description: String, source: &str, path: &Path) -> Skill {
    Skill {
        name,
        description,
        source: source.to_string(),
        path: Some(path.to_path_buf()),
        uses: 0,
        last_used: None,
    }
}

fn namespaced(ns: Option<&str>, name: &str) -> String {
    match ns {
        Some(ns) => format!("{ns}:{name}"),
        None => name.to_string(),
    }
}

/// Build a skill from a `SKILL.md` (fallback name = its directory).
fn skill_from_file(path: &Path, source: &str, ns: Option<&str>) -> Option<Skill> {
    let md = std::fs::read_to_string(path).ok()?;
    let fallback = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (name, description) = parse_frontmatter(&md, &fallback);
    if name.is_empty() {
        return None;
    }
    Some(new_skill(namespaced(ns, &name), description, source, path))
}

/// Collect `SKILL.md` skills under `dir`: a directory containing `SKILL.md` is
/// one skill; otherwise recurse (bounded by `depth`, skipping hidden dirs).
fn scan_skill_tree(dir: &Path, depth: usize, source: &str, ns: Option<&str>, out: &mut Vec<Skill>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    dirs.sort();
    for d in dirs {
        let md = d.join("SKILL.md");
        if md.is_file() {
            out.extend(skill_from_file(&md, source, ns));
        } else {
            scan_skill_tree(&d, depth - 1, source, ns, out);
        }
    }
}

/// Collect `<dir>/*.md` custom slash commands (name = file stem).
fn scan_commands(dir: &Path, source: &str, ns: Option<&str>, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort();
    for p in files {
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let md = std::fs::read_to_string(&p).unwrap_or_default();
        // Commands use frontmatter for `description` only; the name is the file.
        let (_, description) = parse_frontmatter(&md, &stem);
        out.push(new_skill(namespaced(ns, &stem), description, source, &p));
    }
}

/// `(plugin name, install path)` for plugins in an `installed_plugins.json`
/// body that apply to `root` (user-scoped, or project/local-scoped to `root`).
pub(super) fn installed_plugins(json: &str, root: &str) -> Vec<(String, PathBuf)> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return vec![];
    };
    let Some(plugins) = v.get("plugins").and_then(Value::as_object) else {
        return vec![];
    };
    let mut out = Vec::new();
    for (key, installs) in plugins {
        let name = key.split('@').next().unwrap_or(key).to_string();
        for inst in installs.as_array().into_iter().flatten() {
            let Some(path) = inst.get("installPath").and_then(Value::as_str) else {
                continue;
            };
            let project = inst.get("projectPath").and_then(Value::as_str);
            if project.is_some_and(|p| Path::new(p) != Path::new(root)) {
                continue;
            }
            out.push((name.clone(), PathBuf::from(path)));
        }
    }
    out
}

/// Synced plugins: `<plugins>/synced/<id>/<plugin>/.claude-plugin/plugin.json`.
fn synced_plugins(plugins_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(ids) = std::fs::read_dir(plugins_dir.join("synced")) else {
        return out;
    };
    for id in ids.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(plugins) = std::fs::read_dir(&id) else {
            continue;
        };
        for p in plugins.flatten().map(|e| e.path()) {
            let manifest = p.join(".claude-plugin/plugin.json");
            let Ok(body) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let name = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
                .or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            if let Some(name) = name {
                out.push((name, p));
            }
        }
    }
    out
}

/// Discover skills and commands on disk (see module docs for precedence).
/// Deduped by name, earlier sources winning.
pub fn discover(root: &str) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let home = crate::util::claude_home();
    if let Some(home) = &home {
        scan_skill_tree(
            &home.join("skills"),
            SKILL_DEPTH,
            "personal",
            None,
            &mut out,
        );
        scan_commands(&home.join("commands"), "command", None, &mut out);
    }
    let project = Path::new(root).join(".claude");
    scan_skill_tree(
        &project.join("skills"),
        SKILL_DEPTH,
        "project",
        None,
        &mut out,
    );
    scan_commands(&project.join("commands"), "command", None, &mut out);
    if let Some(home) = &home {
        let plugins_dir = home.join("plugins");
        let json =
            std::fs::read_to_string(plugins_dir.join("installed_plugins.json")).unwrap_or_default();
        let mut plugins = installed_plugins(&json, root);
        plugins.extend(synced_plugins(&plugins_dir));
        for (name, path) in &plugins {
            scan_skill_tree(&path.join("skills"), 2, "plugin", Some(name), &mut out);
            scan_commands(&path.join("commands"), "plugin", Some(name), &mut out);
        }
    }
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.name.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cockpit-skills-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn finds_nested_synced_skills_and_commands() {
        let d = tmpdir("nested");
        write(&d.join("skills/top/SKILL.md"), "---\nname: top\n---\n");
        write(
            &d.join("skills/synced/abc-123/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: PDFs\n---\n",
        );
        write(
            &d.join("skills/.staging/x/SKILL.md"),
            "---\nname: hidden\n---\n",
        );
        let mut out = Vec::new();
        scan_skill_tree(&d.join("skills"), SKILL_DEPTH, "personal", None, &mut out);
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"top") && names.contains(&"pdf"),
            "{names:?}"
        );
        assert!(!names.contains(&"hidden"));

        write(
            &d.join("commands/ship.md"),
            "---\ndescription: Ship it\n---\nbody",
        );
        let mut cmds = Vec::new();
        scan_commands(&d.join("commands"), "command", None, &mut cmds);
        assert_eq!(cmds[0].name, "ship");
        assert_eq!(cmds[0].description, "Ship it");
        assert_eq!(cmds[0].source, "command");
    }

    #[test]
    fn plugin_skills_are_namespaced() {
        let d = tmpdir("plugin");
        write(
            &d.join("skills/review/SKILL.md"),
            "---\nname: review\n---\n",
        );
        let mut out = Vec::new();
        scan_skill_tree(&d.join("skills"), 2, "plugin", Some("toolkit"), &mut out);
        assert_eq!(out[0].name, "toolkit:review");
    }

    #[test]
    fn parses_installed_plugins_json() {
        let j = r#"{"version":2,"plugins":{
            "lsp@official":[{"scope":"user","installPath":"/p/lsp/1.0.0"}],
            "other@m":[{"scope":"project","projectPath":"/elsewhere","installPath":"/p/other"}],
            "mine@m":[{"scope":"local","projectPath":"/repo","installPath":"/p/mine"}]}}"#;
        let mut v = installed_plugins(j, "/repo");
        v.sort();
        assert_eq!(
            v,
            vec![
                ("lsp".to_string(), PathBuf::from("/p/lsp/1.0.0")),
                ("mine".to_string(), PathBuf::from("/p/mine")),
            ]
        );
        assert!(installed_plugins("not json", "/repo").is_empty());
    }
}
