//! The project picker: state and key handling, with no rendering and no
//! terminal types beyond the key code.
//!
//! The picker owns an in-memory copy of the registry and state. Every edit is
//! applied there and written straight back, so the picker never shows a list
//! that disagrees with the file. Rows (git status, jobs, activity) arrive
//! separately from the background overview gather and are matched by path.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::model::Registry;
use super::overview::ProjectRow;
use super::paths::{canonical, contract_tilde, expand_tilde, repo_root_of};
use super::store::{save_registry_to, save_state_to, State};
use crate::fuzzy::Fuzzy;

/// What the picker is collecting text for, if anything.
#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    /// Live filter over the rows.
    Filter,
    /// Path of a repository to register.
    AddPath,
    /// New alias for the selected project.
    Rename,
    /// `+tag -tag` edits for the selected project.
    Tags,
    /// Waiting for `y` to unregister the named project.
    ConfirmRemove(String),
}

/// What the host should do after a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    None,
    Close,
    /// Open this project (repository root).
    Switch(PathBuf),
    /// The registry or state changed; persist state and refresh rows.
    Changed,
}

/// Where edits are written. Injected so tests never touch the real config.
#[derive(Debug, Clone)]
pub struct Files {
    pub registry: PathBuf,
    pub state: PathBuf,
}

pub struct Picker {
    pub registry: Registry,
    pub state: State,
    pub files: Files,
    /// All rows from the last overview gather.
    pub rows: Vec<ProjectRow>,
    /// Indices into `rows`, filtered and in display order.
    pub visible: Vec<usize>,
    pub selected: usize,
    pub prompt: Option<Prompt>,
    pub input: String,
    pub filter: String,
    /// One-line feedback ("added sovra", errors).
    pub message: Option<(String, bool)>,
    /// Root of the project currently on screen.
    pub current: Option<PathBuf>,
}

impl Picker {
    pub fn new(registry: Registry, state: State, files: Files, current: Option<PathBuf>) -> Self {
        let mut p = Picker {
            registry,
            state,
            files,
            rows: Vec::new(),
            visible: Vec::new(),
            selected: 0,
            prompt: None,
            input: String::new(),
            filter: String::new(),
            message: None,
            current: current.map(|c| canonical(&c)),
        };
        p.rows = p.skeleton_rows();
        p.refilter(&mut Fuzzy::default());
        p
    }

    /// Registered projects as bare rows, shown until the first gather lands so
    /// the picker is never empty on open.
    fn skeleton_rows(&self) -> Vec<ProjectRow> {
        self.registry
            .ordered()
            .into_iter()
            .map(|p| ProjectRow {
                name: p.name.clone(),
                path: p.path.clone(),
                registered: true,
                pinned: p.pinned,
                tags: p.tags.clone(),
                exists: p.exists(),
                ..ProjectRow::default()
            })
            .collect()
    }

    /// Take fresh rows from the background gather, keeping the selection on
    /// the same project.
    pub fn set_rows(&mut self, rows: Vec<ProjectRow>, fz: &mut Fuzzy) {
        let keep = self.selected_row().map(|r| canonical(&r.path));
        self.rows = rows;
        self.refilter(fz);
        if let Some(path) = keep {
            if let Some(pos) = self
                .visible
                .iter()
                .position(|&i| canonical(&self.rows[i].path) == path)
            {
                self.selected = pos;
            }
        }
    }

    /// Rows that reflect the in-memory registry: registered flags, names and
    /// pins follow edits immediately, before the next gather.
    fn reconcile(&mut self) {
        let mut rows = std::mem::take(&mut self.rows);
        for row in rows.iter_mut() {
            match self
                .registry
                .projects
                .iter()
                .find(|p| canonical(&p.path) == canonical(&row.path))
            {
                Some(p) => {
                    row.registered = true;
                    row.name = p.name.clone();
                    row.pinned = p.pinned;
                    row.tags = p.tags.clone();
                }
                None => {
                    row.registered = false;
                    row.pinned = false;
                }
            }
        }
        for p in &self.registry.projects {
            if !rows
                .iter()
                .any(|r| canonical(&r.path) == canonical(&p.path))
            {
                rows.push(ProjectRow {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    registered: true,
                    pinned: p.pinned,
                    exists: p.exists(),
                    ..ProjectRow::default()
                });
            }
        }
        rows.retain(|r| r.registered || !self.state.is_hidden(&r.path));
        self.rows = rows;
    }

