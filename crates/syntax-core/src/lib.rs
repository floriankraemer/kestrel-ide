//! Tree-sitter-backed syntax highlighting foundation.
//!
//! Qt-free by design (mirrors `editor-core`/`project-model`) — `ui-shell`
//! wraps [`highlight`] behind a `QSyntaxHighlighter` adapter later. This
//! crate only classifies bytes of already-loaded text into spans.

mod catalog;
mod folds;
mod registry;
pub mod runtime;
pub mod theme;

use std::ops::Range;
use std::sync::Arc;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

pub use registry::{
    language_by_id, language_for_path, language_name, registry, reload, CompiledLanguage, Def,
    Language, LanguageDef, LanguageRegistry, OwnedLanguageDef, QuerySet, BUILTIN_LANGUAGES,
};

/// The highlighting vocabulary: the standard tree-sitter capture names
/// that upstream `queries/highlights.scm` files actually emit.
///
/// **Static and closed, deliberately.** Runtime-loaded grammars never
/// intern new entries — an unknown capture falls back up its dotted path
/// (see [`Scope::resolve`]) or is dropped. Interning would let this table
/// and the view's format table (indexed by the same ids) drift apart, and
/// a stale id is a wrong colour, not a compile error. Adding a scope means
/// editing this list and nothing else: the seam carries a bare id and the
/// view range-guards it.
///
/// Append-friendly but *not* reorder-friendly while a build is in flight:
/// ids are only meaningful within one build (they are never persisted —
/// [`Scope::name`] is what T2 writes to disk).
pub static SCOPES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.call",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "markup",
    "markup.bold",
    "markup.heading",
    "markup.heading.1",
    "markup.heading.2",
    "markup.heading.3",
    "markup.heading.4",
    "markup.heading.5",
    "markup.heading.6",
    "markup.italic",
    "markup.link",
    "markup.link.label",
    "markup.link.url",
    "markup.list",
    "markup.quote",
    "markup.raw",
    "markup.raw.block",
    "markup.strikethrough",
    "module",
    "number",
    "number.float",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regexp",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "type.definition",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
];

/// A handle into [`SCOPES`] — an index, not an enum, so adding a scope is
/// a one-line table edit and never a bridge or C++ change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Scope(u16);

impl Scope {
    /// The scope a query capture name maps to, with hierarchical fallback:
    /// `a.b.c` → `a.b` → `a` → `None`.
    ///
    /// This is what makes upstream `.scm` files usable unmodified — a
    /// grammar we never saw can emit `@keyword.coroutine` and still get
    /// keyword colouring. Resolution is done once per query at
    /// compile time (see `registry::capture_scopes`), never per span.
    pub fn resolve(capture_name: &str) -> Option<Scope> {
        let mut candidate = capture_name;
        loop {
            if let Some(index) = SCOPES.iter().position(|s| *s == candidate) {
                return Some(Scope(index as u16));
            }
            candidate = candidate.rsplit_once('.')?.0;
        }
    }

    /// The canonical capture name, e.g. `"function.method"`.
    pub fn name(self) -> &'static str {
        SCOPES[usize::from(self.0)]
    }

    /// The raw table index, as it crosses the FFI seam.
    pub fn id(self) -> u16 {
        self.0
    }

    /// The inverse of [`Scope::id`]: rebuilds a `Scope` from a raw table
    /// index a caller read back off the FFI seam (C9's semantic-token
    /// overlay does this — a `FfiHighlightSpan::scope` this same process
    /// wrote out earlier). `None` for an out-of-range id, matching the
    /// range-guard [`HighlightSpan`]'s own doc comment says the view must
    /// already apply.
    pub fn from_id(id: u16) -> Option<Scope> {
        (usize::from(id) < SCOPES.len()).then_some(Scope(id))
    }
}

/// A classified byte range within the highlighted text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub scope: Scope,
}

/// One occurrence of an identifier-like node (byte range `start..end` into
/// the source text `name` was read from), from `locals.scm`'s
/// `@definition`/`@reference` captures (A2). `is_definition` is true when
/// this occurrence is also a declaration site (function/struct/parameter/
/// `let`-binding name, ...) per the language's `locals.scm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub is_definition: bool,
}

/// A foldable region (Task C): byte range `start..end` of a block/body-like
/// node (function/method body, class/struct/enum body, object/array, ...)
/// from the language's `folds.scm` `@fold` capture. Emitted in document
/// order by [`Highlighter::fold_ranges`], which reads off the same
/// incrementally-maintained tree `set_text`/`edit` already keep current —
/// no second full-buffer parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRange {
    pub start: usize,
    pub end: usize,
    /// Byte offset of the line the fold marker anchors to: the declaration
    /// that owns this block (`fn foo(...)`, `class Foo`, `impl Foo {`, ...)
    /// rather than the `{` line itself. Computed structurally — see
    /// [`Highlighter::fold_ranges`] — with no per-language special-casing.
    /// Equal to `start` when no owning declaration is found.
    pub anchor: usize,
}

/// Structural kind of a symbol extracted by [`outline()`] (Task D). Not
/// every language uses every variant — e.g. Rust has no `Interface` in the
/// literal sense (its `trait`s map onto it), PHP's `trait`s also map onto
/// it (see `php/tags.scm`), and JSON uses none of them. Kept to what's
/// actually meaningful across the languages whose grammar distinguishes
/// it rather than a maximal per-language taxonomy — a language with no
/// distinct grammar node for e.g. a property keeps reporting it as
/// `Field` (see each `tags.scm`'s own comments for what a given
/// language's grammar actually supports).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Struct,
    Enum,
    Interface,
    Method,
    Function,
    Field,
    Constant,
    Property,
    Constructor,
    EnumMember,
}

mod symbol_category;
pub use symbol_category::SymbolCategory;

/// One definition site extracted by [`outline()`] (Task D): a name plus
/// its structural kind, the byte range of the whole definition (`start`/
/// `end` — used to jump-select or fold the definition) and of just its
/// name token (`name_start`/`name_end` — used to place the cursor exactly
/// on the identifier). `children` holds definitions nested inside this
/// one by AST byte-range containment (e.g. a class's methods/fields, an
/// `impl` block's methods) — see [`outline()`]'s doc comment for why
/// containment, not an explicit parent capture, decides nesting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolNode {
    pub name: String,
    pub kind: SymbolKind,
    pub start: usize,
    pub end: usize,
    pub name_start: usize,
    pub name_end: usize,
    pub children: Vec<SymbolNode>,
}

/// One "subtype declares supertype" edge extracted by
/// [`supertype_edges()`]: `type_name` is the declaring type (the class,
/// struct, interface or `impl` target), `supertype_name` is one type it
/// extends, implements, or -- in Rust -- one trait it implements.
///
/// A declaration listing several supertypes produces one edge per
/// supertype. `type_start`/`type_end` are the byte range of the declaring
/// type's *name token*, so a jump can land exactly on the identifier
/// (the same convention [`SymbolNode`]'s `name_start` uses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupertypeEdge {
    pub type_name: String,
    pub supertype_name: String,
    pub type_start: usize,
    pub type_end: usize,
}

/// Map a `tags.scm` `@definition.<kind>` capture name onto [`SymbolKind`]
/// — the part after the dot is the kind name verbatim (lowercased).
fn symbol_kind_for_capture(capture_name: &str) -> Option<SymbolKind> {
    match capture_name.strip_prefix("definition.")? {
        "class" => Some(SymbolKind::Class),
        "struct" => Some(SymbolKind::Struct),
        "enum" => Some(SymbolKind::Enum),
        "interface" => Some(SymbolKind::Interface),
        "method" => Some(SymbolKind::Method),
        "function" => Some(SymbolKind::Function),
        "field" => Some(SymbolKind::Field),
        "constant" => Some(SymbolKind::Constant),
        "property" => Some(SymbolKind::Property),
        "constructor" => Some(SymbolKind::Constructor),
        "enum_member" => Some(SymbolKind::EnumMember),
        _ => None,
    }
}

/// One-shot full parse of `text` with `grammar`, for the stateless
/// extraction entry points. `Highlighter` keeps its own persistent tree
/// instead.
fn parse_once(grammar: &tree_sitter::Language, text: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(grammar).ok()?;
    parser.parse(text, None)
}

/// Highlight `text` as `language`, returning spans in document order.
///
/// Stateless one-shot convenience wrapper for tests/simple callers: builds
/// a throwaway [`Highlighter`], does one full parse, and discards it. For
/// repeated highlighting of an evolving document (the real editor use
/// case), construct a [`Highlighter`] once and call [`Highlighter::edit`]
/// per change instead — that keeps the persistent tree and reparses
/// incrementally rather than re-parsing the whole buffer every time.
pub fn highlight(language: Language, text: &str) -> Vec<HighlightSpan> {
    Highlighter::new(language).set_text(text)
}

/// Every identifier-like node in `text`, parsed as `language`, in document
/// order — not just declaration sites (A2). Stateless one-shot, matching
/// [`highlight`]'s convention: does its own parse rather than reusing a
/// [`Highlighter`]'s persistent tree, since nothing needs this
/// incrementally yet.
///
/// Backed by a `locals.scm` per language (see `crates/syntax-core/queries/
/// */locals.scm`) with `@definition`/`@reference` captures. A node can
/// legitimately match both (e.g. a function name is a definition site and
/// also matches the catch-all reference pattern — see the comment atop
/// `rust/locals.scm`), so captures are folded by node byte-range with OR:
/// each identifier node appears exactly once in the result, with
/// `is_definition` true if any capture on it was `@definition`.
/// [`Language::PLAIN_TEXT`] (or a language with no `locals.scm`) yields an
/// empty vec.
pub fn identifier_occurrences(language: Language, text: &str) -> Vec<Occurrence> {
    let Some(compiled) = registry::compiled(language) else {
        return Vec::new();
    };
    let (Some(query), Some(tree)) = (
        compiled.locals.as_ref(),
        parse_once(&compiled.grammar, text),
    ) else {
        return Vec::new();
    };
    occurrences_from_tree(query, &tree, text)
}

/// [`identifier_occurrences`]'s body against an already-parsed tree, so
/// [`analyze_file`] can reuse one parse across all three extractions.
fn occurrences_from_tree(query: &Query, tree: &tree_sitter::Tree, text: &str) -> Vec<Occurrence> {
    count_query_walk();
    let mut by_range: std::collections::BTreeMap<(usize, usize), bool> =
        std::collections::BTreeMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let capture_name = query.capture_names()[capture.index as usize];
            let is_definition = match capture_name {
                "definition" => true,
                "reference" => false,
                _ => continue,
            };
            let range = (capture.node.start_byte(), capture.node.end_byte());
            let entry = by_range.entry(range).or_insert(false);
            *entry |= is_definition;
        }
    }

    by_range
        .into_iter()
        .map(|((start, end), is_definition)| Occurrence {
            name: text[start..end].to_string(),
            start,
            end,
            is_definition,
        })
        .collect()
}

