//! The project registry: pure data and the edits you can make to it.
//!
//! Nothing here touches the filesystem — `store` loads and saves, this module
//! validates. Every mutation returns a `ProjectError` you can show verbatim.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paths::{canonical, contract_tilde, expand_tilde};

/// One registered project: a git repository you care about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Unique short alias — what `-p <name>` and the picker match on.
    pub name: String,
    /// Repository root. Stored `~`-relative; expanded on load.
    #[serde(with = "tilde_path")]
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Pinned projects sort first and get the `1`–`9` hotkeys.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

impl Project {
    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }
}

/// A directory tree scanned for git repositories to suggest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanRoot {
    #[serde(with = "tilde_path")]
    pub path: PathBuf,
    /// How many directory levels below `path` to look for a `.git`.
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_depth() -> usize {
    2
}

/// Where suggestions come from. Suggestions are never added on their own —
/// they wait in the picker until you press `a`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discovery {
    /// Suggest repos Claude Code has been run in (`~/.claude.json`, transcripts).
    #[serde(default = "yes")]
    pub claude_history: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<ScanRoot>,
}

fn yes() -> bool {
    true
}

impl Default for Discovery {
    fn default() -> Self {
        Discovery {
            claude_history: true,
            roots: Vec::new(),
        }
    }
}

/// The whole `projects.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub discovery: Discovery,
    #[serde(default, rename = "project")]
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectError {
    NotFound(String),
    NameTaken(String),
    PathTaken { path: String, name: String },
    InvalidName(String),
    NotARepo(String),
    Io(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::NotFound(q) => write!(f, "no project matches '{q}'"),
            ProjectError::NameTaken(n) => write!(f, "a project named '{n}' already exists"),
            ProjectError::PathTaken { path, name } => {
                write!(f, "{path} is already registered as '{name}'")
            }
            ProjectError::InvalidName(n) => write!(
                f,
                "invalid name '{n}': use letters, digits, '-', '_' or '.' (max 40)"
            ),
            ProjectError::NotARepo(p) => write!(f, "{p} is not inside a git repository"),
            ProjectError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProjectError {}

/// Names are typed on the command line and in the palette, so keep them plain.
pub fn validate_name(name: &str) -> Result<(), ProjectError> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(ProjectError::InvalidName(name.to_string()))
    }
}

/// A default alias from a folder name: lowercased, spaces to `-`, anything
/// else unusual dropped.
pub fn slug(raw: &str) -> String {
    let s: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect::<String>()
        .to_lowercase();
    let s: String = s.chars().take(40).collect();
    if s.is_empty() {
        "project".into()
    } else {
        s
    }
}

impl Registry {
    pub fn find(&self, query: &str) -> Option<&Project> {
        self.index_of(query).map(|i| &self.projects[i])
    }

    /// Match by exact name, then by path (`~` and relative paths accepted),
    /// then by unique case-insensitive name prefix.
    pub fn index_of(&self, query: &str) -> Option<usize> {
        if let Some(i) = self.projects.iter().position(|p| p.name == query) {
            return Some(i);
        }
        let as_path = canonical(&expand_tilde(query));
        if let Some(i) = self
            .projects
            .iter()
            .position(|p| canonical(&p.path) == as_path)
        {
            return Some(i);
        }
        let q = query.to_lowercase();
        let hits: Vec<usize> = (0..self.projects.len())
            .filter(|&i| self.projects[i].name.to_lowercase().starts_with(&q))
            .collect();
        (hits.len() == 1).then(|| hits[0])
    }

    /// The registered project whose root contains `p` (deepest match wins, so a
    /// repo nested in another registered repo resolves to itself).
    pub fn containing(&self, p: &Path) -> Option<&Project> {
        let p = canonical(p);
        self.projects
            .iter()
            .filter(|proj| p.starts_with(canonical(&proj.path)))
            .max_by_key(|proj| proj.path.components().count())
    }

