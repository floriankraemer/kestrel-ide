//! The LSP snippet grammar (`textDocument/completion`'s `insertText` when
//! `insertTextFormat == 2`), parsed into the text to insert plus the ranges
//! Tab/Shift+Tab walk (R2).
//!
//! `lsp_core::completion` used to flatten this itself
//! (`strip_snippet`) back when the client advertised `snippetSupport:
//! false`; now that it advertises `true` (`manager::client_capabilities`),
//! the server sends the real grammar and something has to turn it into a
//! caret session — that something is this module, one layer above
//! `lsp-core` so it can sit beside `editor_core::snippet_session`
//! (`ui-shell` joins the two: parse here, then hand the ranges to a
//! session).
//!
//! Grammar covered: `$1`/`$0` (a bare tab stop), `${1:default}` (with
//! default text, itself possibly nested placeholders), `${1|a,b,c|}` (a
//! choice — its first option is taken; there is no dropdown here), and
//! `\$`/`\}`/`\\` escapes. Variables (`$TM_SELECTED_TEXT` and the like) are
//! not part of this plan item and are left as literal text, the same
//! ceiling `strip_snippet` already had.

use std::iter::Peekable;
use std::ops::Range;
use std::str::Chars;

/// One `$n`/`${n:...}` occurrence: which tab stop it belongs to, and where
/// its default text landed in [`ParsedSnippet::text`] (char offsets, empty
/// range for a bare `$n`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub number: u32,
    pub range: Range<usize>,
}

/// A snippet source reduced to what an editor needs: the text to insert,
/// and every tab stop found in it, in the order they appeared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSnippet {
    pub text: String,
    pub placeholders: Vec<Placeholder>,
}

/// Parse `source` (an LSP snippet, `insertTextFormat: 2`'s `insertText`).
pub fn parse(source: &str) -> ParsedSnippet {
    let mut text = String::new();
    let mut placeholders = Vec::new();
    let mut chars = source.chars().peekable();
    parse_into(&mut chars, &mut text, &mut placeholders);
    ParsedSnippet { text, placeholders }
}

/// Every tab stop in `snippet`, grouped by number and ordered the way
/// Tab/Shift+Tab walk them: ascending by number, with `$0` — "end here",
/// LSP's own convention — always last regardless of its numeric value.
///
/// One range per stop (its first occurrence): a placeholder repeated at two
/// numbers the same (`${1:x}...${1:x}`, "linked editing") mirrors in real
/// IntelliJ/VS Code; tracking every repeat is future work, not this one —
/// see `editor_core::snippet_session`'s own doc comment.
pub fn stops(snippet: &ParsedSnippet) -> Vec<Range<usize>> {
    use std::collections::BTreeMap;
    let mut by_number: BTreeMap<u32, Range<usize>> = BTreeMap::new();
    for placeholder in &snippet.placeholders {
        by_number
            .entry(placeholder.number)
            .or_insert_with(|| placeholder.range.clone());
    }
    let mut ordered: Vec<(u32, Range<usize>)> = by_number.into_iter().collect();
    ordered.sort_by_key(|(number, _)| if *number == 0 { u32::MAX } else { *number });
    ordered.into_iter().map(|(_, range)| range).collect()
}

fn parse_into(chars: &mut Peekable<Chars>, out: &mut String, placeholders: &mut Vec<Placeholder>) {
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some('$') | Some('}') | Some('\\') => out.push(chars.next().expect("peeked")),
                _ => out.push('\\'),
            },
            '$' => match chars.peek() {
                Some('{') => {
                    chars.next();
                    parse_placeholder(chars, out, placeholders);
                }
                Some(next) if next.is_ascii_digit() => {
                    let mut digits = String::new();
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        digits.push(chars.next().expect("peeked"));
                    }
                    let start = out.chars().count();
                    placeholders.push(Placeholder {
                        number: digits.parse().unwrap_or(0),
                        range: start..start,
                    });
                }
                _ => out.push('$'),
            },
            _ => out.push(c),
        }
    }
}

