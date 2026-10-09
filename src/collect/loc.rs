//! Lines-of-code counting via the `tokei` crate (replaces the `cloc` subprocess).

/// One language row. `lines` is total physical lines (code + comments +
/// blanks) — what `wc -l` / an editor reports; `code` is tokei's code-only
/// count, which excludes comments and blank lines.
#[derive(Debug, Clone, PartialEq)]
pub struct LocRow {
    pub language: String,
    pub files: usize,
    pub lines: usize,
    pub code: usize,
}

/// Repo totals across all counted languages.
#[derive(Debug, Clone, PartialEq)]
pub struct LocTotals {
    pub files: usize,
    pub lines: usize,
    pub code: usize,
}

/// Sum rows into totals (pure; the tokei scan itself is integration-tested).
pub fn totals(rows: &[LocRow]) -> LocTotals {
    LocTotals {
        files: rows.iter().map(|r| r.files).sum(),
        lines: rows.iter().map(|r| r.lines).sum(),
        code: rows.iter().map(|r| r.code).sum(),
    }
}

use tokei::{Config, Languages};

/// Parse `git ls-files -s -z` into repo-relative file paths: submodule
/// gitlinks (mode 160000) are dropped, and a conflicted file listed once per
/// stage appears only once.
pub fn parse_ls_files_stage(out: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    out.split('\0')
        .filter_map(|rec| {
            let (meta, path) = rec.split_once('\t')?;
            let mode = meta.split_whitespace().next()?;
            (mode != "160000" && !path.is_empty()).then(|| path.to_string())
        })
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Files tracked by git in the current worktree, as absolute paths. This is
/// the accurate basis for repo LOC: it honours `.gitignore` transitively and —
/// crucially — excludes nested worktree checkouts, submodules, and any
/// untracked build output that a plain directory walk would double-count.
/// Empty when `root` isn't a git repo (callers fall back to a directory walk).
fn git_tracked_files(root: &str) -> Vec<std::path::PathBuf> {
    let out = crate::collect::git::git(&["-C", root, "ls-files", "-s", "-z"]);
    let base = std::path::Path::new(root);
    parse_ls_files_stage(&out)
        .into_iter()
        .map(|rel| base.join(rel))
        .filter(|p| !p.is_dir())
        .collect()
}

/// Scan `root` and return per-language rows sorted by total lines (desc).
/// Counts only git-tracked files so the totals match what's actually in the
/// repo (gitignored paths, build artifacts, and sibling worktrees are excluded).
pub fn loc_rows(root: &str) -> Vec<LocRow> {
    let mut languages = Languages::new();
    let config = Config::default();
    let tracked = git_tracked_files(root);
    if tracked.is_empty() {
        // Not a git repo (or empty) — fall back to a gitignore-aware walk.
        languages.get_statistics(&[root], &[], &config);
    } else {
        languages.get_statistics(&tracked, &[], &config);
    }

    let mut rows: Vec<LocRow> = languages
        .iter()
        .map(|(lang_type, lang)| LocRow {
            language: lang_type.name().to_string(),
            files: lang.reports.len(),
            lines: lang.code + lang.comments + lang.blanks,
            code: lang.code,
        })
        .filter(|r| r.lines > 0)
        .collect();
    rows.sort_by_key(|b| std::cmp::Reverse(b.lines));
    rows
}

/// Languages that are data rather than source (benchmark JSON, CSV dumps,
/// SVG art, text fixtures). They can dwarf the real code in a headline total.
pub fn is_data_language(name: &str) -> bool {
    matches!(name, "JSON" | "CSV" | "TSV" | "SVG" | "Plain Text" | "XML")
}

/// Totals over source languages only (rows where [`is_data_language`] is false).
pub fn code_totals(rows: &[LocRow]) -> LocTotals {
    let code: Vec<LocRow> = rows
        .iter()
        .filter(|r| !is_data_language(&r.language))
        .cloned()
        .collect();
    totals(&code)
}

/// Memo for [`loc_rows_cached`]: the last scan and the repo state it saw.
#[derive(Debug, Default)]
pub struct LocCache {
    key: Option<(String, Option<std::time::SystemTime>)>,
    rows: Vec<LocRow>,
}

/// Cache key: HEAD sha + mtime of the index (which changes on add/commit/
/// checkout). Unstaged edits to tracked files don't move it — by design, to
/// keep tokei off the hot path.
fn loc_cache_key(root: &str) -> (String, Option<std::time::SystemTime>) {
    let git = crate::collect::git::git;
    let head = git(&["-C", root, "rev-parse", "HEAD"]).trim().to_string();
    let rel = git(&["-C", root, "rev-parse", "--git-path", "index"]);
    let idx = std::path::Path::new(root).join(rel.trim());
    let mtime = std::fs::metadata(idx).and_then(|m| m.modified()).ok();
    (head, mtime)
}

/// [`loc_rows`], re-scanning only when HEAD or the index changed since the
/// last call with this cache.
pub fn loc_rows_cached(root: &str, cache: &mut LocCache) -> Vec<LocRow> {
    let key = loc_cache_key(root);
    if cache.key.as_ref() != Some(&key) {
        cache.rows = loc_rows(root);
        cache.key = Some(key);
    }
    cache.rows.clone()
}

use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Attribute, Cell, CellAlignment, Color,
    ContentArrangement, Table,
};