/// One raw `tags.scm` match before nesting: a definition's kind and byte
/// range plus its name token's byte range, not yet organized into a tree.
struct RawSymbol {
    kind: SymbolKind,
    start: usize,
    end: usize,
    name_start: usize,
    name_end: usize,
}

/// Per-file symbol outline of `text`, parsed as `language` (Task D) — the
/// data source for Structure's per-file tier. Stateless one-shot, matching
/// [`highlight`]/[`identifier_occurrences`]'s convention: does its own
/// parse rather than reusing a [`Highlighter`]'s persistent tree. `outline`
/// is refreshed on save (a project-wide-scope panel doesn't need live
/// per-keystroke updates), so there's no incremental-tree benefit to chase
/// here the way there is for on-every-keystroke highlighting.
///
/// Backed by a `tags.scm` per language (see `crates/syntax-core/queries/
/// */tags.scm`), following the community `tree-sitter-tags` convention:
/// each pattern captures the whole definition node as `@definition.<kind>`
/// and its identifier as `@name`. Nesting (methods/fields under their
/// class, methods under an `impl` block, ...) is derived here from AST
/// byte-range containment rather than from an explicit parent capture —
/// simpler to get right than threading parent pointers through every
/// query, and correct by construction since a tree-sitter node's range
/// always fully contains its descendants' ranges. [`Language::PLAIN_TEXT`]
/// (or a language with an empty `tags.scm`, i.e. JSON) yields an empty vec.
pub fn outline(language: Language, text: &str) -> Vec<SymbolNode> {
    let Some(compiled) = registry::compiled(language) else {
        return Vec::new();
    };
    let (Some(query), Some(tree)) = (compiled.tags.as_ref(), parse_once(&compiled.grammar, text))
    else {
        return Vec::new();
    };
    outline_from_tree(query, &tree, text)
}

/// [`outline`]'s body against an already-parsed tree, so [`analyze_file`]
/// can reuse one parse across all three extractions.
fn outline_from_tree(query: &Query, tree: &tree_sitter::Tree, text: &str) -> Vec<SymbolNode> {
    count_query_walk();
    let mut raw: Vec<RawSymbol> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let mut definition: Option<(SymbolKind, usize, usize)> = None;
        let mut name: Option<(usize, usize)> = None;
        for capture in m.captures {
            let capture_name = query.capture_names()[capture.index as usize];
            if capture_name == "name" {
                name = Some((capture.node.start_byte(), capture.node.end_byte()));
            } else if let Some(kind) = symbol_kind_for_capture(capture_name) {
                definition = Some((kind, capture.node.start_byte(), capture.node.end_byte()));
            }
        }
        if let (Some((kind, start, end)), Some((name_start, name_end))) = (definition, name) {
            raw.push(RawSymbol {
                kind,
                start,
                end,
                name_start,
                name_end,
            });
        }
    }

    build_symbol_tree(raw, text)
}

/// Every "subtype declares supertype" edge in `text`, parsed as
/// `language` -- the data behind Go to Implementation (which types declare
/// this supertype?) and its inverse, Go to Interface. Stateless one-shot,
/// same convention as [`outline`].
///
/// Backed by an `inherits.scm` per language (see `crates/syntax-core/
/// queries/*/inherits.scm`) capturing the declaring type's name token as
/// `@type` and one declared supertype's name token as `@supertype`. A
/// declaration listing several supertypes matches the pattern once per
/// supertype, so each pair arrives as its own edge with no extra work
/// here. [`Language::PLAIN_TEXT`] (or a language with an empty
/// `inherits.scm`, i.e. JSON) yields an empty vec.
///
/// Name-based like the rest of this crate (ADR-0008): an edge records the
/// supertype's *written name*, not a resolved type -- `implements
/// Comparable` and a same-named interface in another namespace are
/// indistinguishable here by design.
pub fn supertype_edges(language: Language, text: &str) -> Vec<SupertypeEdge> {
    let Some(compiled) = registry::compiled(language) else {
        return Vec::new();
    };
    let (Some(query), Some(tree)) = (
        compiled.inherits.as_ref(),
        parse_once(&compiled.grammar, text),
    ) else {
        return Vec::new();
    };
    supertype_edges_from_tree(query, &tree, text)
}

/// [`supertype_edges`]'s body against an already-parsed tree, so
/// [`analyze_file`] can reuse one parse across all three extractions.
fn supertype_edges_from_tree(
    query: &Query,
    tree: &tree_sitter::Tree,
    text: &str,
) -> Vec<SupertypeEdge> {
    count_query_walk();
    let mut edges: Vec<SupertypeEdge> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let mut type_span: Option<(usize, usize)> = None;
        let mut supertype_span: Option<(usize, usize)> = None;
        for capture in m.captures {
            match query.capture_names()[capture.index as usize] {
                "type" => type_span = Some((capture.node.start_byte(), capture.node.end_byte())),
                "supertype" => {
                    supertype_span = Some((capture.node.start_byte(), capture.node.end_byte()))
                }
                _ => {}
            }
        }
        if let (Some((ts, te)), Some((ss, se))) = (type_span, supertype_span) {
            edges.push(SupertypeEdge {
                type_name: text[ts..te].to_string(),
                supertype_name: text[ss..se].to_string(),
                type_start: ts,
                type_end: te,
            });
        }
    }
    edges.sort_by_key(|e| (e.type_start, e.supertype_name.clone()));
    edges
}

/// How many extraction query walks this process has run, for tests that
/// need to assert *which* work a code path did rather than how long it
/// took (see index-core's go-to-definition early-out). A relaxed counter
/// bumped once per query walk — not per match — so it costs nothing on
/// the hot paths it measures.
#[doc(hidden)]
pub static QUERY_WALKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_query_walk() {
    QUERY_WALKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// One parsed buffer whose extraction queries the caller drives
/// individually — parse once, then ask for outline / occurrences /
/// supertype edges only as each is actually needed.
///
/// [`analyze_file`] is the "I need all three" shorthand over this. Prefer
/// the handle wherever a cheap query can decide that the expensive ones
/// are pointless: go-to-definition looks at [`Self::occurrences`] first
/// and, if the caret is not on an identifier, returns without ever
/// running the `tags` walk. Each query walk costs roughly as much as the
/// parse itself, so skipping one is not a micro-optimisation.
pub struct ParsedFile<'text> {
    compiled: Arc<CompiledLanguage>,
    tree: tree_sitter::Tree,
    text: &'text str,
}

impl<'text> ParsedFile<'text> {
    /// Parse `text` as `language`. `None` when the language has no grammar
    /// (plain text, a query that would not compile) or the parse fails —
    /// the same conditions under which the one-shot entry points return
    /// empty results.
    pub fn parse(language: Language, text: &'text str) -> Option<Self> {
        let compiled = registry::compiled(language)?;
        let tree = parse_once(&compiled.grammar, text)?;
        Some(Self {
            compiled,
            tree,
            text,
        })
    }

    /// Same result as [`outline`], without re-parsing.
    pub fn outline(&self) -> Vec<SymbolNode> {
        self.compiled
            .tags
            .as_ref()
            .map(|q| outline_from_tree(q, &self.tree, self.text))
            .unwrap_or_default()
    }

    /// Same result as [`identifier_occurrences`], without re-parsing.
    pub fn occurrences(&self) -> Vec<Occurrence> {
        self.compiled
            .locals
            .as_ref()
            .map(|q| occurrences_from_tree(q, &self.tree, self.text))
            .unwrap_or_default()
    }

    /// Same result as [`supertype_edges`], without re-parsing.
    pub fn supertype_edges(&self) -> Vec<SupertypeEdge> {
        self.compiled
            .inherits
            .as_ref()
            .map(|q| supertype_edges_from_tree(q, &self.tree, self.text))
            .unwrap_or_default()
    }
}

/// Everything [`index-core`](../index_core/index.html) extracts from one
/// file, from a single parse: what [`outline`],
/// [`identifier_occurrences`] and [`supertype_edges`] each return, but
/// with the buffer parsed once instead of three times.
pub struct FileAnalysis {
    pub outline: Vec<SymbolNode>,
    pub occurrences: Vec<Occurrence>,
    pub supertype_edges: Vec<SupertypeEdge>,
}

/// Parse `text` as `language` once and run all three extraction queries
/// against that one tree. Equivalent to calling [`outline`],
/// [`identifier_occurrences`] and [`supertype_edges`] separately -- same
/// results, a third of the parsing. Each field is empty when its query is
/// absent for the language, and all three are empty when the language has
/// no grammar or the parse fails, matching the individual entry points.
pub fn analyze_file(language: Language, text: &str) -> FileAnalysis {
    let Some(parsed) = ParsedFile::parse(language, text) else {
        return FileAnalysis {
            outline: Vec::new(),
            occurrences: Vec::new(),
            supertype_edges: Vec::new(),
        };
    };
    FileAnalysis {
        outline: parsed.outline(),
        occurrences: parsed.occurrences(),
        supertype_edges: parsed.supertype_edges(),
    }
}

/// Nests `raw` definitions by AST byte-range containment: a classic
/// "build a tree from ranges" stack scan. `raw` is sorted by start byte
/// (ties broken by longest-range-first, so an outer definition is always
/// pushed before an inner one that starts at the same byte — e.g. a
/// struct with no leading trivia and its first field). While the next
/// definition still starts before the stack top's end, it nests inside;
/// once a definition starts at or past the top's end, the top is closed
/// out (attached to its own parent, or promoted to a root) and popped.
/// This is correct by construction — not a heuristic — because tree-sitter
/// node ranges never partially overlap, only nest or sit disjoint.
fn build_symbol_tree(mut raw: Vec<RawSymbol>, text: &str) -> Vec<SymbolNode> {
    raw.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));

    let mut roots: Vec<SymbolNode> = Vec::new();
    let mut open: Vec<SymbolNode> = Vec::new();

    fn attach(open: &mut [SymbolNode], roots: &mut Vec<SymbolNode>, node: SymbolNode) {
        match open.last_mut() {
            Some(parent) => parent.children.push(node),
            None => roots.push(node),
        }
    }

    for r in raw {
        while let Some(top) = open.last() {
            if top.end <= r.start {
                let done = open.pop().expect("just peeked Some");
                attach(&mut open, &mut roots, done);
            } else {
                break;
            }
        }
        open.push(SymbolNode {
            name: text[r.name_start..r.name_end].to_string(),
            kind: r.kind,
            start: r.start,
            end: r.end,
            name_start: r.name_start,
            name_end: r.name_end,
            children: Vec::new(),
        });
    }
    while let Some(done) = open.pop() {
        attach(&mut open, &mut roots, done);
    }

    roots
}

