//! Loading and saving the registry (config) and the cockpit's own state.
//!
//! Unlike the rest of the cockpit, a broken `projects.toml` is an error, not an
//! empty default: silently treating it as empty and then saving would wipe the
//! user's list on the next `project add`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{ProjectError, Registry};
use super::paths::{canonical, contract_tilde, expand_tilde, registry_file, state_file};

const HEADER: &str = "\
# claude-cockpit projects. Edit freely, or use `claude-cockpit project …`.
# Each [[project]] needs a unique `name` and the repo `path`; `tags` and
# `pinned` are optional. [discovery] controls the suggestions in the picker.
";

/// How many recently opened projects to remember.
const RECENT_MAX: usize = 20;

/// What the cockpit remembers between runs. Never hand-edited.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Root of the project open when the cockpit last exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// Most recent first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<Recent>,
    /// Suggestions the user dismissed; discovery skips these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: String,
    /// Unix seconds.
    pub at: i64,
}

impl State {
    /// Record `root` as just opened.
    pub fn touch(&mut self, root: &Path, now: i64) {
        let key = contract_tilde(&canonical(root));
        self.recent.retain(|r| r.path != key);
        self.recent.insert(
            0,
            Recent {
                path: key.clone(),
                at: now,
            },
        );
        self.recent.truncate(RECENT_MAX);
        self.last = Some(key);
    }

    pub fn last_path(&self) -> Option<PathBuf> {
        self.last.as_deref().map(expand_tilde)
    }

    pub fn is_hidden(&self, root: &Path) -> bool {
        let key = contract_tilde(&canonical(root));
        self.hidden.contains(&key)
    }

    pub fn hide(&mut self, root: &Path) {
        let key = contract_tilde(&canonical(root));
        if !self.hidden.contains(&key) {
            self.hidden.push(key);
        }
    }

    pub fn unhide(&mut self, root: &Path) {
        let key = contract_tilde(&canonical(root));
        self.hidden.retain(|h| *h != key);
    }

    /// When `root` was last opened, if it is in the recent list.
    pub fn opened_at(&self, root: &Path) -> Option<i64> {
        let key = contract_tilde(&canonical(root));
        self.recent.iter().find(|r| r.path == key).map(|r| r.at)
    }
}

fn read_toml<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ProjectError> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s)
            .map_err(|e| ProjectError::Io(format!("{}: {e}", contract_tilde(path)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(ProjectError::Io(format!("{}: {e}", contract_tilde(path)))),
    }
}

/// Write via a sibling temp file + rename so a crash never leaves half a file.
fn write_atomic(path: &Path, body: &str) -> Result<(), ProjectError> {
    let io = |e: std::io::Error| ProjectError::Io(format!("{}: {e}", contract_tilde(path)));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

fn no_home() -> ProjectError {
    ProjectError::Io("cannot locate a home directory for the config".into())
}

pub fn load_registry_from(path: &Path) -> Result<Registry, ProjectError> {
    read_toml(path)
}

pub fn save_registry_to(path: &Path, reg: &Registry) -> Result<(), ProjectError> {
    let body = toml::to_string_pretty(reg).map_err(|e| ProjectError::Io(e.to_string()))?;
    write_atomic(path, &format!("{HEADER}\n{body}"))
}

pub fn load_registry() -> Result<Registry, ProjectError> {
    load_registry_from(&registry_file().ok_or_else(no_home)?)
}

pub fn save_registry(reg: &Registry) -> Result<(), ProjectError> {
    save_registry_to(&registry_file().ok_or_else(no_home)?, reg)
}

/// State is a convenience: an unreadable state file degrades to empty.
pub fn load_state() -> State {
    state_file()
        .and_then(|p| read_toml(&p).ok())
        .unwrap_or_default()
}

pub fn save_state(st: &State) -> Result<(), ProjectError> {
    let path = state_file().ok_or_else(no_home)?;
    let body = toml::to_string_pretty(st).map_err(|e| ProjectError::Io(e.to_string()))?;
    write_atomic(&path, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::testutil::temp_dir;

    #[test]
    fn missing_file_is_empty_but_broken_file_is_an_error() {
        let d = temp_dir("store");
        let f = d.join("projects.toml");
        assert_eq!(load_registry_from(&f).unwrap(), Registry::default());
        std::fs::write(&f, "[[project]\nname=").unwrap();
        assert!(load_registry_from(&f).is_err());
    }

    #[test]
    fn save_then_load_round_trips_with_header() {
        let d = temp_dir("store-rt");
        let f = d.join("sub/projects.toml");
        let mut r = Registry::default();
        r.add(Path::new("/nonexistent/demo"), None).unwrap();
        save_registry_to(&f, &r).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.starts_with("# claude-cockpit projects"));
        assert_eq!(load_registry_from(&f).unwrap(), r);
    }

    #[test]
    fn touch_dedupes_and_caps_recents() {
        let mut st = State::default();
        for i in 0..30 {
            st.touch(Path::new(&format!("/nonexistent/p{i}")), i);
        }
        st.touch(Path::new("/nonexistent/p5"), 100);
        assert_eq!(st.recent.len(), RECENT_MAX);
        assert_eq!(st.recent[0].path, "/nonexistent/p5");
        assert_eq!(
            st.recent.iter().filter(|r| r.path.ends_with("/p5")).count(),
            1
        );
        assert_eq!(st.last_path(), Some(PathBuf::from("/nonexistent/p5")));
        assert_eq!(st.opened_at(Path::new("/nonexistent/p5")), Some(100));
    }

    #[test]
    fn hide_and_unhide() {
        let mut st = State::default();
        let p = Path::new("/nonexistent/x");
        st.hide(p);
        st.hide(p);
        assert!(st.is_hidden(p));
        assert_eq!(st.hidden.len(), 1);
        st.unhide(p);
        assert!(!st.is_hidden(p));
    }
}