use crate::render::VIOLET;

/// Build the "Repo Code · git-tracked" table from tokei rows.
pub fn loc_table(rows: &[LocRow]) -> Table {
    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    t.set_header(vec![
        Cell::new("Language").add_attribute(Attribute::Bold),
        Cell::new("Files")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Lines")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
    ]);

    for r in rows {
        t.add_row(vec![
            Cell::new(&r.language).add_attribute(Attribute::Bold),
            Cell::new(crate::util::thousands(r.files as u64))
                .fg(Color::DarkGrey)
                .set_alignment(CellAlignment::Right),
            Cell::new(crate::util::thousands(r.lines as u64))
                .fg(Color::Cyan)
                .set_alignment(CellAlignment::Right),
        ]);
    }

    let s = totals(rows);
    t.add_row(vec![
        Cell::new("TOTAL").add_attribute(Attribute::Bold),
        Cell::new(crate::util::thousands(s.files as u64))
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(crate::util::thousands(s.lines as u64))
            .fg(VIOLET)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
    ]);
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::git::test_repo::*;

    #[test]
    fn ls_files_stage_drops_gitlinks_and_dupes() {
        let out = "100644 aaa 0\tsrc/a.rs\x00160000 bbb 0\tvendor/sub\0\
                   100644 c1 1\tconf.rs\x00100644 c2 2\tconf.rs\x00100644 c3 3\tconf.rs\0";
        assert_eq!(parse_ls_files_stage(out), vec!["src/a.rs", "conf.rs"]);
    }

    #[test]
    fn data_languages_excluded_from_code_totals() {
        assert!(is_data_language("JSON") && is_data_language("Plain Text"));
        assert!(!is_data_language("Rust"));
        let row = |l: &str, n: usize| LocRow {
            language: l.into(),
            files: 1,
            lines: n,
            code: n,
        };
        let rows = vec![row("JSON", 1000), row("Rust", 10)];
        assert_eq!(code_totals(&rows).lines, 10);
        assert_eq!(totals(&rows).lines, 1010);
    }

    #[test]
    fn real_repo_counts_conflicted_once_and_skips_submodule() {
        let r = init("loc", "main");
        write(r.path(), "m.rs", "fn a() {}\n");
        commit_all(r.path(), "rs");
        let sub = init("loc-sub", "main");
        run(
            r.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &sub.s(),
                "sub",
            ],
        );
        commit_all(r.path(), "sub");
        run(r.path(), &["checkout", "-qb", "other"]);
        write(r.path(), "m.rs", "fn b() {}\n");
        commit_all(r.path(), "b");
        run(r.path(), &["checkout", "-q", "main"]);
        write(r.path(), "m.rs", "fn c() {}\n");
        commit_all(r.path(), "c");
        let _ = std::process::Command::new("git")
            .args(["-C", &r.s(), "-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["merge", "-q", "other"])
            .output();
        let files = git_tracked_files(&r.s());
        let rels: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(r.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(rels.iter().filter(|p| *p == "m.rs").count(), 1, "{rels:?}");
        assert!(!rels.iter().any(|p| p == "sub" || p.starts_with("sub/")));

        let mut cache = LocCache::default();
        let first = loc_rows_cached(&r.s(), &mut cache);
        let key = cache.key.clone();
        assert_eq!(loc_rows_cached(&r.s(), &mut cache), first);
        assert_eq!(cache.key, key);
    }

    #[test]
    fn sums_files_lines_and_code() {
        let rows = vec![
            LocRow {
                language: "Rust".into(),
                files: 3,
                lines: 130,
                code: 100,
            },
            LocRow {
                language: "Python".into(),
                files: 2,
                lines: 55,
                code: 40,
            },
        ];
        assert_eq!(
            totals(&rows),
            LocTotals {
                files: 5,
                lines: 185,
                code: 140
            }
        );
    }
}