/// True when tree-sitter parsed a predicate on `pattern` that it does not
/// itself evaluate, so the pattern would match *unguarded*.
///
/// `QueryCursor::matches` filters on the standard text predicates
/// (`#eq?`, `#not-eq?`, `#match?`, `#not-match?`, `#any-of?`,
/// `#not-any-of?` and their `#any-*` variants) — that is where predicate
/// evaluation actually happens, and it is why guarded patterns are safe to
/// ship. Two other kinds are parsed but never applied:
///
///   * property predicates — `#is? local` / `#is-not? local`, which depend
///     on a locals-scope resolver this crate does not have;
///   * general predicates — anything tree-sitter does not know at all,
///     which is where nvim-treesitter's `#lua-match?`, `#has-ancestor?`
///     and friends land.
///
/// A pattern carrying one of those is *dropped*, not silently unguarded.
/// Failing closed is the only safe default: Ruby's `(identifier) @method
/// (#is-not? local)` unguarded paints every identifier in the file. It
/// also makes pasting an upstream `.scm` file safe — an unsupported guard
/// costs that one pattern's highlighting, never a wrong repaint of the
/// whole document.
///
/// `#set!` is a directive, not a predicate; it lands in the query's
/// property *settings* and is simply ignored here.
fn pattern_is_guarded_by_an_unevaluated_predicate(query: &Query, pattern: usize) -> bool {
    !query.property_predicates(pattern).is_empty() || !query.general_predicates(pattern).is_empty()
}

/// Document order for a highlight stream: by `start`, and on a tie the
/// wider span first. The view applies formats in stream order, so the
/// narrower — more specific — span has to come last to stay visible: an
/// `@markup.heading` over `# Title` must not repaint the `#` its own
/// `@punctuation.special` claimed.
fn sort_spans(spans: &mut [HighlightSpan]) {
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
}

/// `capture_scopes` is the query's capture index -> [`Scope`] table,
/// resolved once when the query was compiled — the per-span hot path only
/// indexes it.
///
/// Overlapping captures on the *same* node resolve first-pattern-wins, the
/// standard tree-sitter highlighting convention: a file lists its specific
/// patterns first and its naming-convention catch-alls (CamelCase is a
/// type, SCREAMING_CASE is a constant) last, and the specific one wins.
/// Without this the view would paint whichever span happened to be sorted
/// last, since it applies formats in order and the later write wins.
fn spans_from_tree(
    query: &Query,
    capture_scopes: &[Option<Scope>],
    tree: &tree_sitter::Tree,
    text: &str,
) -> Vec<HighlightSpan> {
    // (span, pattern index) — the pattern index is only needed to break
    // same-node ties and is dropped again below.
    let mut spans: Vec<(HighlightSpan, usize)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        if pattern_is_guarded_by_an_unevaluated_predicate(query, m.pattern_index) {
            continue;
        }
        for capture in m.captures {
            if let Some(Some(scope)) = capture_scopes.get(capture.index as usize) {
                spans.push((
                    HighlightSpan {
                        start: capture.node.start_byte(),
                        end: capture.node.end_byte(),
                        scope: *scope,
                    },
                    m.pattern_index,
                ));
            }
        }
    }
    // Widest first on a tie, so the narrower — more specific — span is
    // applied last and stays visible; among spans over the *same* node,
    // the earlier pattern wins.
    spans.sort_by(|(a, ap), (b, bp)| {
        a.start
            .cmp(&b.start)
            .then(b.end.cmp(&a.end))
            .then(ap.cmp(bp))
    });
    spans.dedup_by_key(|(span, _)| (span.start, span.end));
    spans.into_iter().map(|(span, _)| span).collect()
}

/// How deep injected regions are followed: the host document is depth 0,
/// and a tree is only asked for its own injections while its depth is
/// below this bound. So up to three nested injected trees are parsed
/// (Markdown → HTML → CSS is exactly the limit), and a fourth level is
/// left unhighlighted rather than followed.
///
/// A bound, not recursion to exhaustion: injection queries are data —
/// a runtime grammar (G1a/G1b) or a hostile file can describe a region
/// that injects itself, and an unbounded walk on that input hangs the
/// editor. Three levels covers every real nesting anyone has reported.
pub const MAX_INJECTION_DEPTH: usize = 3;

/// Documents larger than this are not highlighted at all: [`Highlighter`]
/// keeps the text, skips the parse, and returns no spans.
///
/// A bound of the same kind as [`MAX_INJECTION_DEPTH`], for the same
/// reason — the input decides the cost. Highlighting is whole-document
/// (the query runs over the whole tree, and every injected region is
/// re-parsed from scratch), so a multi-megabyte machine-generated file
/// with a `<script>` in it pays a full JavaScript parse of the bundle
/// inside it, synchronously, before the first paint. Colour on a file that
/// large is worth less than the editor staying responsive, which is the
/// same trade every other editor makes at roughly this size.
pub const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;

/// Lines longer than this get no spans, even in a document under
/// [`MAX_HIGHLIGHT_BYTES`].
///
/// A line this long is minified or generated, so per-token colour conveys
/// nothing, and it is exactly where the cost lands: the view sets one
/// character format per span, and thousands of formats on one text block
/// make that block expensive to lay out every time it is scrolled into
/// view. The value matches VS Code's `editor.maxTokenizationLineLength`.
pub const MAX_HIGHLIGHT_LINE_BYTES: usize = 20_000;

