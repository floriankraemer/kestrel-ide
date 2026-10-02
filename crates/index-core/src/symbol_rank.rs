//! Go to Class / Symbol ranking: which definitions a typed query lists
//! first. A private-field `impl TextIndex` block like `search_scope.rs`,
//! kept out of `lib.rs` for the same file-size reason.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Utf32Str};

use crate::{IndexError, SymbolMatch, TextIndex};

impl TextIndex {
    /// Go-to-symbol for search-as-you-type: the same definition set as
    /// [`find_definitions`](Self::find_definitions), but fuzzy-matched and
    /// ranked best-first rather than exact-substring filtered and ordered by
    /// file. An empty query returns the first `limit` definitions.
    pub fn find_definitions_ranked(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SymbolMatch>, IndexError> {
        let mut matches = self.find_definitions("")?;
        if query.is_empty() {
            matches.truncate(limit);
            return Ok(matches);
        }

        let mut matcher = nucleo_matcher::Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, SymbolMatch)> = matches
            .into_iter()
            .filter_map(|m| {
                let score = pattern.score(Utf32Str::new(&m.name, &mut buf), &mut matcher)?;
                Some((score, m))
            })
            .collect();
        scored.sort_by(|a, b| {
            name_match_tier(&a.1.name, query)
                .cmp(&name_match_tier(&b.1.name, query))
                .then_with(|| b.0.cmp(&a.0))
                .then_with(|| a.1.name.len().cmp(&b.1.name.len()))
                .then_with(|| (&a.1.path, a.1.line).cmp(&(&b.1.path, b.1.line)))
        });
        Ok(scored.into_iter().take(limit).map(|(_, m)| m).collect())
    }
}

/// How well `name` matches `query` as typed, best first: an exact name, a
/// prefix, any other substring (a word boundary included), then a loose
/// subsequence. Ranked ahead of the fuzzy score so that `Greeter` lists
/// `GreeterTest` before `GreaterToGreaterOrEqual`, which merely contains
/// the letters in order.
fn name_match_tier(name: &str, query: &str) -> u8 {
    let (name, query) = (name.to_lowercase(), query.to_lowercase());
    if name == query {
        0
    } else if name.starts_with(&query) {
        1
    } else if name.contains(&query) {
        2
    } else {
        3
    }
}
