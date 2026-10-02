//! Injected regions: finding them in a tree (`injections.scm`) and the names
//! they use. Split out of `lib.rs` for its size ceiling.

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

use crate::registry::{self, CompiledLanguage, Language};
use crate::{
    pattern_is_guarded_by_an_unevaluated_predicate, HighlightSpan, MAX_HIGHLIGHT_BYTES,
    MAX_INJECTION_DEPTH,
};

/// One injected region found by an `injections.scm` match: the language
/// it is written in, and the byte ranges of its `@injection.content`
/// captures. Several ranges in one match are parsed as *one* tree (that
/// is what `Parser::set_included_ranges` is for), so a language split
/// across several nodes still sees one continuous document.
pub(crate) struct InjectionRegion {
    pub(crate) language: String,
    pub(crate) ranges: Vec<tree_sitter::Range>,
    /// `(#set! injection.combined)`: every match of that pattern for this
    /// language joins one region, parsed as one document.
    combined: bool,
}

impl InjectionRegion {
    /// True when the injected region *contains* `span` — the host span has
    /// nothing left to say about those bytes, so it is dropped in favour of
    /// the injected language's own spans.
    ///
    /// Containment rather than mere overlap, because a host span can
    /// legitimately *enclose* an injected region: Markdown hands every run
    /// of prose to the inline grammar, so a heading, a list item or a fenced
    /// block always encloses an injection, and dropping on overlap left the
    /// whole markup family unpaintable. An enclosing span is sorted before
    /// the spans inside it (see [`spans_with_injections`]), so the injected
    /// language still wins on the bytes it claims.
    pub(crate) fn contains(&self, span: &HighlightSpan) -> bool {
        self.ranges
            .iter()
            .any(|r| span.start >= r.start_byte && span.end <= r.end_byte)
    }
}

/// Fence tags and language names people actually write, mapped onto the
/// catalog ids they mean.
///
/// A Markdown fence is tagged by a human (` ```js `), not by a query
/// author, and injection resolution matches a registry id exactly. The
/// usual tree-sitter answer is one `((#eq? @lang "js") (#set!
/// injection.language "javascript"))` pattern per alias per host language
/// — a table that would have to be written out, and kept in step, in
/// every `injections.scm` that can host a fence. So the normalisation
/// lives here instead: one place, which every injected language name
/// passes through.
///
/// Deliberately short: only aliases that are genuinely common in the
/// wild, and only onto ids the catalog actually has. An unknown name
/// still resolves to nothing and the region is left unhighlighted, which
/// is the correct outcome for a language we do not ship.
const INJECTION_LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("c++", "cpp"),
    ("c#", "csharp"),
    ("cjs", "javascript"),
    ("cs", "csharp"),
    ("cts", "typescript"),
    ("cxx", "cpp"),
    ("golang", "go"),
    ("htm", "html"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("md", "markdown"),
    ("mjs", "javascript"),
    ("mts", "typescript"),
    ("py", "python"),
    ("rs", "rust"),
    ("sh", "bash"),
    ("shell", "bash"),
    ("ts", "typescript"),
    ("yml", "yaml"),
    ("zsh", "bash"),
];

/// [`INJECTION_LANGUAGE_ALIASES`] applied to an already-trimmed,
/// already-lowercased injected language name. A name that is not an alias
/// is returned unchanged — including one that is not a catalog id at all,
/// which resolution then fails on as before.
fn canonical_injection_language(name: &str) -> &str {
    INJECTION_LANGUAGE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, id)| *id)
}