    /// Registered (pinned first, then by name) above suggestions (most
    /// recently active first), narrowed by the filter.
    pub fn refilter(&mut self, fz: &mut Fuzzy) {
        let mut idx: Vec<usize> = (0..self.rows.len())
            .filter(|&i| {
                let r = &self.rows[i];
                let hay = format!(
                    "{} {} {}",
                    r.name,
                    contract_tilde(&r.path),
                    r.tags.join(" ")
                );
                fz.score(&self.filter, &hay).is_some()
            })
            .collect();
        idx.sort_by(|&a, &b| {
            let (x, y) = (&self.rows[a], &self.rows[b]);
            y.registered
                .cmp(&x.registered)
                .then(y.pinned.cmp(&x.pinned))
                .then_with(|| match x.registered {
                    true => x.name.cmp(&y.name),
                    false => y.last_active.cmp(&x.last_active),
                })
        });
        self.visible = idx;
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    pub fn selected_row(&self) -> Option<&ProjectRow> {
        self.visible.get(self.selected).map(|&i| &self.rows[i])
    }

    /// Pinned projects in hotkey order (`1` is the first).
    pub fn pinned_paths(&self) -> Vec<PathBuf> {
        self.registry
            .ordered()
            .into_iter()
            .filter(|p| p.pinned)
            .map(|p| p.path.clone())
            .collect()
    }

    fn say(&mut self, msg: impl Into<String>, error: bool) {
        self.message = Some((msg.into(), error));
    }

    fn save(&mut self) -> Outcome {
        let saved = save_registry_to(&self.files.registry, &self.registry)
            .and_then(|()| save_state_to(&self.files.state, &self.state));
        match saved {
            Ok(()) => Outcome::Changed,
            Err(e) => {
                self.say(e.to_string(), true);
                Outcome::None
            }
        }
    }

    /// Handle one key press.
    pub fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers, fz: &mut Fuzzy) -> Outcome {
        if let Some(prompt) = self.prompt.clone() {
            return self.handle_prompt(prompt, code, fz);
        }
        self.message = None;
        let out = match code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Close,
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Home | KeyCode::Char('g') => self.jump(0),
            KeyCode::End | KeyCode::Char('G') => self.jump(usize::MAX),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char(c @ '1'..='9') => self.open_pinned(c as usize - '1' as usize),
            KeyCode::Char('/') => self.begin(Prompt::Filter, self.filter.clone()),
            KeyCode::Char('n') => self.begin(Prompt::AddPath, "~/".into()),
            KeyCode::Char('a') => self.add_selected(),
            KeyCode::Char('d') | KeyCode::Delete => self.remove_selected(),
            KeyCode::Char('p') => self.toggle_pin(),
            KeyCode::Char('r') => self.begin_on_registered(Prompt::Rename, |r| r.name.clone()),
            KeyCode::Char('t') => self.begin_on_registered(Prompt::Tags, |_| String::new()),
            _ => Outcome::None,
        };
        if out == Outcome::Changed {
            self.reconcile();
            self.refilter(fz);
        }
        out
    }

    fn step(&mut self, d: isize) -> Outcome {
        let n = self.visible.len();
        if n > 0 {
            self.selected = (self.selected as isize + d).rem_euclid(n as isize) as usize;
        }
        Outcome::None
    }

    fn jump(&mut self, to: usize) -> Outcome {
        self.selected = to.min(self.visible.len().saturating_sub(1));
        Outcome::None
    }

    fn begin(&mut self, p: Prompt, initial: String) -> Outcome {
        self.prompt = Some(p);
        self.input = initial;
        Outcome::None
    }

    fn begin_on_registered(&mut self, p: Prompt, init: impl Fn(&ProjectRow) -> String) -> Outcome {
        match self.selected_row() {
            Some(r) if r.registered => {
                let s = init(r);
                self.begin(p, s)
            }
            Some(_) => {
                self.say("add it first (a)", true);
                Outcome::None
            }
            None => Outcome::None,
        }
    }

    fn open_selected(&mut self) -> Outcome {
        match self.selected_row() {
            Some(r) if !r.exists => {
                self.say(
                    format!("{} no longer exists — d to remove", contract_tilde(&r.path)),
                    true,
                );
                Outcome::None
            }
            Some(r) => Outcome::Switch(r.path.clone()),
            None => Outcome::None,
        }
    }

    fn open_pinned(&mut self, i: usize) -> Outcome {
        match self.pinned_paths().get(i) {
            Some(p) => Outcome::Switch(p.clone()),
            None => Outcome::None,
        }
    }

    fn add_selected(&mut self) -> Outcome {
        let Some(row) = self.selected_row().cloned() else {
            return Outcome::None;
        };
        if row.registered {
            self.say(format!("{} is already a project", row.name), false);
            return Outcome::None;
        }
        self.add_path(&row.path, Some(row.name))
    }

    fn add_path(&mut self, dir: &Path, name: Option<String>) -> Outcome {
        let Some(root) = repo_root_of(dir) else {
            self.say(
                format!("{} is not inside a git repository", contract_tilde(dir)),
                true,
            );
            return Outcome::None;
        };
        let name = self.registry.unique_name(&name.unwrap_or_else(|| {
            super::model::slug(
                &root
                    .file_name()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
            )
        }));
        match self.registry.add(&root, Some(&name)) {
            Ok(_) => {
                self.state.unhide(&root);
                self.say(format!("added {name}"), false);
                let out = self.save();
                if out == Outcome::Changed {
                    self.select_path(&root);
                }
                out
            }
            Err(e) => {
                self.say(e.to_string(), true);
                Outcome::None
            }
        }
    }

    fn select_path(&mut self, root: &Path) {
        let root = canonical(root);
        if !self.rows.iter().any(|r| canonical(&r.path) == root) {
            self.rows.push(ProjectRow {
                path: root.clone(),
                exists: true,
                ..ProjectRow::default()
            });
        }
        self.reconcile();
        self.refilter(&mut Fuzzy::default());
        if let Some(pos) = self
            .visible
            .iter()
            .position(|&i| canonical(&self.rows[i].path) == root)
        {
            self.selected = pos;
        }
    }

    fn remove_selected(&mut self) -> Outcome {
        let Some(row) = self.selected_row().cloned() else {
            return Outcome::None;
        };
        if row.registered {
            self.prompt = Some(Prompt::ConfirmRemove(row.name));
            return Outcome::None;
        }
        self.state.hide(&row.path);
        self.say(
            format!("hid {} from suggestions", contract_tilde(&row.path)),
            false,
        );
        self.save()
    }

    fn toggle_pin(&mut self) -> Outcome {
        let Some(row) = self.selected_row().cloned() else {
            return Outcome::None;
        };
        if !row.registered {
            self.say("add it first (a)", true);
            return Outcome::None;
        }
        if let Err(e) = self.registry.set_pinned(&row.name, !row.pinned) {
            self.say(e.to_string(), true);
            return Outcome::None;
        }
        self.save()
    }

    fn handle_prompt(&mut self, prompt: Prompt, code: KeyCode, fz: &mut Fuzzy) -> Outcome {
        if let Prompt::ConfirmRemove(name) = prompt {
            self.prompt = None;
            if !matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                self.say("kept", false);
                return Outcome::None;
            }
            let out = match self.registry.remove(&name) {
                Ok(_) => {
                    self.say(
                        format!("removed {name} (the repository is untouched)"),
                        false,
                    );
                    self.save()
                }
                Err(e) => {
                    self.say(e.to_string(), true);
                    Outcome::None
                }
            };
            self.reconcile();
            self.refilter(fz);
            return out;
        }
        match code {
            KeyCode::Esc => {
                if prompt == Prompt::Filter {
                    self.filter.clear();
                    self.refilter(fz);
                }
                self.prompt = None;
                Outcome::None
            }
            KeyCode::Enter => {
                self.prompt = None;
                let text = std::mem::take(&mut self.input);
                let out = self.submit(prompt, text);
                if out == Outcome::Changed {
                    self.reconcile();
                }
                self.refilter(fz);
                out
            }
            KeyCode::Tab if prompt == Prompt::AddPath => {
                self.input = complete_dir(&self.input);
                Outcome::None
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.live_filter(&prompt, fz);
                Outcome::None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                self.live_filter(&prompt, fz);
                Outcome::None
            }
            _ => Outcome::None,
        }
    }

    fn live_filter(&mut self, prompt: &Prompt, fz: &mut Fuzzy) {
        if *prompt == Prompt::Filter {
            self.filter = self.input.clone();
            self.selected = 0;
            self.refilter(fz);
        }
    }

    fn submit(&mut self, prompt: Prompt, text: String) -> Outcome {
        let Some(row) = self.selected_row().cloned() else {
            return match prompt {
                Prompt::AddPath => self.add_path(&expand_tilde(text.trim()), None),
                _ => Outcome::None,
            };
        };
        let result = match prompt {
            Prompt::Filter => return Outcome::None,
            Prompt::AddPath => return self.add_path(&expand_tilde(text.trim()), None),
            Prompt::Rename => self.registry.rename(&row.name, text.trim()),
            Prompt::Tags => {
                let edits: Vec<String> = text.split_whitespace().map(str::to_string).collect();
                self.registry.edit_tags(&row.name, &edits)
            }
            Prompt::ConfirmRemove(_) => unreachable!("handled before"),
        };
        match result {
            Ok(()) => self.save(),
            Err(e) => {
                self.say(e.to_string(), true);
                Outcome::None
            }
        }
    }
}

