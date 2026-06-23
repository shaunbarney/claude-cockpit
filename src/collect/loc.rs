//! Lines-of-code counting via the `tokei` crate (replaces the `cloc` subprocess).

/// One language row: display name, file count, code lines.
#[derive(Debug, Clone, PartialEq)]
pub struct LocRow {
    pub language: String,
    pub files: usize,
    pub code: usize,
}

/// Repo totals across all counted languages.
#[derive(Debug, Clone, PartialEq)]
pub struct LocTotals {
    pub files: usize,
    pub code: usize,
}

/// Sum rows into totals (pure; the tokei scan itself is integration-tested).
pub fn totals(rows: &[LocRow]) -> LocTotals {
    LocTotals {
        files: rows.iter().map(|r| r.files).sum(),
        code: rows.iter().map(|r| r.code).sum(),
    }
}

use tokei::{Config, Languages};

/// Scan `root` and return per-language rows sorted by code lines (desc).
/// tokei is gitignore-aware by default, approximating cloc's `--vcs=git`.
pub fn loc_rows(root: &str) -> Vec<LocRow> {
    let mut languages = Languages::new();
    let config = Config::default();
    languages.get_statistics(&[root], &[], &config);

    let mut rows: Vec<LocRow> = languages
        .iter()
        .map(|(lang_type, lang)| LocRow {
            language: lang_type.name().to_string(),
            files: lang.reports.len(),
            code: lang.code,
        })
        .filter(|r| r.code > 0)
        .collect();
    rows.sort_by(|a, b| b.code.cmp(&a.code));
    rows
}

use comfy_table::{
    Attribute, Cell, CellAlignment, Color, ContentArrangement, Table,
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL,
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
        Cell::new("Files").add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
        Cell::new("Code").add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
    ]);

    for r in rows {
        t.add_row(vec![
            Cell::new(&r.language).add_attribute(Attribute::Bold),
            Cell::new(crate::util::thousands(r.files as u64)).fg(Color::DarkGrey).set_alignment(CellAlignment::Right),
            Cell::new(crate::util::thousands(r.code as u64)).fg(Color::Cyan).set_alignment(CellAlignment::Right),
        ]);
    }

    let s = totals(rows);
    t.add_row(vec![
        Cell::new("TOTAL").add_attribute(Attribute::Bold),
        Cell::new(crate::util::thousands(s.files as u64)).add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
        Cell::new(crate::util::thousands(s.code as u64)).fg(VIOLET).add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
    ]);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_files_and_code() {
        let rows = vec![
            LocRow { language: "Rust".into(), files: 3, code: 100 },
            LocRow { language: "Python".into(), files: 2, code: 40 },
        ];
        assert_eq!(totals(&rows), LocTotals { files: 5, code: 140 });
    }
}
