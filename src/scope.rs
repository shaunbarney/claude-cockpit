//! Scope toggles for the Jobs and Docker widgets: this project only, or
//! everything on the machine.
//!
//! The scopes themselves live in [`crate::refresh`] (the collectors read them);
//! this module is the UI-facing side — flip a scope, ask for a fresh tick, and
//! name the current scope in a widget title.

use std::sync::{Arc, Mutex};

use crate::app::DashboardData;
use crate::collect::docker::Scope;
use crate::refresh;

/// Show every project's jobs (`true`) or only this project's.
pub fn set_jobs(all: bool) {
    refresh::set_jobs_all(all);
    refresh::request_now();
}

/// Show every container (`true`) or only this project's. The Docker list is
/// republished from the last snapshot at once; the tick refreshes it after.
pub fn set_docker(data: &Arc<Mutex<DashboardData>>, all: bool) {
    let scope = if all { Scope::All } else { Scope::Repo };
    refresh::set_docker_scope(data, scope);
    refresh::request_now();
}

/// Whether the Docker widget currently shows every container.
pub fn docker_all() -> bool {
    matches!(refresh::docker_scope(), Scope::All)
}

/// Flip the Jobs scope; returns the new "all projects" state.
pub fn toggle_jobs() -> bool {
    let all = !refresh::jobs_all();
    set_jobs(all);
    all
}

/// Flip the Docker scope; returns the new "all containers" state.
pub fn toggle_docker(data: &Arc<Mutex<DashboardData>>) -> bool {
    let all = !docker_all();
    set_docker(data, all);
    all
}

/// Jobs widget title for a scope.
pub fn jobs_title(all: bool) -> &'static str {
    if all {
        " Jobs · all projects "
    } else {
        " Jobs "
    }
}

/// Docker widget title for a scope.
pub fn docker_title(all: bool) -> &'static str {
    if all {
        " Docker · all "
    } else {
        " Docker "
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_name_the_wide_scope_only() {
        assert_eq!(jobs_title(false), " Jobs ");
        assert_eq!(jobs_title(true), " Jobs · all projects ");
        assert_eq!(docker_title(false), " Docker ");
        assert_eq!(docker_title(true), " Docker · all ");
    }

    // The scopes are process-wide, so both toggles share one test to keep
    // parallel tests from observing a half-flipped state.
    #[test]
    fn toggles_flip_and_restore_scopes() {
        let data = Arc::new(Mutex::new(DashboardData::default()));
        let jobs_before = refresh::jobs_all();
        assert_eq!(toggle_jobs(), !jobs_before);
        assert_eq!(refresh::jobs_all(), !jobs_before);
        assert_eq!(toggle_jobs(), jobs_before);

        let docker_before = docker_all();
        assert_eq!(toggle_docker(&data), !docker_before);
        assert_eq!(docker_all(), !docker_before);
        set_docker(&data, docker_before);
        assert_eq!(docker_all(), docker_before);
    }
}