/// One injected region found by an `injections.scm` match: the language
/// it is written in, and the byte ranges of its `@injection.content`
/// captures. Several ranges in one match are parsed as *one* tree (that
/// is what `Parser::set_included_ranges` is for), so a language split
/// across several nodes still sees one continuous document.
struct InjectionRegion {
    language: String,
    ranges: Vec<tree_sitter::Range>,
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
    fn contains(&self, span: &HighlightSpan) -> bool {
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
fn injection_regions(query: &Query, tree: &tree_sitter::Tree, text: &str) -> Vec<InjectionRegion> {
    let capture_names = query.capture_names();
    let mut regions = Vec::new();
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
        // `set_included_ranges` rejects ranges that are not ascending and
        // disjoint, and query captures arrive in match order, not
        // document order.
        ranges.sort_by_key(|r| r.start_byte);
        ranges.dedup_by_key(|r| r.start_byte);
        regions.push(InjectionRegion { language, ranges });
    }
    regions
}

/// Highlight spans for `tree` plus every language injected into it, as one
/// stream sorted by `(start, end)`.
///
/// The sort is a contract, not a convenience: the view binary-searches
/// this stream by `start` (`syntax_highlighter.cpp`), so an out-of-order
/// span silently mis-colours the document rather than failing loudly.
/// Ties on `start` are ordered widest-first, because the view applies
/// formats in stream order and the narrower, more specific span has to be
/// the one that ends up visible.
///
/// Injected spans win: a host span *inside* an injected region is dropped
/// before the injected spans are merged in, so `<script>`'s JS is coloured
/// as JS and not left under whatever the host grammar made of it. A host
/// span that *encloses* an injected region survives (a Markdown heading
/// encloses the inline run it is made of) and is sorted first, so the view
/// paints it and then paints the injected spans over it.
///
/// `resolve` maps an injected language name onto a compiled language. It
/// is a parameter rather than a direct registry call so the recursion can
/// be driven by synthetic languages in tests — [`MAX_INJECTION_DEPTH`] is
/// only worth having if something proves it holds.
fn spans_with_injections(
    compiled: &CompiledLanguage,
    tree: &tree_sitter::Tree,
    text: &str,
    depth: usize,
    resolve: &dyn Fn(&str) -> Option<Arc<CompiledLanguage>>,
) -> Vec<HighlightSpan> {
    let mut spans = compiled.highlights.as_ref().map_or_else(Vec::new, |query| {
        spans_from_tree(query, &compiled.highlight_scopes, tree, text)
    });
    let regions = match compiled.injections.as_ref() {
        Some(query) if depth < MAX_INJECTION_DEPTH => injection_regions(query, tree, text),
        _ => Vec::new(),
    };
    if regions.is_empty() {
        return spans;
    }
    spans.retain(|span| !regions.iter().any(|region| region.contains(span)));
    for region in regions {
        let Some(inner) = resolve(&region.language) else {
            continue;
        };
        let mut parser = Parser::new();
        if parser.set_language(&inner.grammar).is_err()
            || parser.set_included_ranges(&region.ranges).is_err()
        {
            continue;
        }
        // `set_included_ranges` over the *whole* document, not a
        // substring: byte offsets in the sub-tree are already document
        // offsets, so nothing has to be shifted back afterwards.
        let Some(subtree) = parser.parse(text, None) else {
            continue;
        };
        spans.extend(spans_with_injections(
            &inner,
            &subtree,
            text,
            depth + 1,
            resolve,
        ));
    }
    sort_spans(&mut spans);
    spans
}

/// Byte ranges of the lines in `text` that are longer than
/// [`MAX_HIGHLIGHT_LINE_BYTES`], in ascending order. The line terminator is
/// not counted toward a line's length.
fn overlong_line_ranges(text: &str) -> Vec<Range<usize>> {
    // No line can exceed the cap if the whole document doesn't, which is
    // every ordinary source file — so the common case never scans.
    if text.len() <= MAX_HIGHLIGHT_LINE_BYTES {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (i, &byte) in text.as_bytes().iter().enumerate() {
        if byte == b'\n' {
            if i - start > MAX_HIGHLIGHT_LINE_BYTES {
                ranges.push(start..i);
            }
            start = i + 1;
        }
    }
    if text.len() - start > MAX_HIGHLIGHT_LINE_BYTES {
        ranges.push(start..text.len());
    }
    ranges
}

/// Drop every span that *begins* inside a line over
/// [`MAX_HIGHLIGHT_LINE_BYTES`].
///
/// By where it starts, not by overlap: a span that opens on an ordinary
/// line and runs into a long one (a multi-line string, say) is one format,
/// which costs nothing. What this exists to remove is the thousands of
/// spans a minified line generates entirely within itself.
fn drop_spans_on_long_lines(spans: &mut Vec<HighlightSpan>, text: &str) {
    let long = overlong_line_ranges(text);
    if long.is_empty() {
        return;
    }
    spans.retain(|span| {
        long.binary_search_by(|line| {
            if line.end <= span.start {
                std::cmp::Ordering::Less
            } else if line.start > span.start {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
    });
}

/// Byte offset `offset` within `text`, expressed as a tree-sitter
/// [`tree_sitter::Point`] (row, byte-column-within-row) — the coordinate
/// `InputEdit` needs alongside byte offsets. `offset` is clamped to
/// `text.len()`.
fn point_at(text: &str, offset: usize) -> tree_sitter::Point {
    let bytes = text.as_bytes();
    let offset = offset.min(bytes.len());
    let mut row = 0usize;
    let mut line_start = 0usize;
    for (i, &b) in bytes[..offset].iter().enumerate() {
        if b == b'\n' {
            row += 1;
            line_start = i + 1;
        }
    }
    tree_sitter::Point {
        row,
        column: offset - line_start,
    }
}

/// Stateful, incremental syntax highlighter: keeps a persistent
/// `tree_sitter::Tree` per instance (one per open document/tab, on the
/// caller's side) and reparses incrementally via `Tree::edit` +
/// `Parser::parse(text, Some(&old_tree))` instead of a fresh whole-buffer
/// parse on every change (upgrade over the v1 ceiling documented on
/// [`highlight`]'s predecessor — decision A6/A1).
///
/// [`Language::PLAIN_TEXT`] is a valid, cheap no-op: `set_text`/`edit` just
/// track the text and return no spans, so callers don't need to special
/// case unrecognized extensions.
pub struct Highlighter {
    /// Held as an `Arc`, not looked up per call: a registry reload must
    /// not invalidate an open editor's grammar mid-session.
    compiled: Option<Arc<CompiledLanguage>>,
    parser: Option<Parser>,
    tree: Option<tree_sitter::Tree>,
    text: String,
}

impl Highlighter {
    /// Create a highlighter for `language`. Cheap: the query/grammar are
    /// process-wide, compiled once and shared behind an `Arc`, so this only
    /// allocates a `Parser` and an empty text buffer.
    pub fn new(language: Language) -> Self {
        let compiled = registry::compiled(language);
        let parser = compiled.as_ref().and_then(|compiled| {
            let mut parser = Parser::new();
            parser.set_language(&compiled.grammar).ok()?;
            Some(parser)
        });
        Self {
            compiled,
            parser,
            tree: None,
            text: String::new(),
        }
    }

    /// Full (re)parse of `text`, discarding any previous incremental tree.
    /// Use for initial load; use [`Highlighter::edit`] for subsequent
    /// changes to get incremental reparsing.
    pub fn set_text(&mut self, text: &str) -> Vec<HighlightSpan> {
        self.tree = None;
        self.text = text.to_string();
        self.reparse()
    }

    /// Apply one contiguous byte-range replace and reparse incrementally.
    ///
    /// `new_text` is the *entire* new document text. `start_byte..
    /// old_end_byte` is the byte range being replaced in the *previous*
    /// text (as passed to the last `set_text`/`edit` call); `start_byte..
    /// new_end_byte` is the corresponding range in `new_text`. This is the
    /// standard tree-sitter `InputEdit` shape, expressed as byte offsets
    /// only — row/column `Point`s are derived internally from the old and
    /// new text so callers don't need to track them.
    pub fn edit(
        &mut self,
        new_text: &str,
        start_byte: usize,
        old_end_byte: usize,
        new_end_byte: usize,
    ) -> Vec<HighlightSpan> {
        if let Some(tree) = self.tree.as_mut() {
            let edit = tree_sitter::InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position: point_at(&self.text, start_byte),
                old_end_position: point_at(&self.text, old_end_byte),
                new_end_position: point_at(new_text, new_end_byte),
            };
            tree.edit(&edit);
        }
        self.text = new_text.to_string();
        self.reparse()
    }

    /// Reparse the host tree incrementally, then re-derive every injected
    /// region from scratch.
    ///
    /// Only the host tree is persistent. Injected sub-trees are rebuilt on
    /// every reparse, because an edit can move, split, delete or create a
    /// region outright (typing `</script>` re-languages everything after
    /// it), and a sub-tree kept across that is not stale in a way the user
    /// forgives — it is the wrong language on screen. The cost is one full
    /// parse per injected region per keystroke, bounded by how much of the
    /// document is injected; a document with no `injections.scm` pays
    /// nothing at all and behaves exactly as before.
    ///
    /// ponytail: re-derive per edit; make injected sub-trees incremental
    /// (edit each, keyed by region identity) only if profiling on a real
    /// HTML/Markdown file says it matters.
    fn reparse(&mut self) -> Vec<HighlightSpan> {
        // Both size ceilings are enforced here rather than in `set_text`
        // and `edit` separately: this is the one path both take, and it is
        // the path whose cost they bound. Dropping the tree matters as much
        // as returning no spans — `fold_ranges` reads off it, so a document
        // over budget also stops paying for folds.
        if self.text.len() > MAX_HIGHLIGHT_BYTES {
            self.tree = None;
            return Vec::new();
        }
        let Some(compiled) = self.compiled.clone() else {
            return Vec::new();
        };
        let Some(parser) = self.parser.as_mut() else {
            return Vec::new();
        };
        let Some(new_tree) = parser.parse(&self.text, self.tree.as_ref()) else {
            return Vec::new();
        };
        // Resolved through the registry per reparse rather than cached: a
        // lookup is an `Arc` clone plus a `OnceLock` read, and reading
        // through means a live registry reload (G2) also reaches injected
        // grammars. The host grammar stays pinned, as decision 3 requires.
        let mut spans = spans_with_injections(&compiled, &new_tree, &self.text, 0, &|id| {
            language_by_id(id).and_then(registry::compiled)
        });
        drop_spans_on_long_lines(&mut spans, &self.text);
        self.tree = Some(new_tree);
        spans
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn lang(id: &str) -> Language {
        language_by_id(id).expect("catalog language")
    }
    fn rust() -> Language {
        lang("rust")
    }
    fn json() -> Language {
        lang("json")
    }
    fn csharp() -> Language {
        lang("csharp")
    }
    fn java() -> Language {
        lang("java")
    }
    fn php() -> Language {
        lang("php")
    }

    fn scope(name: &str) -> Scope {
        Scope::resolve(name).unwrap_or_else(|| panic!("no such scope: {name}"))
    }

    fn find<'a>(spans: &'a [HighlightSpan], name: &str) -> Option<&'a HighlightSpan> {
        let wanted = scope(name);
        spans.iter().find(|s| s.scope == wanted)
    }

    #[test]
    fn analyze_file_matches_the_three_separate_entry_points() {
        let rust_source = r#"
            pub trait Greeter { fn greet(&self) -> String; }
            pub struct Loud { volume: u8 }
            impl Greeter for Loud {
                fn greet(&self) -> String { let n = self.volume; format!("{n}") }
            }
        "#;
        for (language, source) in [
            (rust(), rust_source),
            (json(), "{\"a\": [1, 2]}"),
            (Language::PLAIN_TEXT, "just words"),
        ] {
            let combined = analyze_file(language, source);
            assert_eq!(combined.outline, outline(language, source));
            assert_eq!(
                combined.occurrences,
                identifier_occurrences(language, source)
            );
            assert_eq!(combined.supertype_edges, supertype_edges(language, source));
        }
        // Not vacuous: the Rust fixture really does exercise all three.
        let combined = analyze_file(rust(), rust_source);
        assert!(!combined.outline.is_empty());
        assert!(!combined.occurrences.is_empty());
        assert!(!combined.supertype_edges.is_empty());
    }

    #[test]
    fn scope_names_are_unique_and_every_dotted_scope_has_its_root() {
        let mut sorted: Vec<&str> = SCOPES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SCOPES.len(), "duplicate scope name");
        for name in SCOPES {
            if let Some((parent, _)) = name.rsplit_once('.') {
                assert!(
                    SCOPES.contains(&parent),
                    "{name} has no parent scope {parent}"
                );
            }
        }
    }

    #[test]
    fn markdown_markup_captures_resolve_to_the_markup_family() {
        let source = "# Title\n\nA [label](https://example.com) and *emphasis*.\n";
        let spans = highlight(lang("markdown"), source);
        let text_of = |name: &str| {
            spans
                .iter()
                .find(|span| span.scope.name() == name)
                .map(|span| &source[span.start..span.end])
        };
        assert_eq!(text_of("markup.heading.1"), Some("# Title\n"));
        assert_eq!(text_of("markup.link.label"), Some("label"));
        assert_eq!(text_of("markup.link.url"), Some("https://example.com"));
        assert_eq!(text_of("markup.italic"), Some("*emphasis*"));
    }

    #[test]
    fn an_unknown_capture_falls_back_up_its_dotted_path() {
        // Exact hit.
        assert_eq!(scope("function.method").name(), "function.method");
        // One level up: `function.method` is in the table, its `.static`
        // refinement is not.
        assert_eq!(scope("function.method.static").name(), "function.method");
        // All the way to the root: no `keyword.*` entry exists at all.
        assert_eq!(scope("keyword.conditional.ternary").name(), "keyword");
    }

    #[test]
    fn a_fully_unknown_capture_is_dropped() {
        assert_eq!(Scope::resolve("nonsense"), None);
        assert_eq!(Scope::resolve("nonsense.and.more"), None);
        assert_eq!(Scope::resolve(""), None);
    }

    #[test]
    fn extension_maps_to_language() {
        assert_eq!(language_for_path(Path::new("a.rs")), rust());
        assert_eq!(language_for_path(Path::new("a.json")), json());
        assert_eq!(language_for_path(Path::new("a.cs")), csharp());
        assert_eq!(language_for_path(Path::new("a.java")), java());
        assert_eq!(language_for_path(Path::new("a.php")), php());
        assert_eq!(language_for_path(Path::new("a.txt")), Language::PLAIN_TEXT);
        assert_eq!(language_for_path(Path::new("a")), Language::PLAIN_TEXT);
    }

    #[test]
    fn language_name_covers_every_language() {
        assert_eq!(language_name(rust()), "Rust");
        assert_eq!(language_name(json()), "JSON");
        assert_eq!(language_name(csharp()), "C#");
        assert_eq!(language_name(java()), "Java");
        assert_eq!(language_name(php()), "PHP");
        assert_eq!(language_name(Language::PLAIN_TEXT), "Plain Text");
    }

    #[test]
    fn plain_text_yields_no_spans() {
        assert!(highlight(Language::PLAIN_TEXT, "fn foo() {}").is_empty());
    }

    #[test]
    fn rust_fn_keyword_is_highlighted() {
        let text = "fn foo() {}";
        let spans = highlight(rust(), text);
        let span = find(&spans, "keyword").expect("expected a Keyword span");
        assert_eq!(&text[span.start..span.end], "fn");
    }

    #[test]
    fn rust_function_name_is_highlighted() {
        let text = "fn foo() {}";
        let spans = highlight(rust(), text);
        let span = find(&spans, "function").expect("expected a Function span");
        assert_eq!(&text[span.start..span.end], "foo");
    }

    #[test]
    fn rust_string_literal_is_highlighted() {
        let text = "fn foo() { let s = \"hi\"; }";
        let spans = highlight(rust(), text);
        let span = find(&spans, "string").expect("expected a String span");
        assert_eq!(&text[span.start..span.end], "\"hi\"");
    }

    #[test]
    fn rust_comment_is_highlighted() {
        let text = "fn foo() { // hello\n}";
        let spans = highlight(rust(), text);
        let span = find(&spans, "comment").expect("expected a Comment span");
        assert!(&text[span.start..span.end].starts_with("// hello"));
    }

    #[test]
    fn rust_number_is_highlighted() {
        let text = "fn foo() { let x = 42; }";
        let spans = highlight(rust(), text);
        let span = find(&spans, "number").expect("expected a Number span");
        assert_eq!(&text[span.start..span.end], "42");
    }

    #[test]
    fn rust_type_is_highlighted() {
        let text = "fn foo() { let x: i32 = 42; }";
        let spans = highlight(rust(), text);
        let span = find(&spans, "type").expect("expected a Type span");
        assert_eq!(&text[span.start..span.end], "i32");
    }

    #[test]
    fn json_string_key_is_highlighted() {
        let text = "{\"key\": \"value\"}";
        let spans = highlight(json(), text);
        let span = find(&spans, "string").expect("expected a String span");
        assert_eq!(&text[span.start..span.end], "\"key\"");
    }

    #[test]
    fn json_number_is_highlighted() {
        let text = "{\"n\": 42}";
        let spans = highlight(json(), text);
        let span = find(&spans, "number").expect("expected a Number span");
        assert_eq!(&text[span.start..span.end], "42");
    }

    #[test]
    fn json_boolean_is_highlighted_as_keyword() {
        let text = "{\"b\": true}";
        let spans = highlight(json(), text);
        let span = find(&spans, "keyword").expect("expected a Keyword span");
        assert_eq!(&text[span.start..span.end], "true");
    }

    #[test]
    fn spans_are_within_text_bounds() {
        let text = "fn foo() { let x: i32 = 42; \"s\"; }";
        for span in highlight(rust(), text) {
            assert!(span.start <= span.end);
            assert!(span.end <= text.len());
        }
    }

    #[test]
    fn incremental_edit_matches_a_fresh_full_reparse() {
        // "let x = 42;" -> "let xy = 42;": insert "y" after "x" (single
        // char, byte offset 8..8 -> 8..9).
        let old_text = "fn foo() { let x = 42; }";
        let new_text = "fn foo() { let xy = 42; }";

        let mut incremental = Highlighter::new(rust());
        incremental.set_text(old_text);
        let incremental_spans = incremental.edit(new_text, 16, 16, 17);

        let fresh_spans = highlight(rust(), new_text);

        assert_eq!(incremental_spans, fresh_spans_sorted(fresh_spans.clone()));
        // The number literal, well away from the edit, is still classified
        // correctly and at its new (shifted) position.
        let number =
            find(&incremental_spans, "number").expect("expected a Number span after the edit");
        assert_eq!(&new_text[number.start..number.end], "42");
    }

    fn fresh_spans_sorted(mut spans: Vec<HighlightSpan>) -> Vec<HighlightSpan> {
        sort_spans(&mut spans);
        spans
    }

    #[test]
    fn editing_inside_a_string_literal_does_not_reclassify_surrounding_code() {
        // Insert a character inside the string literal "hi" -> "hxi".
        let old_text = "fn foo() { let s = \"hi\"; let n = 1; }";
        let new_text = "fn foo() { let s = \"hxi\"; let n = 1; }";

        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(old_text);
        // Byte 21 is right after the opening quote + "h": edit "i" -> "xi".
        let spans = highlighter.edit(new_text, 21, 21, 22);

        let keyword = find(&spans, "keyword").expect("expected a Keyword span");
        assert_eq!(&new_text[keyword.start..keyword.end], "fn");

        let function = find(&spans, "function").expect("expected a Function span");
        assert_eq!(&new_text[function.start..function.end], "foo");

        let string = find(&spans, "string").expect("expected a String span");
        assert_eq!(&new_text[string.start..string.end], "\"hxi\"");

        let number = find(&spans, "number").expect("expected a Number span");
        assert_eq!(&new_text[number.start..number.end], "1");
    }

    #[test]
    fn a_document_over_the_byte_ceiling_is_not_highlighted() {
        let mut small = String::from("fn foo() { let x = 42; }\n");
        let mut highlighter = Highlighter::new(rust());
        assert!(
            !highlighter.set_text(&small).is_empty(),
            "an ordinary document must still highlight"
        );

        // Same code, repeated past MAX_HIGHLIGHT_BYTES.
        while small.len() <= MAX_HIGHLIGHT_BYTES {
            small.push_str("fn foo() { let x = 42; }\n");
        }
        assert!(highlighter.set_text(&small).is_empty());
        // The tree goes with it, so folds stop costing anything too.
        assert!(highlighter.fold_ranges().is_empty());
    }

    #[test]
    fn a_line_over_the_line_ceiling_gets_no_spans_but_its_neighbours_do() {
        let long_literal = "x".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1);
        let text = format!("fn a() {{ let n = 1; }}\nfn b() {{ let s = \"{long_literal}\"; }}\nfn c() {{ let n = 2; }}\n");

        let spans = Highlighter::new(rust()).set_text(&text);
        assert!(!spans.is_empty());

        let long_line_start = text.find("fn b()").expect("second line exists");
        let long_line_end = long_line_start
            + text[long_line_start..]
                .find('\n')
                .expect("second line is terminated");
        assert!(
            !spans
                .iter()
                .any(|span| span.start >= long_line_start && span.start < long_line_end),
            "no span may start inside the over-long line"
        );

        let third_line_start = long_line_end + 1;
        assert!(
            spans.iter().any(|span| span.start >= third_line_start),
            "the ordinary line after the long one must keep its spans"
        );
        assert!(
            spans.iter().any(|span| span.start < long_line_start),
            "the ordinary line before the long one must keep its spans"
        );
    }

    #[test]
    fn overlong_line_ranges_finds_long_lines_including_an_unterminated_last_one() {
        let long = "y".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1);
        assert!(overlong_line_ranges("short\nlines\nonly\n").is_empty());

        let text = format!("short\n{long}\nshort\n{long}");
        let ranges = overlong_line_ranges(&text);
        assert_eq!(ranges.len(), 2);
        // The terminator is not part of the line.
        assert_eq!(&text[ranges[0].clone()], long);
        assert_eq!(&text[ranges[1].clone()], long);
        assert_eq!(ranges[1].end, text.len());
    }

