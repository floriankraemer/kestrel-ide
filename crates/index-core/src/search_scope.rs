//! R8: Find in Files depth — a file mask + path scope applied at the
//! candidate stage, whole-word matching, a literal-prefix ngram narrowing
//! for regex queries, and a `total_hint` so the view can say "N of M".
//!
//! A private-field `impl TextIndex` block, same trick `lifecycle.rs` and
//! `replace_preview.rs` already use: this module is a descendant of the
//! crate root, so it can reach `TextIndex`'s private fields without them
//! being `pub(crate)`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;

use crate::{escape_literal, IndexError, SearchMatch, TextIndex, NGRAM_SIZE};

/// A file mask, IntelliJ-style: comma/semicolon-separated glob patterns,
/// `!pattern` for an exclusion — `*.rs, !*_test.rs` keeps every Rust file
/// except test files. A pattern with no path separator matches the file
/// name only (`*.rs` matches `src/main.rs`), same as `.gitignore`.
#[derive(Debug, Clone)]
pub struct FileMask {
    include: GlobSet,
    exclude: GlobSet,
    /// Whether any positive pattern was given — an all-exclusions mask
    /// (`!*_test.rs` alone) still means "everything except that", not
    /// "nothing".
    has_include: bool,
}

impl FileMask {
    /// Parse a comma/semicolon-separated mask. An empty or all-whitespace
    /// `spec` is not an error — it is simply no mask (`matches` always
    /// true) — so a blank mask field in the toolbar behaves as "no filter"
    /// rather than "match nothing".
    pub fn parse(spec: &str) -> Result<Option<FileMask>, IndexError> {
        let mut include = GlobSetBuilder::new();
        let mut exclude = GlobSetBuilder::new();
        let mut has_include = false;
        let mut any = false;
        for raw in spec.split([',', ';']) {
            let pattern = raw.trim();
            if pattern.is_empty() {
                continue;
            }
            any = true;
            let (negated, glob_text) = match pattern.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, pattern),
            };
            let glob = Glob::new(glob_text).map_err(|e| IndexError::Query(e.to_string()))?;
            if negated {
                exclude.add(glob);
            } else {
                include.add(glob);
                has_include = true;
            }
        }
        if !any {
            return Ok(None);
        }
        Ok(Some(FileMask {
            include: include
                .build()
                .map_err(|e| IndexError::Query(e.to_string()))?,
            exclude: exclude
                .build()
                .map_err(|e| IndexError::Query(e.to_string()))?,
            has_include,
        }))
    }

    /// `relative` is the project-relative path (forward-slashed), so a
    /// bare `*.rs` pattern (no `/`) matches by file name via globset's own
    /// "no separator in the pattern -> match the base name" rule.
    fn matches(&self, relative: &Path) -> bool {
        let included = !self.has_include || self.include.is_match(relative);
        included && !self.exclude.is_match(relative)
    }
}

/// Which files a search is allowed to touch, beyond the mask: an explicit
/// allow-list of paths (a chosen directory, or the open-files set) —
/// `None` means the whole project.
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    pub paths: Option<Vec<PathBuf>>,
    pub mask: Option<FileMask>,
    pub whole_word: bool,
}

impl SearchScope {
    fn allows(&self, path: &Path, root: &Path) -> bool {
        if let Some(paths) = &self.paths {
            if !paths.iter().any(|allowed| path.starts_with(allowed)) {
                return false;
            }
        }
        if let Some(mask) = &self.mask {
            let relative = path.strip_prefix(root).unwrap_or(path);
            let relative = relative.to_string_lossy().replace('\\', "/");
            if !mask.matches(Path::new(&relative)) {
                return false;
            }
        }
        true
    }
}

/// [`TextIndex::search_with`]'s result, plus how many matches there really
/// are: `matches.len() < total_hint` means the result was capped by
/// `limit` and the view should say "showing `matches.len()` of
/// `total_hint`, refine your search" instead of a bare count.
#[derive(Debug, Clone, Default)]
pub struct ScopedSearchResult {
    pub matches: Vec<SearchMatch>,
    pub total_hint: usize,
}

