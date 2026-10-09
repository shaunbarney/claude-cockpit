//! The project the cockpit is currently showing, shared with the refresh threads.
//!
//! Collectors read the root at the top of every tick rather than capturing it
//! once, so a project switch takes effect on the next pass. The generation
//! counter lets a sleeping thread notice a switch and wake early instead of
//! showing the old project for up to ten seconds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

#[derive(Debug, Default)]
pub struct ActiveProject {
    inner: RwLock<(String, String)>,
    generation: AtomicU64,
}

impl ActiveProject {
    /// `root` is the repository path (empty when none is open); `name` the
    /// registered alias, or empty to fall back to the folder name.
    pub fn new(root: &str, name: &str) -> Self {
        ActiveProject {
            inner: RwLock::new((root.to_string(), name.to_string())),
            generation: AtomicU64::new(0),
        }
    }

    pub fn root(&self) -> String {
        self.read().0
    }

    /// Display name: the registered alias, else the root's folder name.
    pub fn name(&self) -> String {
        let (root, name) = self.read();
        if !name.is_empty() {
            return name;
        }
        std::path::Path::new(&root)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Point the cockpit at another project.
    pub fn set(&self, root: &str, name: &str) {
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) =
            (root.to_string(), name.to_string());
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn read(&self) -> (String, String) {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_switches_root_and_bumps_generation() {
        let a = ActiveProject::new("/w/alpha", "");
        assert_eq!(a.name(), "alpha");
        let g = a.generation();
        a.set("/w/beta", "b");
        assert_eq!(a.root(), "/w/beta");
        assert_eq!(a.name(), "b");
        assert_eq!(a.generation(), g + 1);
    }
}