    #[test]
    fn highlighter_handles_plain_text_as_a_no_op() {
        let mut highlighter = Highlighter::new(Language::PLAIN_TEXT);
        assert!(highlighter.set_text("hello").is_empty());
        assert!(highlighter.edit("hello world", 5, 5, 11).is_empty());
    }

    #[test]
    fn plain_text_has_no_identifier_occurrences() {
        assert!(identifier_occurrences(Language::PLAIN_TEXT, "fn foo() {}").is_empty());
    }

    #[test]
    fn rust_function_and_parameter_are_definitions_used_twice_in_body() {
        let text = "fn add(x: i32) -> i32 { x + x }";
        let occurrences = identifier_occurrences(rust(), text);

        let by_name = |name: &str| -> Vec<&Occurrence> {
            occurrences.iter().filter(|o| o.name == name).collect()
        };

        let foo = by_name("add");
        assert_eq!(foo.len(), 1, "function name should occur once: {foo:?}");
        assert!(foo[0].is_definition);
        assert_eq!(&text[foo[0].start..foo[0].end], "add");

        let xs = by_name("x");
        assert_eq!(xs.len(), 3, "1 definition + 2 references: {xs:?}");
        let definitions: Vec<_> = xs.iter().filter(|o| o.is_definition).collect();
        let references: Vec<_> = xs.iter().filter(|o| !o.is_definition).collect();
        assert_eq!(definitions.len(), 1, "exactly one `x` is the parameter");
        assert_eq!(references.len(), 2, "both body uses of `x` are references");

        // Occurrences are in document order and byte ranges point at the
        // right substrings.
        for occurrence in &occurrences {
            assert_eq!(
                &text[occurrence.start..occurrence.end],
                occurrence.name,
                "byte range must point at the occurrence's own text"
            );
        }
        let mut starts: Vec<usize> = occurrences.iter().map(|o| o.start).collect();
        let mut sorted_starts = starts.clone();
        sorted_starts.sort_unstable();
        assert_eq!(
            starts, sorted_starts,
            "occurrences must be in document order"
        );
        starts.dedup();
    }

    #[test]
    fn rust_struct_name_is_a_definition() {
        let text = "struct Point { x: i32 }";
        let occurrences = identifier_occurrences(rust(), text);
        let point = occurrences
            .iter()
            .find(|o| o.name == "Point")
            .expect("expected an occurrence for the struct name");
        assert!(point.is_definition);
        assert_eq!(&text[point.start..point.end], "Point");
    }

    // --- C# (Task B) ---

    const CSHARP_SNIPPET: &str = "class Greeter {\n    public string Name;\n\n    public Greeter(string name) {\n        Name = name;\n    }\n\n    public string Greet() {\n        // say hi\n        return \"Hello, \" + Name;\n    }\n}\n";

    #[test]
    fn csharp_class_keyword_is_highlighted() {
        let spans = highlight(csharp(), CSHARP_SNIPPET);
        let span = find(&spans, "keyword").expect("expected a Keyword span");
        assert_eq!(&CSHARP_SNIPPET[span.start..span.end], "class");
    }

    #[test]
    fn csharp_string_literal_is_highlighted() {
        let spans = highlight(csharp(), CSHARP_SNIPPET);
        let span = find(&spans, "string").expect("expected a String span");
        assert_eq!(&CSHARP_SNIPPET[span.start..span.end], "\"Hello, \"");
    }

    #[test]
    fn csharp_comment_is_highlighted() {
        let spans = highlight(csharp(), CSHARP_SNIPPET);
        let span = find(&spans, "comment").expect("expected a Comment span");
        assert!(&CSHARP_SNIPPET[span.start..span.end].starts_with("// say hi"));
    }

    #[test]
    fn csharp_class_name_is_highlighted_as_type() {
        let spans = highlight(csharp(), CSHARP_SNIPPET);
        let type_span = spans
            .iter()
            .find(|s| s.scope == scope("type") && &CSHARP_SNIPPET[s.start..s.end] == "Greeter");
        assert!(
            type_span.is_some(),
            "expected `Greeter` highlighted as a Type"
        );
    }

    #[test]
    fn csharp_method_definition_is_recognized() {
        let occurrences = identifier_occurrences(csharp(), CSHARP_SNIPPET);
        let greet = occurrences
            .iter()
            .find(|o| o.name == "Greet")
            .expect("expected an occurrence for the method name");
        assert!(greet.is_definition);
    }