/// A regex's longest required literal prefix, for ngram candidate
/// narrowing — `^foo.*` and `foo(bar|baz)` both narrow on `"foo"`;
/// `.*foo` or an alternation with no common prefix narrow on nothing (the
/// caller falls back to "every indexed file", same as an unparseable
/// pattern already did before R8).
///
/// `Seq::literals()` for [`ExtractKind::Prefix`] holds every alternative a
/// match may start with, not one prefix shared by all of them — `(abc|xyz)`
/// extracts to `["abc", "xyz"]`, and narrowing on just `"xyz"` would wrongly
/// drop a file that only contains `abc`. So only the single-alternative
/// case (`seq.literals()` has exactly one entry) is a *required* prefix
/// safe to narrow on; an alternation with more than one falls back to
/// "every indexed file", same as before R8.
pub fn literal_prefix(pattern: &str) -> Option<String> {
    let hir = regex_syntax::Parser::new().parse(pattern).ok()?;
    let seq = regex_syntax::hir::literal::Extractor::new()
        .kind(regex_syntax::hir::literal::ExtractKind::Prefix)
        .extract(&hir);
    let [literal] = seq.literals()? else {
        return None;
    };
    let text = std::str::from_utf8(literal.as_bytes()).ok()?;
    (text.chars().count() >= NGRAM_SIZE).then(|| text.to_string())
}

impl TextIndex {
    /// [`TextIndex::search_with`], scoped: a [`SearchScope`] (path
    /// allow-list, file mask, whole word) applied at the candidate stage,
    /// and a [`ScopedSearchResult::total_hint`] that keeps counting past
    /// `limit` instead of stopping the scan the moment the cap is hit —
    /// the whole point of R8's "N of M, refine" affordance is knowing M.
    pub fn search_scoped(
        &self,
        pattern: &str,
        is_regex: bool,
        case_sensitive: bool,
        scope: &SearchScope,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<ScopedSearchResult, IndexError> {
        let owned_pattern;
        let mut regex_pattern: String = if is_regex {
            pattern.to_string()
        } else {
            owned_pattern = escape_literal(pattern);
            owned_pattern
        };
        if scope.whole_word {
            regex_pattern = format!(r"\b(?:{regex_pattern})\b");
        }
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!case_sensitive)
            .build(&regex_pattern)
            .map_err(|e| IndexError::Query(e.to_string()))?;

        let mut result = ScopedSearchResult::default();
        for path in self.candidate_files_scoped(pattern, is_regex, case_sensitive, scope)? {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut searcher = Searcher::new();
            let path_for_sink = path.clone();
            let matches = &mut result.matches;
            let total_hint = &mut result.total_hint;
            let search_result = searcher.search_path(
                &matcher,
                &path,
                UTF8(|line_number, line| {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    let mut from = 0;
                    while let Ok(Some(m)) = matcher.find_at(line.as_bytes(), from) {
                        *total_hint += 1;
                        if matches.len() < limit {
                            matches.push(SearchMatch {
                                path: path_for_sink.clone(),
                                line: line_number as usize,
                                start: m.start(),
                                end: m.end(),
                                line_text: trimmed.to_string(),
                            });
                        }
                        from = if m.end() > m.start() {
                            m.end()
                        } else {
                            m.end() + 1
                        };
                        if from > line.len() {
                            break;
                        }
                    }
                    Ok(true)
                }),
            );
            // Same as `search_with`: a candidate that vanished between
            // narrowing and now is skipped, not a whole-search failure.
            let _ = search_result;
        }
        Ok(result)
    }

    /// [`TextIndex::candidate_files`], extended with `scope` filtering and
    /// regex literal-prefix narrowing (`R8`'s upgrade of the ceiling
    /// `candidate_files` itself documents) — a regex whose
    /// [`literal_prefix`] is long enough to produce ngram terms narrows on
    /// that prefix exactly like a literal query narrows on the whole
    /// pattern.
    fn candidate_files_scoped(
        &self,
        pattern: &str,
        is_regex: bool,
        case_sensitive: bool,
        scope: &SearchScope,
    ) -> Result<Vec<PathBuf>, IndexError> {
        let base = if is_regex {
            match case_sensitive.then(|| literal_prefix(pattern)).flatten() {
                Some(prefix) => self.narrow_on(&prefix)?,
                None => self.candidate_files(pattern, case_sensitive)?,
            }
        } else {
            self.candidate_files(pattern, case_sensitive)?
        };
        Ok(base
            .into_iter()
            .filter(|path| scope.allows(path, &self.root))
            .collect())
    }

