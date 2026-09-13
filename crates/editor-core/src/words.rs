//! Document-word completion fallback (R2): every identifier-shaped word
//! already in a buffer, for when there is no language server to ask —
//! IntelliJ/VS Code's own last resort, and cheap enough to run on every
//! keystroke since it is one linear scan with no parser involved.

use std::collections::BTreeSet;

/// Every distinct run of identifier characters (`is_alphanumeric` or `_`,
/// not starting with a digit) in `text`, sorted and deduplicated — order
/// beyond that is the caller's business (`lsp_core::completion::filter`
/// ranks and orders the final list).
pub fn words_in(text: &str) -> Vec<String> {
    let mut words = BTreeSet::new();
    let mut current = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
            continue;
        }
        if !current.is_empty() {
            if current.chars().next().is_some_and(|c| !c.is_ascii_digit()) {
                words.insert(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    words.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_shaped_runs_are_collected() {
        assert_eq!(
            words_in("let foo_bar = fooBar2 + 1;"),
            ["fooBar2", "foo_bar", "let"],
            "a bare number is not an identifier"
        );
    }

    #[test]
    fn a_word_starting_with_a_digit_is_not_an_identifier() {
        assert_eq!(words_in("2fast"), Vec::<String>::new());
    }

    #[test]
    fn duplicates_collapse_to_one_entry() {
        assert_eq!(words_in("foo foo foo"), ["foo"]);
    }

    #[test]
    fn empty_text_has_no_words() {
        assert!(words_in("").is_empty());
    }
}
