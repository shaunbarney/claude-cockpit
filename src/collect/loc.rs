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

/// Files tracked by git in the current worktree (`git ls-files`), as absolute
/// paths. This is the accurate basis for repo LOC: it honours `.gitignore`
/// transitively and — crucially — excludes nested worktree checkouts and any
/// untracked build output that a plain directory walk would double-count.
/// Empty when `root` isn't a git repo (callers fall back to a directory walk).
fn git_tracked_files(root: &str) -> Vec<std::path::PathBuf> {
    let out = crate::collect::git::git(&["-C", root, "ls-files", "-z"]);
    let base = std::path::Path::new(root);
    out.split('\0')
        .filter(|s| !s.is_empty())
        .map(|rel| base.join(rel))
        .collect()
}

/// Scan `root` and return per-language rows sorted by code lines (desc).
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
