//! A shared lexical scanner `split`, `classify`, `navigation` and
//! `completion` all drive instead of each hand-rolling its own
//! string/comment-skipping loop. Deliberately not a full SQL tokenizer —
//! it only needs to (a) tell a keyword/identifier `Word` apart from
//! punctuation, and (b) never mistake a `;` or a keyword *inside* a string
//! or a comment for a real one. `sqlparser`'s own tokenizer already does
//! this correctly but only for input that parses fully; a console's
//! contents very often don't (a script mid-edit, one bad statement among
//! several good ones), so `split`/`classify` need a scanner that keeps
//! going regardless.

use crate::dialects::LexOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    /// A run of identifier characters, or the (unescaped) contents of a
    /// quoted identifier — callers that match against a keyword compare
    /// case-insensitively, which is safe here since a quoted identifier
    /// happening to spell a keyword is a rare, inconsequential
    /// misclassification (see this module's doc comment).
    Word(String),
    /// Any other single character token: `(`, `)`, `,`, `.`, `;`, `*`, …
    Symbol(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
}

/// If `sql[i..]` starts a string literal, a quoted identifier, a comment,
/// or (when `opts.dollar_quotes`) a dollar-quoted body, returns the byte
/// index just past it. `None` means "not the start of anything this
/// scanner skips opaquely" — the caller then handles `sql[i..]` as an
/// ordinary character.
pub(crate) fn skip_ignorable(sql: &str, i: usize, opts: &LexOptions) -> Option<usize> {
    let rest = &sql[i..];
    if let Some(stripped) = rest.strip_prefix('\'') {
        return Some(i + 1 + skip_quoted_body(stripped, '\''));
    }
    if let Some(stripped) = rest.strip_prefix('"') {
        return Some(i + 1 + skip_quoted_body(stripped, '"'));
    }
    if let Some(stripped) = rest.strip_prefix('`') {
        return Some(i + 1 + skip_quoted_body(stripped, '`'));
    }
    if rest.starts_with("--") {
        let end = rest.find('\n').unwrap_or(rest.len());
        return Some(i + end);
    }
    if opts.hash_comment && rest.starts_with('#') {
        let end = rest.find('\n').unwrap_or(rest.len());
        return Some(i + end);
    }
    if let Some(stripped) = rest.strip_prefix("/*") {
        let end = stripped.find("*/").map(|p| p + 2).unwrap_or(stripped.len());
        return Some(i + 2 + end);
    }
    if opts.dollar_quotes && rest.starts_with('$') {
        // `$$` or `$tag$` — the tag is `[A-Za-z0-9_]*` between the two `$`.
        let after_first = &rest[1..];
        let tag_len = after_first
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after_first.len());
        if after_first[tag_len..].starts_with('$') {
            let opener_len = 1 + tag_len + 1; // `$` + tag + `$`
            let opener = &rest[..opener_len];
            let body = &rest[opener_len..];
            let close = body
                .find(opener)
                .map(|p| p + opener.len())
                .unwrap_or(body.len());
            return Some(i + opener_len + close);
        }
    }
    None
}

/// Consumes `body` (the text right after the opening quote) up to and
/// including the matching closing quote, treating a doubled quote
/// (`''`, `""`, ` `` `) as an escaped literal quote rather than the
/// closer. Returns the byte length consumed, opening quote excluded.
fn skip_quoted_body(body: &str, quote: char) -> usize {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] as char == quote {
            if body[i + 1..].starts_with(quote) {
                i += quote.len_utf8() * 2;
                continue;
            }
            return i + quote.len_utf8();
        }
        // Advance by one char, not one byte, to stay on UTF-8 boundaries.
        let ch_len = body[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        i += ch_len;
    }
    i
}

/// Tokenizes the whole input eagerly. Fine for a console's or a script
/// file's usual size; not something a multi-hundred-MB dump should be run
/// through.
/// ponytail: whole-string tokenize, revisit with an incremental scanner if
/// a real script this size ever shows up in `split`'s NFR bench.
pub fn scan(sql: &str, opts: &LexOptions) -> Vec<Spanned> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < sql.len() {
        if let Some(skip_to) = skip_ignorable(sql, i, opts) {
            i = skip_to.max(i + 1);
            continue;
        }
        let ch = sql[i..].chars().next().unwrap();
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        if ch.is_alphanumeric() || ch == '_' {
            let word_len = sql[i..]
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(sql.len() - i);
            out.push(Spanned {
                tok: Tok::Word(sql[i..i + word_len].to_string()),
                start: i,
                end: i + word_len,
            });
            i += word_len;
            continue;
        }
        out.push(Spanned {
            tok: Tok::Symbol(ch),
            start: i,
            end: i + ch.len_utf8(),
        });
        i += ch.len_utf8();
    }
    out
}