    /// A name not yet in use: `base`, else `base-2`, `base-3`, …
    pub fn unique_name(&self, base: &str) -> String {
        let taken = |n: &str| self.projects.iter().any(|p| p.name == n);
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|i| format!("{base}-{i}"))
            .find(|n| !taken(n))
            .expect("unbounded")
    }

    /// Register `path` (already resolved to a repo root). `name` defaults to a
    /// unique slug of the folder name.
    pub fn add(&mut self, path: &Path, name: Option<&str>) -> Result<&Project, ProjectError> {
        let path = canonical(path);
        if let Some(p) = self.projects.iter().find(|p| canonical(&p.path) == path) {
            return Err(ProjectError::PathTaken {
                path: contract_tilde(&path),
                name: p.name.clone(),
            });
        }
        let name = match name {
            Some(n) => {
                validate_name(n)?;
                if self.projects.iter().any(|p| p.name == n) {
                    return Err(ProjectError::NameTaken(n.to_string()));
                }
                n.to_string()
            }
            None => {
                let base = path
                    .file_name()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                self.unique_name(&slug(&base))
            }
        };
        self.projects.push(Project {
            name,
            path,
            tags: Vec::new(),
            pinned: false,
        });
        Ok(self.projects.last().expect("just pushed"))
    }

    pub fn remove(&mut self, query: &str) -> Result<Project, ProjectError> {
        let i = self.require(query)?;
        Ok(self.projects.remove(i))
    }

    pub fn rename(&mut self, query: &str, new: &str) -> Result<(), ProjectError> {
        validate_name(new)?;
        let i = self.require(query)?;
        if self
            .projects
            .iter()
            .enumerate()
            .any(|(j, p)| j != i && p.name == new)
        {
            return Err(ProjectError::NameTaken(new.to_string()));
        }
        self.projects[i].name = new.to_string();
        Ok(())
    }

    pub fn set_pinned(&mut self, query: &str, pinned: bool) -> Result<(), ProjectError> {
        let i = self.require(query)?;
        self.projects[i].pinned = pinned;
        Ok(())
    }

    /// Apply `+tag` / `-tag` edits (a bare `tag` means add).
    pub fn edit_tags(&mut self, query: &str, edits: &[String]) -> Result<(), ProjectError> {
        let i = self.require(query)?;
        let tags = &mut self.projects[i].tags;
        for e in edits {
            if let Some(t) = e.strip_prefix('-') {
                tags.retain(|x| x != t);
            } else {
                let t = e.strip_prefix('+').unwrap_or(e).to_string();
                if !t.is_empty() && !tags.contains(&t) {
                    tags.push(t);
                }
            }
        }
        tags.sort();
        Ok(())
    }

    /// Drop projects whose directory is gone; returns what was removed.
    pub fn prune(&mut self) -> Vec<Project> {
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.projects)
            .into_iter()
            .partition(|p| !p.exists());
        self.projects = keep;
        gone
    }

    /// Pinned first (in file order), then the rest by name.
    pub fn ordered(&self) -> Vec<&Project> {
        let mut v: Vec<&Project> = self.projects.iter().collect();
        v.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| a.name.cmp(&b.name)));
        v
    }

    fn require(&self, query: &str) -> Result<usize, ProjectError> {
        self.index_of(query)
            .ok_or_else(|| ProjectError::NotFound(query.to_string()))
    }
}

/// (De)serialise paths as `~`-relative strings.
mod tilde_path {
    use super::{contract_tilde, expand_tilde};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::path::{Path, PathBuf};

