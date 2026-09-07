//! A pre-tokenised slice as an `imara_diff` input.
//!
//! `imara_diff` ships line sources only; both the whitespace modes (lines
//! compared by a rule) and the intra-line diff (words or characters) need
//! to hand it a token list they built themselves.

use std::hash::Hash;

use imara_diff::TokenSource;

pub(super) struct Tokens<'a, T>(pub(super) &'a [T]);

impl<'a, T: Eq + Hash + Clone> TokenSource for Tokens<'a, T> {
    type Token = T;
    type Tokenizer = std::iter::Cloned<std::slice::Iter<'a, T>>;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.iter().cloned()
    }

    fn estimate_tokens(&self) -> u32 {
        self.0.len() as u32
    }
}
