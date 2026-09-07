//! Line equality under "ignore whitespace".
//!
//! A [`Hunk`](super::Hunk)'s line ranges are token *indices*, not content,
//! so swapping the token type's `Eq`/`Hash` for a whitespace-blind one is
//! the entire feature — the diff algorithm and the hunk builder never know.

use std::hash::{Hash, Hasher};

use imara_diff::TokenSource;

/// A line, compared and hashed by its whitespace-collapsed content rather
/// than its exact bytes.
#[derive(Clone, Copy)]
pub(super) struct WsLine<'a>(&'a str);

impl PartialEq for WsLine<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.split_whitespace().eq(other.0.split_whitespace())
    }
}

impl Eq for WsLine<'_> {}

impl Hash for WsLine<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for word in self.0.split_whitespace() {
            word.hash(state);
        }
        // A separator between words so "ab" "c" and "a" "bc" don't collide.
        0u8.hash(state);
    }
}

pub(super) struct WsInsensitiveLines<'a>(imara_diff::sources::Lines<'a>);

pub(super) fn ws_insensitive_lines(text: &str) -> WsInsensitiveLines<'_> {
    WsInsensitiveLines(imara_diff::sources::lines(text))
}

impl<'a> TokenSource for WsInsensitiveLines<'a> {
    type Token = WsLine<'a>;
    type Tokenizer = std::iter::Map<
        <imara_diff::sources::Lines<'a> as TokenSource>::Tokenizer,
        fn(&'a str) -> WsLine<'a>,
    >;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.tokenize().map(WsLine)
    }

    fn estimate_tokens(&self) -> u32 {
        self.0.estimate_tokens()
    }
}

#[cfg(test)]
mod tests {
    use crate::diff::{diff_lines_opts, HunkKind};

    #[test]
    fn ignoring_whitespace_collapses_a_whitespace_only_edit() {
        assert!(
            diff_lines_opts("a\n", "a \n", true).unwrap().is_empty(),
            "a trailing-space-only edit must vanish with ignore_whitespace"
        );
        assert!(
            diff_lines_opts("  a\tb  \n", "a b\n", true)
                .unwrap()
                .is_empty(),
            "reflowed inner whitespace must still compare equal"
        );
    }

    #[test]
    fn ignoring_whitespace_still_reports_a_real_content_change() {
        let hunks = diff_lines_opts("a\n", "a b\n", true).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Modified);
    }
}