/// Complete the last path segment against existing directories: a unique match
/// completes fully (with a trailing `/`), several matches extend to their
/// common prefix.
pub fn complete_dir(input: &str) -> String {
    let (dir_part, stem) = match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input),
    };
    let dir = expand_tilde(if dir_part.is_empty() { "." } else { dir_part });
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return input.to_string();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(stem) && (stem.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    match names.as_slice() {
        [] => input.to_string(),
        [one] => format!("{dir_part}{one}/"),
        many => {
            let first = &many[0];
            let common = many.iter().fold(first.len(), |n, s| {
                first
                    .chars()
                    .zip(s.chars())
                    .take(n)
                    .take_while(|(a, b)| a == b)
                    .count()
            });
            format!(
                "{dir_part}{}",
                &first[..first
                    .char_indices()
                    .nth(common)
                    .map_or(first.len(), |(i, _)| i)]
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::testutil::{temp_dir, temp_repo};

    fn picker() -> (Picker, PathBuf, PathBuf) {
        let a = temp_repo("pick-a");
        let b = temp_repo("pick-b");
        let cfg = temp_dir("pick-cfg");
        let mut reg = Registry::default();
        reg.add(&a, Some("alpha")).unwrap();
        let files = Files {
            registry: cfg.join("projects.toml"),
            state: cfg.join("state.toml"),
        };
        let mut p = Picker::new(reg, State::default(), files, None);
        let mut rows = p.rows.clone();
        rows.push(ProjectRow {
            name: "bravo".into(),
            path: b.clone(),
            exists: true,
            last_active: Some(5),
            ..Default::default()
        });
        p.set_rows(rows, &mut Fuzzy::default());
        (p, a, b)
    }

    fn key(p: &mut Picker, c: KeyCode) -> Outcome {
        p.handle_key(c, KeyModifiers::NONE, &mut Fuzzy::default())
    }

    fn typed(p: &mut Picker, s: &str) {
        for c in s.chars() {
            key(p, KeyCode::Char(c));
        }
    }

    #[test]
    fn registered_rows_come_before_suggestions_and_enter_switches() {
        let (mut p, a, b) = picker();
        assert_eq!(p.selected_row().unwrap().name, "alpha");
        assert_eq!(key(&mut p, KeyCode::Enter), Outcome::Switch(a.clone()));
        key(&mut p, KeyCode::Char('j'));
        assert_eq!(key(&mut p, KeyCode::Enter), Outcome::Switch(b));
    }

    #[test]
    fn add_suggestion_persists_and_keeps_selection() {
        let (mut p, _, b) = picker();
        key(&mut p, KeyCode::Char('j'));
        assert_eq!(key(&mut p, KeyCode::Char('a')), Outcome::Changed);
        let row = p.selected_row().unwrap();
        assert!(row.registered);
        assert_eq!(canonical(&row.path), canonical(&b));
        let saved = crate::projects::store::load_registry_from(&p.files.registry).unwrap();
        assert!(saved.find("bravo").is_some());
    }

    #[test]
    fn remove_asks_first_and_hide_is_immediate() {
        let (mut p, _, _) = picker();
        key(&mut p, KeyCode::Char('d'));
        assert_eq!(p.prompt, Some(Prompt::ConfirmRemove("alpha".into())));
        assert_eq!(key(&mut p, KeyCode::Char('n')), Outcome::None);
        assert!(p.registry.find("alpha").is_some());
        key(&mut p, KeyCode::Char('d'));
        assert_eq!(key(&mut p, KeyCode::Char('y')), Outcome::Changed);
        assert!(p.registry.find("alpha").is_none());

        // Removed project falls back to a suggestion row; `d` there hides it.
        let before = p.visible.len();
        key(&mut p, KeyCode::Char('d'));
        assert_eq!(p.visible.len(), before - 1);
    }

    #[test]
    fn rename_pin_and_hotkeys() {
        let (mut p, a, _) = picker();
        key(&mut p, KeyCode::Char('r'));
        for _ in 0..5 {
            key(&mut p, KeyCode::Backspace);
        }
        typed(&mut p, "core");
        assert_eq!(key(&mut p, KeyCode::Enter), Outcome::Changed);
        assert_eq!(p.selected_row().unwrap().name, "core");
        assert_eq!(key(&mut p, KeyCode::Char('1')), Outcome::None); // nothing pinned yet
        assert_eq!(key(&mut p, KeyCode::Char('p')), Outcome::Changed);
        assert_eq!(key(&mut p, KeyCode::Char('1')), Outcome::Switch(a));
    }

    #[test]
    fn filter_narrows_and_esc_restores() {
        let (mut p, _, _) = picker();
        key(&mut p, KeyCode::Char('/'));
        typed(&mut p, "brav");
        assert_eq!(p.visible.len(), 1);
        assert_eq!(p.selected_row().unwrap().name, "bravo");
        key(&mut p, KeyCode::Esc);
        assert_eq!(p.visible.len(), 2);
    }

    #[test]
    fn add_by_path_rejects_non_repos() {
        let (mut p, _, _) = picker();
        let plain = temp_dir("pick-plain");
        key(&mut p, KeyCode::Char('n'));
        p.input = plain.to_string_lossy().into_owned();
        assert_eq!(key(&mut p, KeyCode::Enter), Outcome::None);
        assert!(p.message.as_ref().unwrap().1);

        let repo = temp_repo("pick-new");
        key(&mut p, KeyCode::Char('n'));
        p.input = repo
            .join("f.txt")
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(key(&mut p, KeyCode::Enter), Outcome::Changed);
        assert_eq!(canonical(&p.selected_row().unwrap().path), canonical(&repo));
    }

    #[test]
    fn tab_completes_directories() {
        let d = temp_dir("complete");
        std::fs::create_dir_all(d.join("alpha-one")).unwrap();
        std::fs::create_dir_all(d.join("alpha-two")).unwrap();
        std::fs::create_dir_all(d.join("beta")).unwrap();
        let base = format!("{}/", d.display());
        assert_eq!(complete_dir(&format!("{base}b")), format!("{base}beta/"));
        assert_eq!(complete_dir(&format!("{base}al")), format!("{base}alpha-"));
        assert_eq!(complete_dir(&format!("{base}zz")), format!("{base}zz"));
    }
}
