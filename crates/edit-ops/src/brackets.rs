//! Finding a bracket's partner, and where Ctrl+] jumps to.
//!
//! Two paths, in this order:
//!
//! 1. **The grammar.** A bracket is an anonymous token in the tree, so its
//!    partner is the sibling token of the counterpart kind under the same
//!    parent node. This is the path that is right about a `}` inside a
//!    string, an unbalanced file, and a language where brackets nest by
//!    rules a counter does not know.
//! 2. **A depth counter over the text**, for plain text, a file past the
//!    highlight ceiling, and a bracket the tree does not model as a token.
//!    Where a tree exists, the counter still uses it to skip brackets
//!    inside strings and comments.
//!
//! A file with no matching bracket gets `None`. Guessing — jumping to the
//! nearest bracket of the right kind — is worse than not moving.

use std::ops::Range;

use syntax_core::Language;

use crate::syntax::{Syntax, Tokens};

/// A bracket and its partner, both as byte ranges into the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BracketMatch {
    /// The bracket the caret was on or just after.
    pub bracket: Range<usize>,
    /// Its partner.
    pub partner: Range<usize>,
}

/// The bracket at (or immediately before) `offset`, whether or not it has a
/// partner — what a live pair highlight needs and [`matching_bracket`] does
/// not give: a caret on an unmatched closer still names a bracket, it just
/// has nothing to pair it with, and the highlight paints that bracket in the
/// error colour rather than not painting anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairAt {
    /// The bracket the caret was on or just after.
    pub bracket: Range<usize>,
    /// Its partner, when the file has one.
    pub partner: Option<Range<usize>>,
}

/// The bracket at (or immediately before) `offset`, and its partner if it
/// has one.
///
/// The caret is treated as being on the bracket to its right first, then
/// the one to its left — the convention every editor uses, and the one
/// that makes `foo()|` jump backwards.
pub fn pair_at(language: Language, text: &str, offset: usize) -> Option<PairAt> {
    let tokens = Tokens::of(language);
    let (bracket, delimiter, forward) = bracket_at(&tokens, text, offset)?;
    let syntax = Syntax::parse(language, text);
    let partner = by_grammar(&syntax, &bracket, &delimiter, forward)
        .or_else(|| by_counting(&syntax, text, &bracket, &delimiter, forward));
    Some(PairAt { bracket, partner })
}

/// The bracket at (or immediately before) `offset` and its partner — `None`
/// when either there is no bracket there or it has no partner, which is
/// exactly what jumping to it needs (`None` either way refuses the jump).
pub fn matching_bracket(language: Language, text: &str, offset: usize) -> Option<BracketMatch> {
    let found = pair_at(language, text, offset)?;
    Some(BracketMatch {
        bracket: found.bracket,
        partner: found.partner?,
    })
}

/// Where the caret goes for "go to matching bracket": just past the
/// partner, so the caret sits outside the pair the way it does after
/// typing the bracket itself.
pub fn jump_target(language: Language, text: &str, offset: usize) -> Option<usize> {
    matching_bracket(language, text, offset).map(|found| found.partner.end)
}

/// `(range, (open, close), forward)` for the bracket the caret addresses.
fn bracket_at(
    tokens: &Tokens,
    text: &str,
    offset: usize,
) -> Option<(Range<usize>, (String, String), bool)> {
    let offset = offset.min(text.len());
    for (open, close) in &tokens.brackets {
        if text[offset..].starts_with(open.as_str()) {
            return Some((
                offset..offset + open.len(),
                (open.clone(), close.clone()),
                true,
            ));
        }
        if text[offset..].starts_with(close.as_str()) {
            return Some((
                offset..offset + close.len(),
                (open.clone(), close.clone()),
                false,
            ));
        }
    }
    for (open, close) in &tokens.brackets {
        if text[..offset].ends_with(open.as_str()) {
            return Some((
                offset - open.len()..offset,
                (open.clone(), close.clone()),
                true,
            ));
        }
        if text[..offset].ends_with(close.as_str()) {
            return Some((
                offset - close.len()..offset,
                (open.clone(), close.clone()),
                false,
            ));
        }
    }
    None
}

/// The partner as the tree sees it: a sibling token under the same parent.
fn by_grammar(
    syntax: &Syntax,
    bracket: &Range<usize>,
    (open, close): &(String, String),
    forward: bool,
) -> Option<Range<usize>> {
    let node = syntax.node_at(bracket.clone())?;
    let wanted = if forward { close } else { open };
    if node.kind() != if forward { open } else { close } {
        return None;
    }
    let parent = node.parent()?;
    let mut cursor = parent.walk();
    let children: Vec<_> = parent.children(&mut cursor).collect();
    let found = if forward {
        children
            .iter()
            .find(|child| child.kind() == wanted && child.start_byte() >= bracket.end)
    } else {
        children
            .iter()
            .rev()
            .find(|child| child.kind() == wanted && child.end_byte() <= bracket.start)
    }?;
    Some(found.start_byte()..found.end_byte())
}