/// The byte range of the identifier (bare, or quoted with any of `"`/`` ` ``)
/// that contains `offset`, if any — `navigation`'s and `completion`'s
/// shared "what word is the caret on/near" primitive.
pub fn word_at(sql: &str, offset: usize) -> Option<(usize, usize, String)> {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    if offset > sql.len() || !sql.is_char_boundary(offset) {
        return None;
    }
    // A quoted identifier: only recognised when the caret sits strictly
    // inside the quotes (an unclosed quote does not count as a word).
    for quote in ['"', '`'] {
        if let Some(start) = sql[..offset].rfind(quote) {
            if let Some(rel_end) = sql[start + 1..].find(quote) {
                let end = start + 1 + rel_end;
                if offset > start && offset <= end {
                    return Some((start + 1, end, sql[start + 1..end].to_string()));
                }
            }
        }
    }
    let start = sql[..offset]
        .rfind(|c: char| !is_word_char(c))
        .map(|p| p + 1)
        .unwrap_or(0);
    let end = offset
        + sql[offset..]
            .find(|c: char| !is_word_char(c))
            .unwrap_or(sql.len() - offset);
    if start >= end {
        return None;
    }
    let word = &sql[start..end];
    if word.is_empty() || !word.chars().next().unwrap().is_alphanumeric() && !word.starts_with('_')
    {
        return None;
    }
    Some((start, end, word.to_string()))
}

/// 0-based line and UTF-16 `character` for a byte offset — the same
/// counting convention `diagnostics_core::Position` uses, so a
/// `db-sql`-produced diagnostic needs no further translation.
pub fn position_at(sql: &str, offset: usize) -> diagnostics_core::Position {
    let mut line = 0u32;
    let mut last_newline = 0usize;
    for (idx, ch) in sql[..offset.min(sql.len())].char_indices() {
        if ch == '\n' {
            line += 1;
            last_newline = idx + 1;
        }
    }
    let character = sql[last_newline..offset.min(sql.len())]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    diagnostics_core::Position { line, character }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> LexOptions {
        LexOptions {
            hash_comment: false,
            dollar_quotes: false,
            delimiter_directive: false,
        }
    }

    #[test]
    fn scan_splits_words_and_symbols() {
        let toks = scan("SELECT * FROM t", &opts());
        assert_eq!(
            toks.iter().map(|s| s.tok.clone()).collect::<Vec<_>>(),
            vec![
                Tok::Word("SELECT".into()),
                Tok::Symbol('*'),
                Tok::Word("FROM".into()),
                Tok::Word("t".into()),
            ]
        );
    }

    #[test]
    fn scan_skips_a_string_literal_containing_a_semicolon() {
        let toks = scan("SELECT ';' AS x", &opts());
        assert!(!toks.iter().any(|s| matches!(&s.tok, Tok::Symbol(';'))));
    }

    #[test]
    fn scan_handles_a_doubled_quote_escape() {
        let toks = scan("SELECT 'it''s' AS x", &opts());
        // The whole 'it''s' literal is skipped as one string, so the next
        // token is AS, not a stray `s`.
        assert_eq!(toks[1].tok, Tok::Word("AS".into()));
    }

    #[test]
    fn scan_skips_line_and_block_comments() {
        let toks = scan("SELECT 1 -- trailing\n/* block */ FROM t", &opts());
        assert_eq!(
            toks.iter().map(|s| s.tok.clone()).collect::<Vec<_>>(),
            vec![
                Tok::Word("SELECT".into()),
                Tok::Word("1".into()),
                Tok::Word("FROM".into()),
                Tok::Word("t".into()),
            ]
        );
    }

    #[test]
    fn scan_honours_hash_comments_only_when_enabled() {
        let hash_opts = LexOptions {
            hash_comment: true,
            ..opts()
        };
        let toks = scan("SELECT 1 # comment\nFROM t", &hash_opts);
        assert_eq!(toks.last().unwrap().tok, Tok::Word("t".into()));
    }

    #[test]
    fn scan_skips_a_dollar_quoted_body() {
        let dollar_opts = LexOptions {
            dollar_quotes: true,
            ..opts()
        };
        let toks = scan("SELECT $$a; b$$ FROM t", &dollar_opts);
        assert!(!toks.iter().any(|s| matches!(&s.tok, Tok::Symbol(';'))));
        assert_eq!(toks.last().unwrap().tok, Tok::Word("t".into()));
    }

    #[test]
    fn scan_skips_a_tagged_dollar_quoted_body() {
        let dollar_opts = LexOptions {
            dollar_quotes: true,
            ..opts()
        };
        let toks = scan("SELECT $tag$a; b$tag$ FROM t", &dollar_opts);
        assert!(!toks.iter().any(|s| matches!(&s.tok, Tok::Symbol(';'))));
    }

    #[test]
    fn word_at_finds_a_bare_identifier_under_the_caret() {
        let (start, end, word) = word_at("SELECT foo FROM t", 8).unwrap();
        assert_eq!(
            (&"SELECT foo FROM t"[start..end], word.as_str()),
            ("foo", "foo")
        );
    }

    #[test]
    fn word_at_finds_a_quoted_identifier() {
        let (_, _, word) = word_at("SELECT \"my col\" FROM t", 10).unwrap();
        assert_eq!(word, "my col");
    }

    #[test]
    fn word_at_is_none_between_words() {
        assert_eq!(word_at("a  b", 2), None);
    }

    #[test]
    fn position_at_counts_lines_and_utf16_columns() {
        let pos = position_at("SELECT 1\nFROM t", 11);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 2);
    }
}