    pub fn serialize<S: Serializer>(p: &Path, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&contract_tilde(p))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(expand_tilde(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        let mut r = Registry::default();
        r.add(Path::new("/nonexistent/alpha"), None).unwrap();
        r.add(Path::new("/nonexistent/Beta Repo"), None).unwrap();
        r
    }

    #[test]
    fn add_derives_unique_slugs_and_rejects_duplicates() {
        let mut r = reg();
        assert_eq!(r.projects[1].name, "beta-repo");
        let p = r.add(Path::new("/elsewhere/alpha"), None).unwrap();
        assert_eq!(p.name, "alpha-2");
        assert!(matches!(
            r.add(Path::new("/nonexistent/alpha"), None),
            Err(ProjectError::PathTaken { .. })
        ));
        assert!(matches!(
            r.add(Path::new("/x/y"), Some("alpha")),
            Err(ProjectError::NameTaken(_))
        ));
        assert!(matches!(
            r.add(Path::new("/x/z"), Some("has space")),
            Err(ProjectError::InvalidName(_))
        ));
    }

    #[test]
    fn lookup_by_name_path_and_prefix() {
        let r = reg();
        assert_eq!(r.find("alpha").unwrap().name, "alpha");
        assert_eq!(r.find("/nonexistent/alpha").unwrap().name, "alpha");
        assert_eq!(r.find("bet").unwrap().name, "beta-repo");
        assert!(r.find("zzz").is_none());
    }

    #[test]
    fn ambiguous_prefix_matches_nothing() {
        let mut r = reg();
        r.add(Path::new("/n/alpine"), None).unwrap();
        assert!(r.find("alp").is_none());
        assert_eq!(r.find("alpi").unwrap().name, "alpine");
    }

    #[test]
    fn rename_pin_tag_remove() {
        let mut r = reg();
        r.rename("alpha", "a").unwrap();
        assert!(matches!(
            r.rename("a", "beta-repo"),
            Err(ProjectError::NameTaken(_))
        ));
        r.set_pinned("beta-repo", true).unwrap();
        assert_eq!(r.ordered()[0].name, "beta-repo");
        r.edit_tags("a", &["+work".into(), "rust".into(), "-work".into()])
            .unwrap();
        assert_eq!(r.find("a").unwrap().tags, vec!["rust".to_string()]);
        assert_eq!(r.remove("a").unwrap().name, "a");
        assert!(matches!(r.remove("a"), Err(ProjectError::NotFound(_))));
    }

    #[test]
    fn containing_prefers_the_deepest_project() {
        let mut r = Registry::default();
        r.add(Path::new("/w/outer"), None).unwrap();
        r.add(Path::new("/w/outer/inner"), None).unwrap();
        assert_eq!(
            r.containing(Path::new("/w/outer/inner/src")).unwrap().name,
            "inner"
        );
        assert_eq!(
            r.containing(Path::new("/w/outer/src")).unwrap().name,
            "outer"
        );
        assert!(r.containing(Path::new("/w/other")).is_none());
    }

    #[test]
    fn prune_drops_missing_dirs() {
        let mut r = reg();
        let real = crate::projects::testutil::temp_dir("prune");
        r.add(&real, Some("real")).unwrap();
        let gone = r.prune();
        assert_eq!(gone.len(), 2);
        assert_eq!(r.projects.len(), 1);
    }

    #[test]
    fn readme_projects_example_parses() {
        let readme = include_str!("../../README.md");
        let block = readme
            .split("```toml\n")
            .skip(1)
            .filter_map(|b| b.split("```").next())
            .find(|b| b.contains("[[project]]"))
            .expect("README has a projects example");
        let reg: Registry = toml::from_str(block).unwrap();
        assert_eq!(reg.projects.len(), 1);
        assert!(reg.projects[0].pinned);
        assert_eq!(reg.discovery.roots[0].depth, 2);
    }

    #[test]
    fn toml_round_trip_uses_tilde_paths() {
        let home = dirs::home_dir().unwrap();
        let mut r = Registry::default();
        r.add(&home.join("code/demo"), None).unwrap();
        r.set_pinned("demo", true).unwrap();
        let text = toml::to_string_pretty(&r).unwrap();
        assert!(text.contains("path = \"~/code/demo\""), "{text}");
        assert!(text.contains("[[project]]"));
        let back: Registry = toml::from_str(&text).unwrap();
        assert_eq!(back, r);
    }
}