/// The partner by counting depth, skipping anything the tree says is a
/// string or a comment.
fn by_counting(
    syntax: &Syntax,
    text: &str,
    bracket: &Range<usize>,
    (open, close): &(String, String),
    forward: bool,
) -> Option<Range<usize>> {
    let mut depth = 0i32;
    let positions: Vec<usize> = if forward {
        text[bracket.start..]
            .char_indices()
            .map(|(index, _)| bracket.start + index)
            .collect()
    } else {
        text[..bracket.end]
            .char_indices()
            .rev()
            .map(|(index, _)| index)
            .collect()
    };
    for at in positions {
        if at != bracket.start && syntax.in_literal_or_comment(at) {
            continue;
        }
        let rest = &text[at..];
        if rest.starts_with(open.as_str()) {
            depth += if forward { 1 } else { -1 };
            if !forward && depth == 0 {
                return Some(at..at + open.len());
            }
        } else if rest.starts_with(close.as_str()) {
            depth += if forward { -1 } else { 1 };
            if forward && depth == 0 {
                return Some(at..at + close.len());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntax_core::language_by_id;

    fn lang(id: &str) -> Language {
        language_by_id(id).unwrap_or_else(|| panic!("{id} is in the catalog"))
    }

    #[test]
    fn an_opening_bracket_finds_its_closer_through_the_grammar() {
        let text = "fn main() { if x { y(); } }\n";
        let open = text.find('{').expect("fixture");
        let found = matching_bracket(lang("rust"), text, open).expect("matched");
        assert_eq!(
            found.partner,
            text.rfind('}').expect("fixture")..text.len() - 1
        );
    }

    #[test]
    fn a_closing_bracket_finds_its_opener() {
        let text = "fn main() { if x { y(); } }\n";
        let close = text.rfind('}').expect("fixture");
        let found = matching_bracket(lang("rust"), text, close).expect("matched");
        assert_eq!(
            found.partner,
            text.find('{').expect("fixture")..text.find('{').expect("fixture") + 1
        );
    }

    #[test]
    fn the_caret_just_after_a_bracket_still_matches_it() {
        let text = "foo(bar)\n";
        let after = text.find(')').expect("fixture") + 1;
        let found = matching_bracket(lang("rust"), text, after).expect("matched");
        assert_eq!(found.bracket, 7..8);
        assert_eq!(found.partner, 3..4);
    }

    #[test]
    fn a_bracket_inside_a_string_is_not_the_partner() {
        let text = "let s = \"(\"; foo(bar);\n";
        let open = text.rfind('(').expect("fixture");
        let found = matching_bracket(lang("rust"), text, open).expect("matched");
        assert_eq!(&text[found.partner.clone()], ")");
        assert_eq!(found.partner.start, text.rfind(')').expect("fixture"));
    }

    #[test]
    fn an_unmatched_bracket_has_no_partner() {
        let text = "fn main() { \n";
        let open = text.find('{').expect("fixture");
        assert!(matching_bracket(lang("rust"), text, open).is_none());
    }

    #[test]
    fn pair_at_still_names_an_unmatched_bracket() {
        // matching_bracket refuses this (nothing to jump to); pair_at is
        // what a live highlight needs — the bracket itself, painted as
        // unmatched rather than not painted at all.
        let text = "fn main() { \n";
        let open = text.find('{').expect("fixture");
        let found = pair_at(lang("rust"), text, open).expect("names the bracket");
        assert_eq!(found.bracket, open..open + 1);
        assert!(found.partner.is_none());
    }

    #[test]
    fn a_caret_not_on_a_bracket_matches_nothing() {
        assert!(matching_bracket(lang("rust"), "let x = 1;\n", 4).is_none());
    }

    #[test]
    fn without_a_grammar_the_text_fallback_counts_depth() {
        let text = "a ( b ( c ) d ) e\n";
        let plain = Language::PLAIN_TEXT;
        // Plain text declares no brackets, so nothing matches at all …
        assert!(matching_bracket(plain, text, 2).is_none());
        // … while a language whose file is past the parse ceiling still
        // matches, because the counter needs no tree.
        let huge = format!(
            "// {}\n{text}",
            "x".repeat(syntax_core::MAX_HIGHLIGHT_BYTES)
        );
        let open = huge.find('(').expect("fixture");
        let found = matching_bracket(lang("rust"), &huge, open).expect("matched");
        assert_eq!(&huge[found.partner.clone()], ")");
        assert_eq!(found.partner.start, huge.rfind(')').expect("fixture"));
    }

    #[test]
    fn the_jump_target_is_just_past_the_partner() {
        let text = "foo(bar)\n";
        let open = text.find('(').expect("fixture");
        assert_eq!(jump_target(lang("rust"), text, open), Some(8));
    }
}
