//! Suggest projects you have not registered yet.
//!
//! Claude Code already knows where you work: `~/.claude.json` keeps a
//! `projects` map keyed by directory, and every transcript under
//! `~/.claude/projects/<dir>/` records the `cwd` it ran in. (The folder names
//! encode the path with `/` and `.` both turned into `-`, so they cannot be
//! decoded reliably — the `cwd` field is the source of truth.) Each candidate is
//! folded to its repository root, so a dozen worktree sessions become one
//! suggestion. Optional scan roots add repos Claude has never seen.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::model::{slug, Registry};
use super::paths::{canonical, expand_tilde, repo_root_of};
use super::store::State;

/// How many leading lines of a transcript to search for its `cwd`.
const CWD_SCAN_LINES: usize = 50;
/// Directories never worth descending into when scanning roots.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "venv"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    ClaudeHistory,
    ScanRoot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub path: PathBuf,
    pub name: String,
    /// Newest transcript activity, Unix seconds.
    pub last_active: Option<i64>,
    pub source: Source,
}

/// Every suggestion not already registered or hidden, most recently active first.
pub fn suggestions(reg: &Registry, state: &State) -> Vec<Suggestion> {
    let mut raw: Vec<(PathBuf, Option<i64>, Source)> = Vec::new();
    if reg.discovery.claude_history {
        if let Some(home) = dirs::home_dir() {
            raw.extend(
                claude_json_dirs(&home.join(".claude.json"))
                    .into_iter()
                    .map(|p| (p, None, Source::ClaudeHistory)),
            );
            raw.extend(
                transcript_dirs(&home.join(".claude/projects"))
                    .into_iter()
                    .map(|(p, t)| (p, Some(t), Source::ClaudeHistory)),
            );
        }
    }
    for root in &reg.discovery.roots {
        raw.extend(
            scan_root(&root.path, root.depth)
                .into_iter()
                .map(|p| (p, None, Source::ScanRoot)),
        );
    }
    let names = github_names(&dirs::home_dir().unwrap_or_default().join(".claude.json"));
    fold(raw, reg, state, &names)
}

/// Resolve raw directories to repo roots, merge duplicates, drop the noise.
fn fold(
    raw: Vec<(PathBuf, Option<i64>, Source)>,
    reg: &Registry,
    state: &State,
    names: &HashMap<PathBuf, String>,
) -> Vec<Suggestion> {
    let home = dirs::home_dir().map(|h| canonical(&h));
    let mut roots: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut by_root: HashMap<PathBuf, Suggestion> = HashMap::new();
    for (dir, at, source) in raw {
        let root = match roots.get(&dir) {
            Some(r) => r.clone(),
            None => {
                let Some(r) = repo_root_of(&dir).map(|r| canonical(&r)) else {
                    continue;
                };
                roots.insert(dir.clone(), r.clone());
                r
            }
        };
        if Some(&root) == home.as_ref()
            || reg
                .containing(&root)
                .is_some_and(|p| canonical(&p.path) == root)
            || state.is_hidden(&root)
        {
            continue;
        }
        let entry = by_root.entry(root.clone()).or_insert_with(|| Suggestion {
            name: default_name(&root, names),
            path: root.clone(),
            last_active: None,
            source,
        });
        entry.last_active = entry.last_active.max(at);
    }
    let mut out: Vec<Suggestion> = by_root.into_values().collect();
    out.sort_by(|a, b| {
        b.last_active
            .cmp(&a.last_active)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// The repo half of a `githubRepoPaths` entry when one points here, else the
/// folder name.
pub fn default_name(root: &Path, names: &HashMap<PathBuf, String>) -> String {
    if let Some(n) = names.get(root) {
        return slug(n);
    }
    slug(
        &root
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
    )
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Keys of `~/.claude.json` → `projects`. Only the keys are read.
fn claude_json_dirs(path: &Path) -> Vec<PathBuf> {
    read_json(path)
        .and_then(|v| {
            v.get("projects")?
                .as_object()
                .map(|m| m.keys().map(PathBuf::from).collect())
        })
        .unwrap_or_default()
}

/// `~/.claude.json` → `githubRepoPaths` (`owner/repo` → [paths]) inverted to
/// path → `repo`.
pub fn github_names(path: &Path) -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    let Some(v) = read_json(path) else { return out };
    let Some(map) = v.get("githubRepoPaths").and_then(|m| m.as_object()) else {
        return out;
    };
    for (slug, paths) in map {
        let repo = slug.rsplit('/').next().unwrap_or(slug).to_string();
        for p in paths
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str())
        {
            out.insert(canonical(Path::new(p)), repo.clone());
        }
    }
    out
}

/// One `(cwd, newest mtime)` per transcript folder.
fn transcript_dirs(projects: &Path) -> Vec<(PathBuf, i64)> {
    let Ok(rd) = std::fs::read_dir(projects) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| newest_transcript(&e.path()))
        .filter_map(|(f, at)| first_cwd(&f).map(|cwd| (cwd, at)))
        .collect()
}

fn newest_transcript(dir: &Path) -> Option<(PathBuf, i64)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let t = e.metadata().ok()?.modified().ok()?;
            let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
            Some((e.path(), secs))
        })
        .max_by_key(|(_, t)| *t)
}