/// The injected regions `query` finds in `tree`.
///
/// Both standard spellings of the language name are supported: an
/// `@injection.language` capture (the node's text names the language) and
/// a `(#set! injection.language "css")` pattern directive. The directive
/// wins when a pattern somehow carries both, since it is the literal the
/// query author wrote rather than text read out of the document.
pub(crate) fn regions(query: &Query, tree: &tree_sitter::Tree, text: &str) -> Vec<InjectionRegion> {
    let capture_names = query.capture_names();
    let mut regions: Vec<InjectionRegion> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        // Same rule as `spans_from_tree`: a guard tree-sitter cannot
        // evaluate takes its pattern down rather than shipping unguarded.
        // Here the cost of getting that wrong is parsing an arbitrary
        // region as the wrong language, not just a wrong colour.
        if pattern_is_guarded_by_an_unevaluated_predicate(query, m.pattern_index) {
            continue;
        }
        let mut language = query
            .property_settings(m.pattern_index)
            .iter()
            .find(|p| &*p.key == "injection.language")
            .and_then(|p| p.value.as_deref())
            .map(str::to_string);
        let mut ranges = Vec::new();
        for capture in m.captures {
            match capture_names[capture.index as usize] {
                "injection.content" => {
                    let range = capture.node.range();
                    if range.end_byte > range.start_byte {
                        ranges.push(range);
                    }
                }
                "injection.language" if language.is_none() => {
                    language = capture
                        .node
                        .utf8_text(text.as_bytes())
                        .ok()
                        .map(|name| name.trim().trim_matches(['"', '\'', '`']).to_lowercase());
                }
                _ => {}
            }
        }
        let Some(language) = language
            .map(|l| canonical_injection_language(l.trim()).to_string())
            .filter(|l| !l.is_empty())
        else {
            continue;
        };
        if ranges.is_empty() {
            continue;
        }
        let combined = query
            .property_settings(m.pattern_index)
            .iter()
            .any(|p| &*p.key == "injection.combined");
        match regions
            .iter_mut()
            .find(|r| combined && r.combined && r.language == language)
        {
            Some(joined) => joined.ranges.extend(ranges),
            None => regions.push(InjectionRegion {
                language,
                ranges,
                combined,
            }),
        }
    }
    // `set_included_ranges` rejects ranges that are not ascending and
    // disjoint, and query captures arrive in match order, not document
    // order.
    for region in &mut regions {
        region.ranges.sort_by_key(|r| r.start_byte);
        region.ranges.dedup_by_key(|r| r.start_byte);
    }
    regions
}

/// Which language each part of a document is written in: the host's, or an
/// injected one's (PHP markup is HTML, a `<script>` in it is JavaScript).
///
/// Built once per text and asked per offset, so an operation over many lines
/// (toggling comments) parses the document once.
#[derive(Debug, Clone)]
pub struct LanguageMap {
    host: Language,
    /// `(byte range, language, depth)` of every injected range, nested ones
    /// included; the innermost (deepest) covering range names the language.
    regions: Vec<(std::ops::Range<usize>, Language, usize)>,
}

impl LanguageMap {
    /// Map `text`, a document in `host`. A document with no injections, a
    /// language with none, or one past [`MAX_HIGHLIGHT_BYTES`] maps wholly to
    /// `host`.
    pub fn of(host: Language, text: &str) -> Self {
        let mut regions = Vec::new();
        if text.len() <= MAX_HIGHLIGHT_BYTES {
            if let Some(compiled) = registry::compiled(host) {
                let mut parser = Parser::new();
                if parser.set_language(&compiled.grammar).is_ok() {
                    if let Some(tree) = parser.parse(text, None) {
                        collect(&compiled, &tree, text, 0, &mut regions);
                    }
                }
            }
        }
        Self { host, regions }
    }

    /// The language `offset` (a byte offset into the mapped text) is written in.
    pub fn at(&self, offset: usize) -> Language {
        self.regions
            .iter()
            .filter(|(range, _, _)| range.contains(&offset))
            .max_by_key(|(_, _, depth)| *depth)
            .map_or(self.host, |(_, language, _)| *language)
    }
}

/// The language `offset` is written in: the innermost injected language
/// covering it, else `host`. See [`LanguageMap`] to ask about many offsets.
pub fn language_at(host: Language, text: &str, offset: usize) -> Language {
    LanguageMap::of(host, text).at(offset)
}

fn collect(
    compiled: &CompiledLanguage,
    tree: &tree_sitter::Tree,
    text: &str,
    depth: usize,
    out: &mut Vec<(std::ops::Range<usize>, Language, usize)>,
) {
    let Some(query) = compiled.injections.as_ref() else {
        return;
    };
    if depth >= MAX_INJECTION_DEPTH {
        return;
    }
    for region in regions(query, tree, text) {
        let Some(language) = registry::language_by_id(&region.language) else {
            continue;
        };
        let Some(inner) = registry::compiled(language) else {
            continue;
        };
        out.extend(
            region
                .ranges
                .iter()
                .map(|r| (r.start_byte..r.end_byte, language, depth + 1)),
        );
        let mut parser = Parser::new();
        if parser.set_language(&inner.grammar).is_err()
            || parser.set_included_ranges(&region.ranges).is_err()
        {
            continue;
        }
        if let Some(subtree) = parser.parse(text, None) {
            collect(&inner, &subtree, text, depth + 1, out);
        }
    }
}
