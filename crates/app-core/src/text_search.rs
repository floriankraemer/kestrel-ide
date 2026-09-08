//! Locale-aware substring matching for UI filter boxes (issue #233's
//! Settings search first, reusable by any future filter).

// ponytail: Unicode case folding (`str::to_lowercase`), not diacritic
// stripping or full ICU locale collation/tailoring — "café" won't match
// "cafe". Upgrade path: fold through `unicode-normalization`'s NFD + strip
// combining marks if a language needs that too.

/// Does `haystack` contain `query`, ignoring case? An empty `query` matches
/// everything.
pub fn matches_query(haystack: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches_query("Editor Font Family", ""));
        assert!(matches_query("", ""));
    }

    #[test]
    fn matches_case_insensitively() {
        assert!(matches_query("Editor Font Family", "font family"));
        assert!(matches_query("Editor Font Family", "FONT"));
    }

    #[test]
    fn matches_substring_within_longer_label() {
        assert!(matches_query("Show whitespace characters", "whitespace"));
    }

    #[test]
    fn no_match_on_unrelated_text() {
        assert!(!matches_query("Editor Font Family", "terminal shell"));
    }

    #[test]
    fn no_match_when_letters_are_out_of_order() {
        // A fuzzy/subsequence matcher would wrongly accept this ("s" and "h"
        // and "e" ... appear in order somewhere); a filter box needs literal
        // substring matching so hits are explainable to whoever typed them.
        assert!(!matches_query("Reads the project", "shell"));
    }
}