fn first_cwd(file: &Path) -> Option<PathBuf> {
    let f = std::fs::File::open(file).ok()?;
    BufReader::new(f)
        .lines()
        .take(CWD_SCAN_LINES)
        .map_while(Result::ok)
        .find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(&line).ok()?;
            v.get("cwd")?.as_str().map(PathBuf::from)
        })
}

/// Git repositories at most `depth` levels below `root`. A repo is not
/// descended into — its nested folders are its own business.
pub fn scan_root(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&expand_tilde(&root.to_string_lossy()), depth, &mut out);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() {
        out.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&e.path(), depth - 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::testutil::{temp_dir, temp_repo};

    #[test]
    fn scan_finds_repos_within_depth_and_skips_their_insides() {
        let top = temp_dir("scan");
        let a = top.join("a");
        let deep = top.join("x/y/z");
        std::fs::create_dir_all(a.join(".git")).unwrap();
        std::fs::create_dir_all(a.join("nested/.git")).unwrap();
        std::fs::create_dir_all(deep.join(".git")).unwrap();
        std::fs::create_dir_all(top.join("node_modules/pkg/.git")).unwrap();
        let found = scan_root(&top, 2);
        assert_eq!(found, vec![a.clone()]);
        let found = scan_root(&top, 3);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn transcript_cwd_is_read_from_the_newest_file() {
        let projects = temp_dir("transcripts");
        let d = projects.join("-some-dir");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("a.jsonl"),
            "{\"type\":\"summary\"}\n{\"cwd\":\"/work/repo\"}\n",
        )
        .unwrap();
        let got = transcript_dirs(&projects);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, PathBuf::from("/work/repo"));
    }

    #[test]
    fn fold_merges_worktrees_and_skips_registered_and_hidden() {
        let repo = temp_repo("fold");
        let wt = repo.join("wt");
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "-q", "-b", "b"])
            .arg(&wt)
            .status()
            .unwrap();
        let other = temp_repo("fold-other");
        let raw = vec![
            (wt.clone(), Some(50), Source::ClaudeHistory),
            (repo.join("sub-not-a-dir"), Some(10), Source::ClaudeHistory),
            (repo.clone(), Some(20), Source::ClaudeHistory),
            (other.clone(), Some(5), Source::ScanRoot),
            (
                PathBuf::from("/definitely/missing"),
                Some(99),
                Source::ClaudeHistory,
            ),
        ];
        let got = fold(
            raw.clone(),
            &Registry::default(),
            &State::default(),
            &HashMap::new(),
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].path, canonical(&repo));
        assert_eq!(got[0].last_active, Some(50));

        let mut reg = Registry::default();
        reg.add(&repo, None).unwrap();
        let mut st = State::default();
        st.hide(&other);
        assert!(fold(raw, &reg, &st, &HashMap::new()).is_empty());
    }

    #[test]
    fn github_name_wins_over_folder() {
        let mut names = HashMap::new();
        names.insert(
            PathBuf::from("/w/SovraSRXProduction"),
            "sovrasrxproduction".to_string(),
        );
        assert_eq!(
            default_name(Path::new("/w/SovraSRXProduction"), &names),
            "sovrasrxproduction"
        );
        assert_eq!(
            default_name(Path::new("/w/My Repo"), &HashMap::new()),
            "my-repo"
        );
    }
}