/// `${` already consumed. Reads the number, then either a `:default`, a
/// `|choice,list|`, or nothing (a bare `${1}`).
fn parse_placeholder(
    chars: &mut Peekable<Chars>,
    out: &mut String,
    placeholders: &mut Vec<Placeholder>,
) {
    let mut digits = String::new();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        digits.push(chars.next().expect("peeked"));
    }
    let number: u32 = digits.parse().unwrap_or(0);

    // Collect the raw body up to the matching `}`, keeping escapes intact so
    // a `:`-bodied placeholder can be parsed recursively (nested
    // placeholders, further escapes) exactly as the top level is.
    let mut body = String::new();
    let mut depth = 1usize;
    let mut separator: Option<char> = None;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                body.push('\\');
                if let Some(escaped) = chars.next() {
                    body.push(escaped);
                }
            }
            '{' => {
                depth += 1;
                body.push(c);
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                body.push(c);
            }
            ':' | '|' if separator.is_none() && depth == 1 => separator = Some(c),
            _ if separator.is_some() => body.push(c),
            _ => {}
        }
    }

    let start = out.chars().count();
    match separator {
        Some(':') => {
            let mut body_chars = body.chars().peekable();
            let mut nested_text = String::new();
            let mut nested_placeholders = Vec::new();
            parse_into(&mut body_chars, &mut nested_text, &mut nested_placeholders);
            out.push_str(&nested_text);
            for mut nested in nested_placeholders {
                nested.range = (nested.range.start + start)..(nested.range.end + start);
                placeholders.push(nested);
            }
            let end = out.chars().count();
            placeholders.push(Placeholder {
                number,
                range: start..end,
            });
        }
        Some('|') => {
            // `${1|Ok,Err|}`: the loop above kept accumulating after the
            // separator, including the closing `|` before `}` — trim it,
            // same as `lsp_core::completion::strip_snippet` did.
            let first = body
                .split(',')
                .next()
                .unwrap_or_default()
                .trim_end_matches('|');
            out.push_str(first);
            let end = out.chars().count();
            placeholders.push(Placeholder {
                number,
                range: start..end,
            });
        }
        // `None`: a bare `${1}`. Any other separator can't happen — the
        // guard above only ever sets `':'` or `'|'` — but the match still
        // has to be exhaustive over `Option<char>`.
        _ => placeholders.push(Placeholder {
            number,
            range: start..start,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_tab_stop_is_an_empty_range() {
        let parsed = parse("fn $1() {\n\t$0\n}");
        assert_eq!(parsed.text, "fn () {\n\t\n}");
        assert_eq!(
            parsed.placeholders,
            [
                Placeholder {
                    number: 1,
                    range: 3..3
                },
                Placeholder {
                    number: 0,
                    range: 9..9
                },
            ]
        );
    }

    #[test]
    fn a_placeholder_with_a_default_keeps_it_in_the_text() {
        let parsed = parse("${1:name}: ${2:Type}");
        assert_eq!(parsed.text, "name: Type");
        assert_eq!(
            parsed.placeholders,
            [
                Placeholder {
                    number: 1,
                    range: 0..4
                },
                Placeholder {
                    number: 2,
                    range: 6..10
                },
            ]
        );
    }

    #[test]
    fn a_choice_placeholder_takes_its_first_option() {
        let parsed = parse("${1|Ok,Err|}");
        assert_eq!(parsed.text, "Ok");
        assert_eq!(
            parsed.placeholders,
            [Placeholder {
                number: 1,
                range: 0..2
            }]
        );
    }

    #[test]
    fn nesting_offsets_the_inner_placeholder_by_the_outer_one_start() {
        let parsed = parse("price\\$ ${1:${2:nested}}");
        assert_eq!(parsed.text, "price$ nested");
        assert_eq!(
            parsed.placeholders,
            [
                Placeholder {
                    number: 2,
                    range: 7..13
                },
                Placeholder {
                    number: 1,
                    range: 7..13
                },
            ],
            "the inner placeholder is recorded first, both spanning the same text \
             the outer one wraps"
        );
    }

    #[test]
    fn escapes_are_preserved_as_plain_text() {
        assert_eq!(parse("cost $ 5").text, "cost $ 5", "a lone $ is text");
        assert_eq!(
            parse("100\\% done").text,
            "100\\% done",
            "not a snippet escape"
        );
    }

    #[test]
    fn stops_are_ordered_ascending_with_dollar_zero_last() {
        let parsed = parse("${2:b}${1:a}$0");
        assert_eq!(stops(&parsed), [1..2, 0..1, 2..2]);
    }

    #[test]
    fn a_repeated_number_keeps_only_its_first_range() {
        let parsed = parse("${1:x} ${1:x}");
        assert_eq!(stops(&parsed), vec![0..1]);
    }

    #[test]
    fn a_snippet_with_no_placeholders_has_no_stops() {
        assert!(stops(&parse("println!()")).is_empty());
    }
}
