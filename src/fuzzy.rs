//! Fuzzy matching for the row filter and the command palette.
//!
//! Thin wrapper over `nucleo-matcher` (the matcher behind Helix and nucleo) so
//! the rest of the app never has to think about UTF-32 buffers. Scoring is
//! Smith-Waterman with bonuses for word boundaries and camel-case humps, which
//! ranks `wtr` → `worktree` above `wtr` → `what-error` the way a user expects.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A reusable matcher. Construction allocates, so keep one on `App` rather than
/// building one per keystroke.
pub struct Fuzzy {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for Fuzzy {
    fn default() -> Self {
        Fuzzy {
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Fuzzy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fuzzy")
    }
}

impl Fuzzy {
    /// Score `haystack` against `needle`, or `None` when it does not match.
    ///
    /// An empty needle matches everything with score 0, which lets callers use
    /// the same code path whether or not a filter is active.
    pub fn score(&mut self, needle: &str, haystack: &str) -> Option<u32> {
        if needle.is_empty() {
            return Some(0);
        }
        let pattern = Pattern::parse(needle, CaseMatching::Ignore, Normalization::Smart);
        self.buf.clear();
        let hay = Utf32Str::new(haystack, &mut self.buf);
        pattern.score(hay, &mut self.matcher)
    }

    /// True when `haystack` matches `needle` at all.
    pub fn matches(&mut self, needle: &str, haystack: &str) -> bool {
        self.score(needle, haystack).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_matches_everything() {
        let mut f = Fuzzy::default();
        assert_eq!(f.score("", "anything"), Some(0));
    }

    #[test]
    fn subsequence_matches_and_ranks() {
        let mut f = Fuzzy::default();
        assert!(f.matches("wtr", "worktree"));
        assert!(!f.matches("zzz", "worktree"));
        // An exact prefix outranks a scattered subsequence.
        let tight = f.score("work", "worktree").unwrap();
        let loose = f.score("work", "wildly-odd-rocket-kit").unwrap();
        assert!(tight > loose, "tight={tight} loose={loose}");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let mut f = Fuzzy::default();
        assert!(f.matches("SOVRA", "sovra-backend-dev"));
        assert!(f.matches("sovra", "SOVRA-BACKEND"));
    }
}