    /// Ngram-narrow on a plain literal string, sharing tantivy's query
    /// parser the way [`TextIndex::candidate_files`]'s literal branch does.
    fn narrow_on(&self, literal: &str) -> Result<Vec<PathBuf>, IndexError> {
        let searcher = self.reader.searcher();
        let num_docs = searcher.num_docs() as usize;
        if num_docs == 0 {
            return Ok(Vec::new());
        }
        let query_parser = QueryParser::for_index(&self.index, vec![self.fields.content]);
        let Some(query) = query_parser.parse_query(literal).ok() else {
            return self.candidate_files(literal, true);
        };
        let top_docs = searcher.search(&query, &TopDocs::with_limit(num_docs))?;
        let mut paths = Vec::with_capacity(top_docs.len());
        for (_score, doc_address) in top_docs {
            let retrieved: tantivy::TantivyDocument = searcher.doc(doc_address)?;
            if let Some(value) = retrieved.get_first(self.fields.path) {
                if let Some(text) = value.as_str() {
                    paths.push(PathBuf::from(text));
                }
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mask_matches_by_extension_across_directories() {
        let mask = FileMask::parse("*.rs").unwrap().unwrap();
        assert!(mask.matches(Path::new("src/main.rs")));
        assert!(!mask.matches(Path::new("src/main.py")));
    }

    #[test]
    fn file_mask_negation_excludes_after_an_include() {
        let mask = FileMask::parse("*.rs, !*_test.rs").unwrap().unwrap();
        assert!(mask.matches(Path::new("src/lib.rs")));
        assert!(!mask.matches(Path::new("src/lib_test.rs")));
    }

    #[test]
    fn an_exclusion_only_mask_still_matches_everything_else() {
        let mask = FileMask::parse("!*_test.rs").unwrap().unwrap();
        assert!(mask.matches(Path::new("src/lib.rs")));
        assert!(!mask.matches(Path::new("src/lib_test.rs")));
    }

    #[test]
    fn a_blank_mask_is_not_an_error_and_matches_everything() {
        assert!(FileMask::parse("   ").unwrap().is_none());
    }

    #[test]
    fn an_invalid_glob_is_reported() {
        assert!(FileMask::parse("[").is_err());
    }

    #[test]
    fn scope_paths_restrict_to_an_allow_listed_subtree() {
        let root = PathBuf::from("/proj");
        let scope = SearchScope {
            paths: Some(vec![PathBuf::from("/proj/src")]),
            mask: None,
            whole_word: false,
        };
        assert!(scope.allows(Path::new("/proj/src/main.rs"), &root));
        assert!(!scope.allows(Path::new("/proj/docs/readme.md"), &root));
    }

    #[test]
    fn literal_prefix_extracts_the_required_leading_text() {
        assert_eq!(literal_prefix("foobar.*"), Some("foobar".to_string()));
        assert_eq!(literal_prefix("^Handler\\w+"), Some("Handler".to_string()));
    }

    #[test]
    fn literal_prefix_is_none_when_nothing_is_required_up_front() {
        assert_eq!(literal_prefix(".*foo"), None);
        assert_eq!(literal_prefix("(abc|xyz)"), None);
        // Too short to produce an ngram term.
        assert_eq!(literal_prefix("ab.*"), None);
    }

    #[test]
    fn search_scoped_respects_a_file_mask() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "needle here\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "needle here\n").unwrap();
        let index = TextIndex::build(dir.path()).unwrap();

        let scope = SearchScope {
            paths: None,
            mask: FileMask::parse("*.rs").unwrap(),
            whole_word: false,
        };
        let result = index
            .search_scoped(
                "needle",
                false,
                true,
                &scope,
                usize::MAX,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(result.matches.len(), 1);
        assert!(result.matches[0].path.ends_with("a.rs"));
        assert_eq!(result.total_hint, 1);
    }

    #[test]
    fn search_scoped_whole_word_skips_substring_hits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "needled needle\n").unwrap();
        let index = TextIndex::build(dir.path()).unwrap();

        let scope = SearchScope {
            paths: None,
            mask: None,
            whole_word: true,
        };
        let result = index
            .search_scoped(
                "needle",
                false,
                true,
                &scope,
                usize::MAX,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            result.matches.len(),
            1,
            "only the standalone word should match"
        );
    }

    #[test]
    fn total_hint_keeps_counting_past_a_low_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x x x x x\n").unwrap();
        let index = TextIndex::build(dir.path()).unwrap();

        let result = index
            .search_scoped(
                "x",
                false,
                true,
                &SearchScope::default(),
                2,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.total_hint, 5);
    }
}
