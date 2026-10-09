//! Which project the cockpit opens on.
//!
//! Order: an explicit `-p <name|path>`, then the repo you launched it from,
//! then the project you had open last time, then your first pinned project.
//! With none of those, the cockpit opens the project picker.

use std::path::{Path, PathBuf};

use super::model::{ProjectError, Registry};
use super::paths::{expand_tilde, repo_root_of};
use super::store::State;

/// Resolve `-p <arg>`: a registered name/alias first, else any path inside a
/// git repository (registered or not).
pub fn resolve_arg(arg: &str, reg: &Registry) -> Result<PathBuf, ProjectError> {
    if let Some(p) = reg.find(arg) {
        return Ok(p.path.clone());
    }
    let as_path = expand_tilde(arg);
    if as_path.exists() {
        return repo_root_of(&as_path).ok_or_else(|| ProjectError::NotARepo(arg.to_string()));
    }
    Err(ProjectError::NotFound(arg.to_string()))
}

/// The startup project, or `None` to open the picker.
pub fn startup(
    arg: Option<&str>,
    cwd: &Path,
    reg: &Registry,
    state: &State,
) -> Result<Option<PathBuf>, ProjectError> {
    if let Some(a) = arg {
        return resolve_arg(a, reg).map(Some);
    }
    if let Some(root) = repo_root_of(cwd) {
        return Ok(Some(root));
    }
    if let Some(last) = state.last_path().filter(|p| p.is_dir()) {
        return Ok(Some(last));
    }
    Ok(reg
        .ordered()
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.path.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::paths::canonical;
    use crate::projects::testutil::{temp_dir, temp_repo};

    #[test]
    fn explicit_arg_wins_and_unknown_is_an_error() {
        let repo = temp_repo("resolve-arg");
        let mut reg = Registry::default();
        reg.add(&repo, Some("demo")).unwrap();
        let got = startup(Some("demo"), Path::new("/"), &reg, &State::default()).unwrap();
        assert_eq!(got.map(|p| canonical(&p)), Some(canonical(&repo)));
        assert!(startup(Some("nope-xyz"), Path::new("/"), &reg, &State::default()).is_err());
        let plain = temp_dir("resolve-plain");
        assert!(matches!(
            resolve_arg(plain.to_str().unwrap(), &reg),
            Err(ProjectError::NotARepo(_))
        ));
    }

    #[test]
    fn cwd_repo_then_last_then_pinned() {
        let repo = temp_repo("resolve-cwd");
        let other = temp_repo("resolve-last");
        let outside = temp_dir("resolve-outside");
        let reg = Registry::default();

        let got = startup(None, &repo, &reg, &State::default()).unwrap();
        assert_eq!(got.map(|p| canonical(&p)), Some(canonical(&repo)));

        let mut st = State::default();
        st.touch(&other, 1);
        let got = startup(None, &outside, &reg, &st).unwrap();
        assert_eq!(got.map(|p| canonical(&p)), Some(canonical(&other)));

        let mut reg = Registry::default();
        reg.add(&repo, Some("pinned")).unwrap();
        reg.set_pinned("pinned", true).unwrap();
        let got = startup(None, &outside, &reg, &State::default()).unwrap();
        assert_eq!(got.map(|p| canonical(&p)), Some(canonical(&repo)));

        let none = startup(None, &outside, &Registry::default(), &State::default()).unwrap();
        assert_eq!(none, None);
    }
}