    #[test]
    fn csharp_parameter_used_twice_is_one_definition_and_one_reference() {
        let text = "class C { void M(string name) { name = name; } }";
        let occurrences = identifier_occurrences(csharp(), text);
        let names: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "name").collect();
        assert_eq!(names.len(), 3, "1 definition + 2 references: {names:?}");
        assert_eq!(names.iter().filter(|o| o.is_definition).count(), 1);
        assert_eq!(names.iter().filter(|o| !o.is_definition).count(), 2);
    }

    // --- Java (Task B) ---

    const JAVA_SNIPPET: &str = "public class Greeter {\n    private String name;\n\n    public Greeter(String name) {\n        this.name = name;\n    }\n\n    public String greet() {\n        // say hi\n        return \"Hello, \" + name;\n    }\n}\n";

    #[test]
    fn java_class_keyword_is_highlighted() {
        let spans = highlight(java(), JAVA_SNIPPET);
        let span = find(&spans, "keyword").expect("expected a Keyword span");
        assert_eq!(&JAVA_SNIPPET[span.start..span.end], "public");
    }

    #[test]
    fn java_string_literal_is_highlighted() {
        let spans = highlight(java(), JAVA_SNIPPET);
        let span = find(&spans, "string").expect("expected a String span");
        assert_eq!(&JAVA_SNIPPET[span.start..span.end], "\"Hello, \"");
    }

    #[test]
    fn java_comment_is_highlighted() {
        let spans = highlight(java(), JAVA_SNIPPET);
        let span = find(&spans, "comment").expect("expected a Comment span");
        assert!(&JAVA_SNIPPET[span.start..span.end].starts_with("// say hi"));
    }

    #[test]
    fn java_class_name_is_highlighted_as_type() {
        let spans = highlight(java(), JAVA_SNIPPET);
        let type_span = spans
            .iter()
            .find(|s| s.scope == scope("type") && &JAVA_SNIPPET[s.start..s.end] == "Greeter");
        assert!(
            type_span.is_some(),
            "expected `Greeter` highlighted as a Type"
        );
    }

    #[test]
    fn java_method_definition_is_recognized() {
        let occurrences = identifier_occurrences(java(), JAVA_SNIPPET);
        let greet = occurrences
            .iter()
            .find(|o| o.name == "greet")
            .expect("expected an occurrence for the method name");
        assert!(greet.is_definition);
    }

    #[test]
    fn java_parameter_used_twice_is_one_definition_and_one_reference() {
        let text = "class C { void m(String name) { name = name; } }";
        let occurrences = identifier_occurrences(java(), text);
        let names: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "name").collect();
        assert_eq!(names.len(), 3, "1 definition + 2 references: {names:?}");
        assert_eq!(names.iter().filter(|o| o.is_definition).count(), 1);
        assert_eq!(names.iter().filter(|o| !o.is_definition).count(), 2);
    }

    #[test]
    fn java_type_usages_are_references() {
        // `BcCheckException` in the extends clause, the field type, and the
        // `new` expression are `type_identifier` nodes in tree-sitter-java,
        // not `identifier` — every one must still be an occurrence, or Go to
        // Declaration and Rename see "no symbol under the caret" there.
        let text = "class ConfigurationException extends BcCheckException {\n  BcCheckException cause;\n  void m() { throw new BcCheckException(); }\n}";
        let occurrences = identifier_occurrences(java(), text);
        let uses: Vec<&Occurrence> = occurrences
            .iter()
            .filter(|o| o.name == "BcCheckException")
            .collect();
        assert_eq!(uses.len(), 3, "extends + field type + new: {uses:?}");
        assert!(uses.iter().all(|o| !o.is_definition));
    }

    #[test]
    fn c_struct_name_is_a_definition_and_its_usage_a_reference() {
        let text = "struct point { int x; };\nstruct point origin;";
        let occurrences = identifier_occurrences(lang("c"), text);
        let points: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "point").collect();
        assert_eq!(points.len(), 2, "1 definition + 1 usage: {points:?}");
        assert!(points[0].is_definition, "the struct tag declares the name");
        assert!(!points[1].is_definition);
    }

    #[test]
    fn cpp_class_name_is_a_definition_and_its_usage_a_reference() {
        let text = "class Shape {};\nclass Circle : public Shape {};\nShape *s;";
        let occurrences = identifier_occurrences(lang("cpp"), text);
        let shapes: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "Shape").collect();
        assert_eq!(
            shapes.len(),
            3,
            "definition + base clause + type: {shapes:?}"
        );
        assert!(shapes[0].is_definition);
        assert!(!shapes[1].is_definition);
        assert!(!shapes[2].is_definition);
    }

    #[test]
    fn php_type_positions_are_references() {
        // extends clause, parameter type, and return type are `(name)`
        // nodes PHP's deliberately non-catch-all query must list explicitly.
        let text =
            "<?php\nclass Circle extends Shape {}\nfunction f(Shape $x): Shape { return $x; }";
        let occurrences = identifier_occurrences(lang("php"), text);
        let shapes: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "Shape").collect();
        assert_eq!(
            shapes.len(),
            3,
            "extends + param type + return type: {shapes:?}"
        );
        assert!(shapes.iter().all(|o| !o.is_definition));
    }

    #[test]
    fn zig_type_usage_is_a_reference_not_a_definition() {
        // `(variable_declaration (identifier))` unanchored also matched the
        // type in `var s: Shape`, misreporting the usage as a definition.
        let text = "const Shape = struct {};\nvar s: Shape = undefined;";
        let occurrences = identifier_occurrences(lang("zig"), text);
        let shapes: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "Shape").collect();
        assert_eq!(shapes.len(), 2, "definition + type usage: {shapes:?}");
        assert!(shapes[0].is_definition);
        assert!(!shapes[1].is_definition);
    }

    #[test]
    fn kotlin_class_name_is_a_definition_and_its_usage_a_reference() {
        let text = "class Shape\nclass Circle : Shape()\nval s: Shape? = null";
        let occurrences = identifier_occurrences(lang("kotlin"), text);
        let shapes: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "Shape").collect();
        assert_eq!(shapes.len(), 3, "definition + supertype + type: {shapes:?}");
        assert!(shapes[0].is_definition);
        assert!(!shapes[1].is_definition);
        assert!(!shapes[2].is_definition);
    }

    // --- PHP (Task B) ---

    // Opens with `<?php`: the catalog row uses `LANGUAGE_PHP` (R4d), the
    // grammar for a whole template file, so a snippet without the opening
    // tag is not PHP code at all — it is markup that happens to look like
    // it. See php/injections.scm.
    const PHP_SNIPPET: &str = "<?php\nclass Greeter {\n    public string $name;\n\n    public function __construct(string $name) {\n        $this->name = $name;\n    }\n\n    public function greet(): string {\n        // say hi\n        return \"Hello, \" . $this->name;\n    }\n}\n";

    #[test]
    fn php_class_keyword_is_highlighted() {
        let spans = highlight(php(), PHP_SNIPPET);
        let span = find(&spans, "keyword").expect("expected a Keyword span");
        assert_eq!(&PHP_SNIPPET[span.start..span.end], "class");
    }

    #[test]
    fn php_string_literal_is_highlighted() {
        let spans = highlight(php(), PHP_SNIPPET);
        let span = find(&spans, "string").expect("expected a String span");
        assert_eq!(&PHP_SNIPPET[span.start..span.end], "\"Hello, \"");
    }

    #[test]
    fn php_comment_is_highlighted() {
        let spans = highlight(php(), PHP_SNIPPET);
        let span = find(&spans, "comment").expect("expected a Comment span");
        assert!(&PHP_SNIPPET[span.start..span.end].starts_with("// say hi"));
    }

    #[test]
    fn php_class_name_is_highlighted_as_type() {
        let spans = highlight(php(), PHP_SNIPPET);
        let type_span = spans
            .iter()
            .find(|s| s.scope == scope("type") && &PHP_SNIPPET[s.start..s.end] == "Greeter");
        assert!(
            type_span.is_some(),
            "expected `Greeter` highlighted as a Type"
        );
    }

    #[test]
    fn php_method_definition_is_recognized() {
        let occurrences = identifier_occurrences(php(), PHP_SNIPPET);
        let greet = occurrences
            .iter()
            .find(|o| o.name == "greet")
            .expect("expected an occurrence for the method name");
        assert!(greet.is_definition);
    }

    #[test]
    fn php_parameter_used_twice_is_one_definition_and_one_reference() {
        let text = "<?php\nfunction add($x) { return $x + $x; }";
        let occurrences = identifier_occurrences(php(), text);
        let names: Vec<&Occurrence> = occurrences.iter().filter(|o| o.name == "$x").collect();
        assert_eq!(names.len(), 3, "1 definition + 2 references: {names:?}");
        assert_eq!(names.iter().filter(|o| o.is_definition).count(), 1);
        assert_eq!(names.iter().filter(|o| !o.is_definition).count(), 2);
    }

    #[test]
    fn json_object_keys_are_references_not_definitions() {
        let text = "{\"key\": \"value\", \"other\": 1}";
        let occurrences = identifier_occurrences(json(), text);

        assert!(
            occurrences.iter().all(|o| !o.is_definition),
            "JSON has no definition sites: {occurrences:?}"
        );
        let names: Vec<&str> = occurrences.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, vec!["\"key\"", "\"other\""]);
    }

    // --- outline() (Task D) ---

    #[test]
    fn plain_text_has_no_outline() {
        assert!(outline(Language::PLAIN_TEXT, "anything").is_empty());
    }

    #[test]
    fn json_has_no_outline() {
        let text = "{\"key\": \"value\", \"nested\": {\"a\": 1}}";
        assert!(outline(json(), text).is_empty());
    }

    #[test]
    fn rust_outline_nests_fields_under_struct_and_methods_under_impl() {
        let text = "struct Point {\n    x: i32,\n    y: i32,\n}\n\nimpl Point {\n    fn new() -> Point { Point { x: 0, y: 0 } }\n    fn dist(&self) -> f64 { 0.0 }\n}\n";
        let roots = outline(rust(), text);

        let point_struct = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Struct && s.name == "Point")
            .expect("expected a Point struct root");
        let field_names: Vec<&str> = point_struct
            .children
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(field_names, vec!["x", "y"]);
        assert!(point_struct
            .children
            .iter()
            .all(|c| c.kind == SymbolKind::Field));

        let point_impl = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Point")
            .expect("expected a Point impl root");
        let method_names: Vec<&str> = point_impl
            .children
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(method_names, vec!["new", "dist"]);
        assert!(point_impl
            .children
            .iter()
            .all(|c| c.kind == SymbolKind::Function));

        // Name byte ranges point at just the identifier, not the whole
        // definition.
        assert_eq!(
            &text[point_struct.name_start..point_struct.name_end],
            "Point"
        );
    }

    #[test]
    fn csharp_outline_nests_methods_under_class() {
        let occurrences = outline(csharp(), CSHARP_SNIPPET);
        let greeter = occurrences
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Greeter")
            .expect("expected a Greeter class root");
        let names: Vec<&str> = greeter.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Name", "Greeter", "Greet"]);
        assert_eq!(greeter.children[0].kind, SymbolKind::Field);
        assert_eq!(greeter.children[1].kind, SymbolKind::Constructor);
        assert_eq!(greeter.children[2].kind, SymbolKind::Method);
    }

    #[test]
    fn csharp_outline_captures_auto_properties_as_properties() {
        const SNIPPET: &str = "class Person {\n    public string Name { get; set; }\n}\n";
        let roots = outline(csharp(), SNIPPET);
        let person = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Person")
            .expect("expected a Person class root");
        assert_eq!(person.children.len(), 1);
        assert_eq!(person.children[0].kind, SymbolKind::Property);
        assert_eq!(person.children[0].name, "Name");
    }

    #[test]
    fn csharp_outline_captures_enum_members() {
        const SNIPPET: &str = "enum Color {\n    Red,\n    Green,\n}\n";
        let roots = outline(csharp(), SNIPPET);
        let color = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Enum && s.name == "Color")
            .expect("expected a Color enum root");
        let names: Vec<&str> = color.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Red", "Green"]);
        assert!(color
            .children
            .iter()
            .all(|c| c.kind == SymbolKind::EnumMember));
    }

    #[test]
    fn java_outline_nests_methods_under_class() {
        let roots = outline(java(), JAVA_SNIPPET);
        let greeter = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Greeter")
            .expect("expected a Greeter class root");
        let names: Vec<&str> = greeter.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["name", "Greeter", "greet"]);
        assert_eq!(greeter.children[0].kind, SymbolKind::Field);
        assert_eq!(greeter.children[1].kind, SymbolKind::Constructor);
        assert_eq!(greeter.children[2].kind, SymbolKind::Method);
    }

    #[test]
    fn java_outline_captures_enum_constants() {
        const SNIPPET: &str = "enum Color {\n    RED,\n    GREEN;\n}\n";
        let roots = outline(java(), SNIPPET);
        let color = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Enum && s.name == "Color")
            .expect("expected a Color enum root");
        let names: Vec<&str> = color.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["RED", "GREEN"]);
        assert!(color
            .children
            .iter()
            .all(|c| c.kind == SymbolKind::EnumMember));
    }

    #[test]
    fn php_outline_nests_methods_under_class() {
        let roots = outline(php(), PHP_SNIPPET);
        let greeter = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Greeter")
            .expect("expected a Greeter class root");
        let names: Vec<&str> = greeter.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["$name", "__construct", "greet"]);
        assert_eq!(greeter.children[0].kind, SymbolKind::Property);
        assert_eq!(greeter.children[1].kind, SymbolKind::Method);
        assert_eq!(greeter.children[2].kind, SymbolKind::Method);
    }

    #[test]
    fn php_outline_captures_class_constants() {
        const SNIPPET: &str = "<?php\nclass Greeter {\n    const GREETING = \"hi\";\n}\n";
        let roots = outline(php(), SNIPPET);
        let greeter = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Greeter")
            .expect("expected a Greeter class root");
        assert_eq!(greeter.children.len(), 1);
        assert_eq!(greeter.children[0].kind, SymbolKind::Constant);
        assert_eq!(greeter.children[0].name, "GREETING");
    }

    #[test]
    fn rust_outline_captures_const_and_static_items() {
        let text = "const MAX: i32 = 10;\nstatic NAME: &str = \"x\";\n";
        let roots = outline(rust(), text);
        let names: Vec<&str> = roots.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["MAX", "NAME"]);
        assert!(roots.iter().all(|s| s.kind == SymbolKind::Constant));
    }

    #[test]
    fn go_outline_captures_const_declarations() {
        let text = "package main\n\nconst MaxItems = 10\n";
        let roots = outline(lang("go"), text);
        let max_items = roots
            .iter()
            .find(|s| s.name == "MaxItems")
            .expect("expected a MaxItems constant root");
        assert_eq!(max_items.kind, SymbolKind::Constant);
    }

    #[test]
    fn typescript_outline_captures_properties_and_enum_members() {
        let text = "class Point {\n    x: number;\n}\nenum Color {\n    Red,\n    Green = 2,\n}\n";
        let roots = outline(lang("typescript"), text);
        let point = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Point")
            .expect("expected a Point class root");
        assert_eq!(point.children[0].kind, SymbolKind::Property);
        assert_eq!(point.children[0].name, "x");

        let color = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Enum && s.name == "Color")
            .expect("expected a Color enum root");
        let names: Vec<&str> = color.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Red", "Green"]);
        assert!(color
            .children
            .iter()
            .all(|c| c.kind == SymbolKind::EnumMember));
    }

    #[test]
    fn javascript_outline_captures_class_fields_as_properties() {
        let text = "class Point {\n    x = 0;\n}\n";
        let roots = outline(lang("javascript"), text);
        let point = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Point")
            .expect("expected a Point class root");
        assert_eq!(point.children[0].kind, SymbolKind::Property);
        assert_eq!(point.children[0].name, "x");
    }

    #[test]
    fn kotlin_outline_captures_properties_and_enum_entries() {
        let text = "class Point {\n    val x: Int = 0\n}\nenum class Color {\n    RED, GREEN\n}\n";
        let roots = outline(lang("kotlin"), text);
        let point = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Point")
            .expect("expected a Point class root");
        assert_eq!(point.children[0].kind, SymbolKind::Property);
        assert_eq!(point.children[0].name, "x");

        let color = roots
            .iter()
            .find(|s| s.kind == SymbolKind::Class && s.name == "Color")
            .expect("expected a Color enum-class root");
        let names: Vec<&str> = color.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["RED", "GREEN"]);
        assert!(color
            .children
            .iter()
            .all(|c| c.kind == SymbolKind::EnumMember));
    }

    // --- supertype edges (Go to Implementation / Go to Interface) ---

    fn supertypes_of<'a>(edges: &'a [SupertypeEdge], type_name: &str) -> Vec<&'a str> {
        let mut names: Vec<&str> = edges
            .iter()
            .filter(|e| e.type_name == type_name)
            .map(|e| e.supertype_name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn rust_impl_trait_for_type_is_a_supertype_edge() {
        let text = "trait Shape {}\nstruct Circle;\nimpl Shape for Circle {}\n";
        let edges = supertype_edges(rust(), text);
        assert_eq!(supertypes_of(&edges, "Circle"), vec!["Shape"]);
        let edge = edges
            .iter()
            .find(|e| e.type_name == "Circle")
            .expect("expected a Circle edge");
        assert_eq!(&text[edge.type_start..edge.type_end], "Circle");
    }

    #[test]
    fn rust_inherent_impl_is_not_a_supertype_edge() {
        let edges = supertype_edges(rust(), "struct Circle;\nimpl Circle {}\n");
        assert!(edges.is_empty());
    }

    #[test]
    fn rust_supertrait_is_a_supertype_edge() {
        let edges = supertype_edges(rust(), "trait Shape: Drawable {}\n");
        assert_eq!(supertypes_of(&edges, "Shape"), vec!["Drawable"]);
    }

    #[test]
    fn java_extends_and_implements_are_separate_edges() {
        let text = "class Circle extends Shape implements Drawable, Sized {}\n";
        let edges = supertype_edges(java(), text);
        assert_eq!(
            supertypes_of(&edges, "Circle"),
            vec!["Drawable", "Shape", "Sized"]
        );
    }

    #[test]
    fn java_interface_extends_is_a_supertype_edge() {
        let edges = supertype_edges(java(), "interface Drawable extends Shape {}\n");
        assert_eq!(supertypes_of(&edges, "Drawable"), vec!["Shape"]);
    }

    #[test]
    fn csharp_base_list_entries_are_supertype_edges() {
        let edges = supertype_edges(csharp(), "class Circle : Shape, IDrawable {}\n");
        assert_eq!(supertypes_of(&edges, "Circle"), vec!["IDrawable", "Shape"]);
    }

    #[test]
    fn php_extends_and_implements_are_supertype_edges() {
        let text = "<?php\nclass Circle extends Shape implements Drawable {}\n";
        let edges = supertype_edges(php(), text);
        assert_eq!(supertypes_of(&edges, "Circle"), vec!["Drawable", "Shape"]);
    }

    #[test]
    fn json_and_plain_text_have_no_supertype_edges() {
        assert!(supertype_edges(json(), "{\"a\": 1}").is_empty());
        assert!(supertype_edges(Language::PLAIN_TEXT, "class A extends B {}").is_empty());
    }

    #[test]
    fn rust_locals_cover_every_construct_the_outline_calls_a_definition() {
        // Regression guard for a drift between rust/tags.scm and
        // rust/locals.scm: a trait, an `impl` target and a struct field
        // were definitions to the outline but not to
        // `identifier_occurrences`, so nothing could navigate to them.
        let text = "pub trait Shape {\n    fn area(&self) -> f64;\n}\n\npub struct Circle {\n    radius: f64,\n}\n\nimpl Shape for Circle {\n    fn area(&self) -> f64 {\n        self.radius\n    }\n}\n";
        let occurrences = identifier_occurrences(rust(), text);
        let defined = |name: &str| {
            occurrences
                .iter()
                .any(|o| o.name == name && o.is_definition)
        };

        assert!(defined("Shape"), "trait name is a definition");
        assert!(defined("Circle"), "struct name is a definition");
        assert!(defined("radius"), "struct field is a definition");

        // And a field *use* is indexed as a reference, not skipped: before
        // the fix `field_identifier` matched no pattern at all.
        assert!(
            occurrences
                .iter()
                .any(|o| o.name == "radius" && !o.is_definition),
            "self.radius is a reference"
        );
    }

    // ---- injections (I1) -------------------------------------------
    //
    // The shipped queries that use the mechanism (Markdown, HTML, PHP)
    // are exercised end to end in `tests/language_catalog.rs`, against
    // their real fixtures.
    //
    // What is proved here is the mechanism's edges, against synthetic
    // hosts built from real grammars: Rust injecting JSON (cross-language,
    // both spellings of the language name), JSON injecting itself
    // (nesting, and the depth bound), and an unknown language name.

    /// A catalog language's compiled grammar and highlights, plus an
    /// `injections.scm` it does not ship. Rebuilt rather than cloned
    /// because `Query` is not `Clone`.
    fn with_injections(language: Language, injections: &str) -> Arc<CompiledLanguage> {
        let def = language.def().expect("catalog language");
        let grammar = (def.grammar())();
        let highlights = Query::new(&grammar, def.queries().highlights.expect("highlights.scm"))
            .expect("highlights.scm compiles");
        let highlight_scopes = highlights
            .capture_names()
            .iter()
            .map(|name| Scope::resolve(name))
            .collect();
        Arc::new(CompiledLanguage {
            highlights: Some(highlights),
            highlight_scopes,
            injections: Some(Query::new(&grammar, injections).expect("injections.scm compiles")),
            locals: None,
            folds: None,
            tags: None,
            inherits: None,
            grammar,
        })
    }

    fn parse_with(compiled: &CompiledLanguage, text: &str) -> tree_sitter::Tree {
        parse_once(&compiled.grammar, text).expect("parse")
    }

    fn from_registry(id: &str) -> Option<Arc<CompiledLanguage>> {
        language_by_id(id).and_then(registry::compiled)
    }

    fn scopes_at<'a>(spans: &'a [HighlightSpan], text: &'a str, name: &str) -> Vec<&'a str> {
        let wanted = scope(name);
        spans
            .iter()
            .filter(|s| s.scope == wanted)
            .map(|s| &text[s.start..s.end])
            .collect()
    }

    /// `#set! injection.language` spelling: a Rust raw string holding JSON.
    #[test]
    fn a_set_directive_names_the_injected_language() {
        let host = with_injections(
            rust(),
            r#"((raw_string_literal (string_content) @injection.content)
                (#set! injection.language "json"))"#,
        );
        let text = r####"const C: &str = r#"{"k": true}"#;"####;
        let tree = parse_with(&host, text);
        let spans = spans_with_injections(&host, &tree, text, 0, &from_registry);

        // `true` as a keyword inside a Rust string literal is something
        // only the injected JSON grammar can produce.
        assert!(
            scopes_at(&spans, text, "keyword").contains(&"true"),
            "region not highlighted as JSON: {spans:?}"
        );
        assert!(
            scopes_at(&spans, text, "string").contains(&"\"k\""),
            "JSON key not highlighted as a JSON string"
        );
        // The host's own `@string` *encloses* the injected region (it takes
        // in the `r#"` delimiters), so it is kept — and sorted ahead of the
        // spans it encloses, which is what lets the view paint JSON over
        // it. A host span sitting *inside* the region is what gets dropped.
        let host_string = spans
            .iter()
            .position(|span| {
                span.scope.name() == "string" && text[span.start..span.end].starts_with("r#\"")
            })
            .expect("the enclosing host string span is kept");
        let injected = spans
            .iter()
            .position(|span| {
                span.scope.name() == "keyword" && &text[span.start..span.end] == "true"
            })
            .expect("the injected JSON keyword");
        assert!(
            host_string < injected,
            "the enclosing host span must be painted before the spans inside it"
        );
        // The host still highlights everything outside the region.
        assert!(scopes_at(&spans, text, "keyword").contains(&"const"));
    }

    /// `@injection.language` capture spelling: the node's *text* names the
    /// language, the way `(#match?)`-free upstream queries do it.
    #[test]
    fn an_injection_language_capture_names_the_injected_language() {
        let host = with_injections(
            rust(),
            "(macro_invocation
               macro: (identifier) @injection.language
               (token_tree (token_tree) @injection.content))",
        );
        let text = r#"fn f() { json!({"k": true}); }"#;
        let tree = parse_with(&host, text);
        let spans = spans_with_injections(&host, &tree, text, 0, &from_registry);

        assert!(
            scopes_at(&spans, text, "string").contains(&"\"k\""),
            "macro body not parsed as JSON: {spans:?}"
        );
    }

    #[test]
    fn merged_spans_stay_sorted_by_start_offset() {
        let host = with_injections(
            rust(),
            r#"((raw_string_literal (string_content) @injection.content)
                (#set! injection.language "json"))"#,
        );
        let text = r####"fn f() { let a = r#"{"x": 1}"#; let b = r#"[2, 3]"#; }"####;
        let tree = parse_with(&host, text);
        let spans = spans_with_injections(&host, &tree, text, 0, &from_registry);

        assert!(
            spans.len() > 4,
            "expected host and injected spans: {spans:?}"
        );
        assert!(
            spans
                .windows(2)
                .all(|w| (w[0].start, w[0].end) <= (w[1].start, w[1].end)),
            "merged stream is not sorted; the view binary-searches it: {spans:?}"
        );
    }

    /// An unknown injected language is skipped, not fatal — a runtime
    /// grammar can name a language this build does not have.
    #[test]
    fn an_unknown_injected_language_leaves_the_host_spans_alone() {
        let host = with_injections(
            rust(),
            r#"((raw_string_literal) @injection.content
                (#set! injection.language "klingon"))"#,
        );
        let text = r####"const C: &str = r#"{"k": true}"#;"####;
        let tree = parse_with(&host, text);
        let spans = spans_with_injections(&host, &tree, text, 0, &from_registry);

        assert!(scopes_at(&spans, text, "keyword").contains(&"const"));
        assert!(!scopes_at(&spans, text, "keyword").contains(&"true"));
    }

    /// A host with no `injections.scm` must produce byte-identical output
    /// to the pre-I1 single-tree path.
    #[test]
    fn a_document_without_injections_is_unchanged() {
        let compiled = registry::compiled(rust()).expect("rust compiles");
        let text = "fn main() { let s = \"hi\"; /* c */ }";
        let tree = parse_with(&compiled, text);
        let merged = spans_with_injections(&compiled, &tree, text, 0, &from_registry);
        let single = spans_from_tree(
            compiled.highlights.as_ref().unwrap(),
            &compiled.highlight_scopes,
            &tree,
            text,
        );
        assert_eq!(merged, single);
        assert_eq!(merged, highlight(rust(), text));
    }

    /// Nesting: injections are followed through injected trees, and the
    /// walk stops at [`MAX_INJECTION_DEPTH`] instead of running forever.
    ///
    /// A chain of four synthetic languages over the JSON grammar, each
    /// injecting the next into any nested object. `l4` is what the
    /// depth-3 tree would ask for, so "`l4` was never resolved" is the
    /// bound, stated as an observation rather than a constant.
    #[test]
    fn nesting_is_followed_and_bounded() {
        let injects = |next: &str| {
            with_injections(
                json(),
                &format!(r#"((object) @injection.content (#set! injection.language "{next}"))"#),
            )
        };
        let host = injects("l1");
        let chain = [
            ("l1", injects("l2")),
            ("l2", injects("l3")),
            ("l3", injects("l4")),
        ];
        // Nested deeper than the bound, so the bound stops the walk, not
        // the document.
        let text = r#"{"a":{"b":{"c":{"d":{"e":true}}}}}"#;
        let tree = parse_with(&host, text);

        let asked = std::cell::RefCell::new(Vec::new());
        let resolve = |id: &str| {
            asked.borrow_mut().push(id.to_string());
            chain
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, compiled)| compiled.clone())
        };
        let spans = spans_with_injections(&host, &tree, text, 0, &resolve);

        let asked = asked.into_inner();
        assert!(
            asked.contains(&"l3".to_string()),
            "nesting stopped early: {asked:?}"
        );
        assert!(
            !asked.contains(&"l4".to_string()),
            "recursed past MAX_INJECTION_DEPTH = {MAX_INJECTION_DEPTH}: {asked:?}"
        );
        assert!(
            spans
                .windows(2)
                .all(|w| (w[0].start, w[0].end) <= (w[1].start, w[1].end)),
            "nested merge is not sorted: {spans:?}"
        );
        assert!(scopes_at(&spans, text, "keyword").contains(&"true"));
    }

    /// Injections survive an incremental edit, and `fold_ranges` still
    /// reads off the host tree.
    #[test]
    fn incremental_editing_still_works_with_a_host_tree() {
        let mut highlighter = Highlighter::new(rust());
        let before = "fn main() {\n    let x = 1;\n}\n";
        highlighter.set_text(before);
        let after = "fn main() {\n    let x = 2;\n}\n";
        let start = before.find('1').unwrap();
        let spans = highlighter.edit(after, start, start + 1, start + 1);
        assert!(scopes_at(&spans, after, "keyword").contains(&"fn"));
        assert!(!highlighter.fold_ranges().is_empty());
    }

    // --- Query predicates -------------------------------------------
    //
    // `spans_from_tree`'s contract: text predicates guard their pattern,
    // predicates tree-sitter cannot evaluate drop it, and same-node
    // captures resolve first-pattern-wins.

    /// Runs one hand-written highlights query, bypassing the shipped
    /// `.scm` file, so a predicate can be tested in isolation.
    fn spans_of(language: Language, highlights: &str, text: &str) -> Vec<HighlightSpan> {
        let def = language.def().expect("catalog language");
        let grammar = (def.grammar())();
        let query = Query::new(&grammar, highlights).expect("query compiles");
        let scopes: Vec<Option<Scope>> = query
            .capture_names()
            .iter()
            .map(|name| Scope::resolve(name))
            .collect();
        let tree = parse_once(&grammar, text).expect("parse");
        spans_from_tree(&query, &scopes, &tree, text)
    }

    #[test]
    fn a_match_predicate_guards_its_pattern() {
        let text = "FOO = Bar + baz\n";
        let spans = spans_of(
            lang("python"),
            r#"((identifier) @constant (#match? @constant "^[A-Z][A-Z0-9_]+$"))"#,
            text,
        );
        // The guard holds: the SCREAMING_CASE name matches and neither the
        // CamelCase nor the lowercase one does.
        assert_eq!(scopes_at(&spans, text, "constant"), vec!["FOO"]);
    }

    #[test]
    fn a_not_match_predicate_guards_its_pattern() {
        let text = "FOO = Bar + baz\n";
        let spans = spans_of(
            lang("python"),
            r#"((identifier) @variable (#not-match? @variable "^[A-Z]"))"#,
            text,
        );
        assert_eq!(scopes_at(&spans, text, "variable"), vec!["baz"]);
    }

    #[test]
    fn eq_and_any_of_predicates_guard_their_patterns() {
        let text = "alpha = beta + gamma\n";
        let spans = spans_of(
            lang("python"),
            r#"((identifier) @constant (#eq? @constant "beta"))"#,
            text,
        );
        assert_eq!(scopes_at(&spans, text, "constant"), vec!["beta"]);

        let spans = spans_of(
            lang("python"),
            r#"((identifier) @constant (#any-of? @constant "alpha" "gamma"))"#,
            text,
        );
        assert_eq!(scopes_at(&spans, text, "constant"), vec!["alpha", "gamma"]);
    }

    /// A predicate tree-sitter parses but never applies must take its
    /// pattern down with it. Shipping such a pattern unguarded is the
    /// failure mode this protects against — `#is-not? local` unguarded
    /// paints every identifier in the file.
    #[test]
    fn a_predicate_tree_sitter_cannot_evaluate_drops_its_pattern() {
        let text = "FOO = Bar + baz\n";
        for guard in [
            // Property predicate: needs a locals resolver we do not have.
            r#"((identifier) @constant (#is-not? local))"#,
            // General predicate: nvim-treesitter flavour, unknown to
            // tree-sitter itself.
            r#"((identifier) @constant (#lua-match? @constant "^%u+$"))"#,
        ] {
            let spans = spans_of(lang("python"), guard, text);
            assert!(
                spans.is_empty(),
                "{guard} shipped unguarded and produced {spans:?}"
            );
        }
    }

    /// `#set!` is a directive, not a predicate: it must not disarm the
    /// pattern it sits on (injection resolution relies on this).
    #[test]
    fn a_set_directive_does_not_drop_its_pattern() {
        let text = "alpha = 1\n";
        let spans = spans_of(
            lang("python"),
            r#"((identifier) @constant (#set! priority 100))"#,
            text,
        );
        assert_eq!(scopes_at(&spans, text, "constant"), vec!["alpha"]);
    }

    #[test]
    fn same_node_captures_resolve_first_pattern_wins() {
        let text = "class Widget:\n    pass\n";
        let spans = spans_of(
            lang("python"),
            r#"
            (class_definition name: (identifier) @type)
            ((identifier) @constant (#match? @constant "^[A-Z]"))
            "#,
            text,
        );
        assert_eq!(scopes_at(&spans, text, "type"), vec!["Widget"]);
        assert!(
            scopes_at(&spans, text, "constant").is_empty(),
            "the later catch-all beat the specific pattern: {spans:?}"
        );
    }

    /// End-to-end through the shipped `python/highlights.scm`: the two
    /// naming conventions every mainstream editor paints.
    #[test]
    fn shipped_queries_paint_the_naming_conventions() {
        let text = "MAX_RETRIES = 3\nvalue = Widget\nname = other\n";
        let spans = highlight(lang("python"), text);
        assert!(
            scopes_at(&spans, text, "constant").contains(&"MAX_RETRIES"),
            "SCREAMING_CASE is not a constant: {spans:?}"
        );
        assert!(
            scopes_at(&spans, text, "type").contains(&"Widget"),
            "CamelCase is not a type: {spans:?}"
        );
        // The lowercase names are untouched by the convention patterns.
        assert!(!scopes_at(&spans, text, "type").contains(&"value"));
        assert!(!scopes_at(&spans, text, "constant").contains(&"name"));
    }
}
