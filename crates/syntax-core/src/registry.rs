//! The language catalog and the registry that resolves it.
//!
//! Mirrors how `app-config`'s `keymap::ACTIONS` splits const data from
//! resolution: [`BUILTIN_LANGUAGES`] is a plain const table, and
//! [`LanguageRegistry`] is the thing that answers questions about it.
//! Adding a language is one row plus its `.scm` files — no new function,
//! no new match arm.
//!
//! Qt-free, like the rest of this crate.

use std::path::Path;
use std::sync::{Arc, LazyLock, OnceLock, RwLock};

use tree_sitter::Query;

use crate::Scope;

pub use crate::catalog::BUILTIN_LANGUAGES;
#[cfg(test)]
use crate::catalog::{BRACKETS, QUOTES_DOUBLE};

/// The `.scm` sources for one language, each optional.
///
/// Generic over how the sources are held so the two owners can share one
/// type: the const catalog bundles them into the binary via `include_str!`
/// (`QuerySet<&'static str>`, the default) and a runtime language owns
/// what it read off disk (`QuerySet<String>`).
#[derive(Debug, Clone, Copy, Default)]
pub struct QuerySet<S = &'static str> {
    pub highlights: Option<S>,
    pub locals: Option<S>,
    pub folds: Option<S>,
    pub tags: Option<S>,
    pub inherits: Option<S>,
    /// Regions of a file written in *another* language (CSS in a `<style>`
    /// element, a fenced code block in Markdown). Standard tree-sitter
    /// shape: `@injection.content` is the region, and the language is named
    /// either by an `@injection.language` capture or a
    /// `(#set! injection.language "css")` directive — see
    /// [`crate::MAX_INJECTION_DEPTH`].
    pub injections: Option<S>,
}

impl QuerySet {
    /// No queries at all; a row names only the ones it ships.
    pub const NONE: QuerySet = QuerySet {
        highlights: None,
        locals: None,
        folds: None,
        tags: None,
        inherits: None,
        injections: None,
    };
}

fn borrow<S: AsRef<str>>(field: &Option<S>) -> Option<&str> {
    field.as_ref().map(AsRef::as_ref)
}

impl<S: AsRef<str>> QuerySet<S> {
    /// Borrowed view, so the catalog's `&'static str` sources and a
    /// runtime language's owned ones read identically.
    pub fn as_deref(&self) -> QuerySet<&str> {
        QuerySet {
            highlights: borrow(&self.highlights),
            locals: borrow(&self.locals),
            folds: borrow(&self.folds),
            tags: borrow(&self.tags),
            inherits: borrow(&self.inherits),
            injections: borrow(&self.injections),
        }
    }
}

/// One language the editor can highlight and index.
///
/// `id` is the stable, persisted key (settings files, per-language color
/// overrides, language-server config) and must never change; `name` is
/// display-only and may.
#[derive(Debug, Clone, Copy)]
pub struct LanguageDef {
    pub id: &'static str,
    pub name: &'static str,
    /// Extensions without the leading dot, lowercase. Collisions between
    /// languages are legal and resolve first-match-wins in catalog order
    /// — see [`LanguageRegistry::language_for_path`]. An extension may be
    /// compound (`blade.php`); it then outranks any shorter one.
    pub extensions: &'static [&'static str],
    /// Whole file names for extensionless languages (`Dockerfile`,
    /// `Makefile`). Matched before extensions, case-sensitively, and once
    /// more against the name minus a trailing `.suffix` after extensions
    /// have had their turn — see [`LanguageRegistry::language_for_path`].
    pub filenames: &'static [&'static str],
    pub grammar: fn() -> tree_sitter::Language,
    pub queries: QuerySet,
    /// The token that comments out the rest of a line, or `None` for a
    /// language that genuinely has none (JSON). Never a guess: a wrong
    /// token corrupts the file `Ctrl+/` is pressed in.
    pub line_comment: Option<&'static str>,
    /// The `(open, close)` delimiters of a block comment, or `None`.
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Bracket pairs, for matching, auto-close and indentation. Ordered
    /// pairs of `(open, close)`; both sides are always distinct strings.
    pub brackets: &'static [(&'static str, &'static str)],
    /// Quote characters that open *and* close a literal in this language.
    /// Separate from [`Self::brackets`] because the two sides are equal,
    /// which changes every rule that inspects them.
    pub quotes: &'static [&'static str],
}

/// A language loaded from the config directory at runtime.
///
/// The owned twin of [`LanguageDef`]: same fields, but nothing borrowed,
/// so a generation of runtime languages is freed when the registry that
/// used it is dropped. `grammar` stays a bare `fn` pointer — foreign
/// grammar libraries are deliberately never unloaded (see
/// [`crate::runtime`]), and this type must not make one droppable.
#[derive(Debug)]
pub struct OwnedLanguageDef {
    pub id: String,
    pub name: String,
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub grammar: fn() -> tree_sitter::Language,
    pub queries: QuerySet<String>,
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
    pub brackets: Vec<(String, String)>,
    pub quotes: Vec<String>,
}

/// What stands behind one registry row: a const catalog row, or a
/// reference-counted runtime one.
///
/// The split is why the catalog can stay a `const` table of `&'static str`
/// while a reload still frees the generation it replaced. Cheap to clone
/// (a pointer copy or an `Arc` bump).
#[derive(Debug, Clone)]
pub enum Def {
    Builtin(&'static LanguageDef),
    Runtime(Arc<OwnedLanguageDef>),
}

/// One iterator type over both a catalog row's `&'static [&'static str]`
/// and a runtime row's `Vec<String>`; exactly one side is ever non-empty.
fn strs<'a>(builtin: &'a [&'static str], owned: &'a [String]) -> impl Iterator<Item = &'a str> {
    builtin
        .iter()
        .copied()
        .chain(owned.iter().map(String::as_str))
}

impl Def {
    pub fn id(&self) -> &str {
        match self {
            Self::Builtin(def) => def.id,
            Self::Runtime(def) => &def.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Builtin(def) => def.name,
            Self::Runtime(def) => &def.name,
        }
    }

    pub fn extensions(&self) -> impl Iterator<Item = &str> {
        match self {
            Self::Builtin(def) => strs(def.extensions, &[]),
            Self::Runtime(def) => strs(&[], &def.extensions),
        }
    }

    pub fn filenames(&self) -> impl Iterator<Item = &str> {
        match self {
            Self::Builtin(def) => strs(def.filenames, &[]),
            Self::Runtime(def) => strs(&[], &def.filenames),
        }
    }

    pub fn grammar(&self) -> fn() -> tree_sitter::Language {
        match self {
            Self::Builtin(def) => def.grammar,
            Self::Runtime(def) => def.grammar,
        }
    }

    pub fn queries(&self) -> QuerySet<&str> {
        match self {
            Self::Builtin(def) => def.queries.as_deref(),
            Self::Runtime(def) => def.queries.as_deref(),
        }
    }

    pub fn line_comment(&self) -> Option<&str> {
        match self {
            Self::Builtin(def) => def.line_comment,
            Self::Runtime(def) => def.line_comment.as_deref(),
        }
    }

    pub fn block_comment(&self) -> Option<(&str, &str)> {
        match self {
            Self::Builtin(def) => def.block_comment,
            Self::Runtime(def) => def
                .block_comment
                .as_ref()
                .map(|(open, close)| (open.as_str(), close.as_str())),
        }
    }

    /// Owned pairs rather than an iterator: every caller wants the whole
    /// (two- or three-element) list, and one `Vec` is cheaper than the
    /// type gymnastics that would unify the two representations.
    pub fn brackets(&self) -> Vec<(&str, &str)> {
        match self {
            Self::Builtin(def) => def.brackets.to_vec(),
            Self::Runtime(def) => def
                .brackets
                .iter()
                .map(|(open, close)| (open.as_str(), close.as_str()))
                .collect(),
        }
    }

    pub fn quotes(&self) -> Vec<&str> {
        match self {
            Self::Builtin(def) => def.quotes.to_vec(),
            Self::Runtime(def) => def.quotes.iter().map(String::as_str).collect(),
        }
    }
}

/// An opaque handle to a language in the registry.
///
/// A `Copy` index, not an enum: adding a language costs a catalog row and
/// nothing else. The index is only meaningful against the registry
/// snapshot it came from, so don't persist it — persist [`Language::id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Language(u16);

impl Language {
    /// The always-present no-op language: no grammar, no queries, no
    /// spans. Reserved at registry index 0, so it survives any reload.
    pub const PLAIN_TEXT: Language = Language(0);

    /// The definition behind this handle in the *current* registry, or
    /// `None` for plain text.
    ///
    /// Returned by value, not borrowed: a runtime definition lives only as
    /// long as some registry snapshot holds it, and cloning a [`Def`] is a
    /// pointer copy or an `Arc` bump. Prefer [`LanguageRegistry::def`] when
    /// you already hold a registry.
    pub fn def(self) -> Option<Def> {
        registry().def(self).cloned()
    }

    /// Stable, persistable id — `"plaintext"` for [`Language::PLAIN_TEXT`].
    ///
    /// Owned, not `&'static str`: since G2 a runtime definition can be
    /// dropped by the next reload, so nothing read out of the global
    /// registry can outlive the snapshot it came from. Borrowing through
    /// [`LanguageRegistry::def`] avoids the allocation where a caller
    /// already holds a snapshot; these two exist for the callers that do
    /// not (status bar, settings pages), where one `String` is noise.
    pub fn id(self) -> String {
        self.def()
            .map_or_else(|| "plaintext".to_string(), |d| d.id().to_string())
    }

    /// Human-readable name for the status bar.
    pub fn name(self) -> String {
        self.def()
            .map_or_else(|| "Plain Text".to_string(), |d| d.name().to_string())
    }
}

/// A language's grammar and its `.scm` queries, compiled once and shared.
///
/// Handed out behind an [`Arc`] so a future registry reload cannot pull
/// the grammar out from under a live `Highlighter`.
pub struct CompiledLanguage {
    pub grammar: tree_sitter::Language,
    pub highlights: Option<Query>,
    /// `highlights`' capture index -> [`Scope`], resolved once here rather
    /// than per span in the highlight hot path. `None` for a capture whose
    /// name has no scope even after hierarchical fallback — that capture
    /// simply yields no spans, which is how an upstream `.scm` file we
    /// never vetted stays safe to load unmodified.
    pub highlight_scopes: Vec<Option<Scope>>,
    pub locals: Option<Query>,
    pub folds: Option<Query>,
    pub tags: Option<Query>,
    pub inherits: Option<Query>,
    pub injections: Option<Query>,
}

fn compile(def: &Def) -> Result<CompiledLanguage, String> {
    let grammar = (def.grammar())();
    let queries = def.queries();
    let compile_one = |kind: &str, source: Option<&str>| -> Result<Option<Query>, String> {
        source
            .map(|source| {
                Query::new(&grammar, source)
                    .map_err(|err| format!("{}/{kind}.scm: {err}", def.id()))
            })
            .transpose()
    };
    let highlights = compile_one("highlights", queries.highlights)?;
    Ok(CompiledLanguage {
        highlight_scopes: capture_scopes(highlights.as_ref()),
        highlights,
        locals: compile_one("locals", queries.locals)?,
        folds: compile_one("folds", queries.folds)?,
        tags: compile_one("tags", queries.tags)?,
        inherits: compile_one("inherits", queries.inherits)?,
        injections: compile_one("injections", queries.injections)?,
        grammar,
    })
}

/// Resolve every capture name in `query` to a [`Scope`] once, in capture
/// index order.
fn capture_scopes(query: Option<&Query>) -> Vec<Option<Scope>> {
    query.map_or_else(Vec::new, |query| {
        query
            .capture_names()
            .iter()
            .map(|name| Scope::resolve(name))
            .collect()
    })
}

struct Entry {
    /// `None` only for the reserved plain-text slot.
    def: Option<Def>,
    /// Turned off by the user (`Settings::disabled_languages`). The entry
    /// stays in the registry so it can still be enumerated and re-enabled;
    /// it is skipped by everything that *resolves* a language, so its files
    /// open as plain text.
    disabled: bool,
    compiled: OnceLock<Result<Arc<CompiledLanguage>, String>>,
}

/// The merged view of every known language: the const catalog today, plus
/// runtime-loaded entries merged in by id later (G1a/G1b). Rebuilt whole
/// on reload rather than mutated in place — see [`registry`].
pub struct LanguageRegistry {
    entries: Vec<Entry>,
}

impl LanguageRegistry {
    fn new(builtins: &'static [LanguageDef]) -> Self {
        Self::with_runtime(builtins, &[], &[])
    }

    /// The catalog with `runtime` entries layered over it by id — the
    /// same shape of override as a user keymap over `ACTIONS`.
    ///
    /// A runtime entry whose id matches a builtin replaces it *in place*,
    /// so overriding a language cannot change which one wins an extension
    /// collision (decision 4); a new id is appended, and therefore loses
    /// any collision against a builtin. Runtime entries are produced by
    /// [`crate::runtime::load`], which has already rejected anything that
    /// does not parse or compile.
    /// `disabled` names the ids the user turned off. They are kept as
    /// entries — [`LanguageRegistry::languages`] still yields them, which is
    /// the only way the settings page can offer to switch one back on — but
    /// no lookup resolves to one.
    pub fn with_runtime(
        builtins: &'static [LanguageDef],
        runtime: &[Arc<OwnedLanguageDef>],
        disabled: &[String],
    ) -> Self {
        let mut defs: Vec<Def> = builtins.iter().map(Def::Builtin).collect();
        for entry in runtime {
            let def = Def::Runtime(Arc::clone(entry));
            match defs.iter().position(|d| d.id() == def.id()) {
                Some(index) => defs[index] = def,
                None => defs.push(def),
            }
        }
        let mut entries = vec![Entry {
            def: None,
            disabled: false,
            compiled: OnceLock::new(),
        }];
        entries.extend(defs.into_iter().map(|def| Entry {
            disabled: disabled.iter().any(|id| id == def.id()),
            def: Some(def),
            compiled: OnceLock::new(),
        }));
        Self { entries }
    }

    /// The catalog entry behind `language` in *this* registry, or `None`
    /// for plain text. Prefer this over [`Language::def`] when holding a
    /// registry that is not the global one.
    pub fn def(&self, language: Language) -> Option<&Def> {
        self.entries.get(usize::from(language.0))?.def.as_ref()
    }

    /// Every language in the registry, plain text first.
    pub fn languages(&self) -> Vec<Language> {
        (0..self.entries.len() as u16).map(Language).collect()
    }

    /// Look one up by id — `None` for an unknown id *and* for a disabled
    /// one: a disabled language must not highlight anything.
    pub fn language_by_id(&self, id: &str) -> Option<Language> {
        if id == "plaintext" {
            return Some(Language::PLAIN_TEXT);
        }
        self.entries
            .iter()
            .position(|e| !e.disabled && e.def.as_ref().is_some_and(|d| d.id() == id))
            .map(|index| Language(index as u16))
    }

    /// Which language highlights `path`. Four steps, in this order, and
    /// within each step the first match in catalog order wins:
    ///
    /// 1. the whole file name against `filenames`, case-sensitively;
    /// 2. the file name's tail against compound extensions (`blade.php`),
    ///    lowercased — so `home.blade.php` is Blade, not PHP;
    /// 3. the extension against `extensions`, lowercased;
    /// 4. the file name with its final `.suffix` removed, against
    ///    `filenames` again — `Dockerfile.dev`, `Makefile.local`,
    ///    `.env.local`.
    ///
    /// Step 4 is last on purpose: `Dockerfile.md` is Markdown, because a
    /// real extension describes the file's contents and a stage suffix
    /// does not. It re-checks only `filenames`, never `extensions`, so it
    /// widens the extensionless languages and cannot make `Cargo.lock.bak`
    /// resolve through some unrelated extension.
    pub fn language_for_path(&self, path: &Path) -> Language {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        // `Dockerfile.dev` -> `Dockerfile`; empty when the name carries no
        // suffix at all, so a plain `Dockerfile` is never checked twice.
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| *s != file_name)
            .unwrap_or_default();

        let defs = || {
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.disabled)
                .filter_map(|(i, e)| Some((i, e.def.as_ref()?)))
        };
        let by_name = |name: &str| {
            (!name.is_empty())
                .then(|| defs().find(|(_, d)| d.filenames().any(|n| n == name)))
                .flatten()
        };
        let lowercase_name = file_name.to_lowercase();
        let compound = || {
            defs().find(|(_, d)| {
                d.extensions()
                    .any(|e| e.contains('.') && lowercase_name.ends_with(&format!(".{e}")))
            })
        };
        let matched = by_name(&file_name)
            .or_else(compound)
            .or_else(|| {
                (!extension.is_empty())
                    .then(|| defs().find(|(_, d)| d.extensions().any(|e| e == extension)))
                    .flatten()
            })
            .or_else(|| by_name(&stem));
        matched.map_or(Language::PLAIN_TEXT, |(index, _)| Language(index as u16))
    }

    /// The compiled grammar + queries for `language`, compiled on first
    /// use. `None` for plain text (nothing to compile); `Some(Err)` when a
    /// shipped query does not compile against its grammar — a broken
    /// language degrades to no highlighting instead of taking the editor
    /// down. `every_shipped_query_compiles` keeps that from shipping.
    pub fn compiled(&self, language: Language) -> Option<Result<Arc<CompiledLanguage>, String>> {
        let entry = self.entries.get(usize::from(language.0))?;
        let def = entry.def.as_ref()?;
        Some(
            entry
                .compiled
                .get_or_init(|| compile(def).map(Arc::new))
                .clone(),
        )
    }
}

/// The process-wide registry.
///
/// Held as `RwLock<Arc<..>>` rather than `RwLock<LanguageRegistry>` on
/// purpose: every read clones the `Arc` and drops the lock immediately, so
/// no lock is ever held across a parse (`index-core` looks languages up
/// from background indexing threads). A future live reload (G2) swaps in a
/// freshly built `Arc` under the write lock; callers holding the old
/// snapshot — and `Highlighter`s holding an `Arc<CompiledLanguage>` out of
/// it — keep working untouched.
static REGISTRY: LazyLock<RwLock<Arc<LanguageRegistry>>> =
    LazyLock::new(|| RwLock::new(Arc::new(LanguageRegistry::new(BUILTIN_LANGUAGES))));

/// A snapshot of the current registry. Cheap (one `Arc` clone).
pub fn registry() -> Arc<LanguageRegistry> {
    REGISTRY
        .read()
        .expect("language registry lock poisoned")
        .clone()
}

/// Which language highlights `path` — [`Language::PLAIN_TEXT`] when
/// nothing claims it. See [`LanguageRegistry::language_for_path`].
pub fn language_for_path(path: &Path) -> Language {
    registry().language_for_path(path)
}

/// Look a language up by its stable id (`"rust"`, `"plaintext"`).
pub fn language_by_id(id: &str) -> Option<Language> {
    registry().language_by_id(id)
}

/// Re-scan `<config_dir>/languages` and swap in a registry built from the
/// builtins plus what it finds (G2), minus the ids in `disabled`. Returns
/// the load errors so the UI can show them; an empty vec means everything
/// on disk loaded.
///
/// `config_dir` is a parameter, not resolved here: `syntax-core` has no
/// `dirs` dependency and does not get one — the caller owns "where config
/// lives", exactly as it does for `runtime::load`.
///
/// Safe to call with editors open. The scan (file I/O, `dlopen`, query
/// compiles) happens *before* the write lock is taken, so a reload never
/// blocks a parse for longer than the pointer swap. Live `Highlighter`s
/// hold an `Arc<CompiledLanguage>` and keep parsing with the grammar they
/// were built with; anything opened afterwards sees the new registry.
pub fn reload(config_dir: &Path, disabled: &[String]) -> Vec<crate::runtime::LanguageLoadError> {
    let loaded = crate::runtime::load(config_dir, BUILTIN_LANGUAGES);
    let rebuilt = Arc::new(LanguageRegistry::with_runtime(
        BUILTIN_LANGUAGES,
        &loaded.entries,
        disabled,
    ));
    *REGISTRY.write().expect("language registry lock poisoned") = rebuilt;
    loaded.errors
}

/// Human-readable name for `language` (status bar, L3).
pub fn language_name(language: Language) -> String {
    language.name()
}

pub(crate) fn compiled(language: Language) -> Option<Arc<CompiledLanguage>> {
    registry().compiled(language)?.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_ids_are_unique() {
        let mut ids: Vec<&str> = BUILTIN_LANGUAGES.iter().map(|d| d.id).collect();
        ids.push("plaintext");
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "duplicate language id in the catalog"
        );
    }

    #[test]
    fn every_shipped_query_compiles() {
        let registry = registry();
        for language in registry.languages() {
            if let Some(Err(err)) = registry.compiled(language) {
                panic!("{}: {err}", language.id());
            }
        }
    }

    /// Languages that genuinely have no comment syntax at all. Membership
    /// is a claim about the language, not a to-do: JSON's specification
    /// has no comment production, which is why every "JSON with comments"
    /// is a different format.
    const NO_COMMENT_SYNTAX: &[&str] = &["json"];

    /// The gate that stops language #32 shipping without `Ctrl+/`. One
    /// test over the catalog rather than one test per language: a
    /// per-language function is the one a new row simply never gets.
    #[test]
    fn every_registered_language_has_a_comment_token_or_is_explicitly_exempt() {
        for def in BUILTIN_LANGUAGES {
            let has_comment = def.line_comment.is_some() || def.block_comment.is_some();
            let exempt = NO_COMMENT_SYNTAX.contains(&def.id);
            assert_ne!(
                has_comment, exempt,
                "{}: declare a comment token, or add it to NO_COMMENT_SYNTAX and say why",
                def.id
            );
        }
    }

    #[test]
    fn every_registered_language_has_bracket_pairs_and_distinct_delimiters() {
        for def in BUILTIN_LANGUAGES {
            assert!(!def.brackets.is_empty(), "{}: no bracket pairs", def.id);
            for (open, close) in def.brackets {
                assert_ne!(open, close, "{}: bracket pair with equal sides", def.id);
            }
            for quote in def.quotes {
                assert!(!quote.is_empty(), "{}: empty quote", def.id);
            }
            if let Some((open, close)) = def.block_comment {
                assert!(!open.is_empty() && !close.is_empty(), "{}", def.id);
            }
            if let Some(token) = def.line_comment {
                assert!(!token.is_empty(), "{}: empty line comment", def.id);
            }
        }
    }

    #[test]
    fn a_runtime_definition_reports_its_own_tokens() {
        let def = Def::Runtime(Arc::new(OwnedLanguageDef {
            id: "toy".into(),
            name: "Toy".into(),
            extensions: vec!["toy".into()],
            filenames: Vec::new(),
            grammar: || tree_sitter_json::LANGUAGE.into(),
            queries: QuerySet::default(),
            line_comment: Some(";".into()),
            block_comment: Some(("#|".into(), "|#".into())),
            brackets: vec![("(".into(), ")".into())],
            quotes: vec!["\"".into()],
        }));
        assert_eq!(def.line_comment(), Some(";"));
        assert_eq!(def.block_comment(), Some(("#|", "|#")));
        assert_eq!(def.brackets(), vec![("(", ")")]);
        assert_eq!(def.quotes(), vec!["\""]);
    }

    #[test]
    fn plain_text_has_nothing_to_compile() {
        assert!(registry().compiled(Language::PLAIN_TEXT).is_none());
        assert_eq!(Language::PLAIN_TEXT.id(), "plaintext");
    }

    #[test]
    fn languages_resolve_by_id() {
        assert_eq!(language_by_id("rust").unwrap().name(), "Rust");
        assert_eq!(language_by_id("plaintext"), Some(Language::PLAIN_TEXT));
        assert_eq!(language_by_id("nope"), None);
    }

    /// `.h` is claimed by C, C++ and Objective-C; the rule is that the
    /// first claimant in [`BUILTIN_LANGUAGES`] wins, deterministically.
    /// Pinned against a stand-in catalog so it holds before those
    /// languages ship (R4a) and cannot be silently flipped by reordering.
    /// `Language` is an index into the registry it came from, so a
    /// stand-in catalog must be asked for its own ids, not the global
    /// registry's.
    fn id_in(registry: &LanguageRegistry, language: Language) -> &str {
        registry.def(language).map_or("plaintext", Def::id)
    }

    #[test]
    fn first_match_wins_in_catalog_order() {
        let c = LanguageDef {
            id: "c",
            name: "C",
            extensions: &["c", "h"],
            filenames: &[],
            grammar: || tree_sitter_rust::LANGUAGE.into(),
            queries: QuerySet::default(),
            line_comment: Some("//"),
            block_comment: None,
            brackets: BRACKETS,
            quotes: QUOTES_DOUBLE,
        };
        let cpp = LanguageDef {
            id: "cpp",
            name: "C++",
            extensions: &["cpp", "h"],
            filenames: &[],
            ..c
        };
        let catalog: &'static [LanguageDef] = Box::leak(Box::new([c, cpp]));
        let registry = LanguageRegistry::new(catalog);

        let resolve = |name| id_in(&registry, registry.language_for_path(Path::new(name)));
        assert_eq!(resolve("a.h"), "c");
        assert_eq!(resolve("a.cpp"), "cpp");
    }

    #[test]
    fn a_whole_file_name_can_claim_a_language() {
        let make = LanguageDef {
            id: "make",
            name: "Makefile",
            extensions: &["mk"],
            filenames: &["Makefile", "GNUmakefile"],
            grammar: || tree_sitter_rust::LANGUAGE.into(),
            queries: QuerySet::default(),
            line_comment: Some("//"),
            block_comment: None,
            brackets: BRACKETS,
            quotes: QUOTES_DOUBLE,
        };
        let catalog: &'static [LanguageDef] = Box::leak(Box::new([make]));
        let registry = LanguageRegistry::new(catalog);

        let resolve = |name| id_in(&registry, registry.language_for_path(Path::new(name)));
        assert_eq!(resolve("/p/Makefile"), "make");
        assert_eq!(resolve("build.mk"), "make");
        // Case-sensitive on purpose: `makefile` is a different file.
        assert_eq!(
            registry.language_for_path(Path::new("makefile")),
            Language::PLAIN_TEXT
        );
    }

    #[test]
    fn a_disabled_language_does_not_resolve_but_is_still_listed() {
        let make = LanguageDef {
            id: "make",
            name: "Makefile",
            extensions: &["mk"],
            filenames: &["Makefile"],
            grammar: || tree_sitter_rust::LANGUAGE.into(),
            queries: QuerySet::default(),
            line_comment: Some("//"),
            block_comment: None,
            brackets: BRACKETS,
            quotes: QUOTES_DOUBLE,
        };
        let catalog: &'static [LanguageDef] = Box::leak(Box::new([make]));
        let registry = LanguageRegistry::with_runtime(catalog, &[], &["make".to_string()]);

        // Neither of its claims resolves: its files are plain text now.
        assert_eq!(
            registry.language_for_path(Path::new("build.mk")),
            Language::PLAIN_TEXT
        );
        assert_eq!(
            registry.language_for_path(Path::new("/p/Makefile")),
            Language::PLAIN_TEXT
        );
        assert_eq!(registry.language_by_id("make"), None);

        // But it is still there to be listed and switched back on — a
        // language dropped from the registry could never be re-enabled.
        let listed: Vec<String> = registry
            .languages()
            .into_iter()
            .filter_map(|language| registry.def(language).map(|def| def.id().to_string()))
            .collect();
        assert_eq!(listed, vec!["make".to_string()]);

        let enabled = LanguageRegistry::with_runtime(catalog, &[], &[]);
        assert_eq!(
            id_in(&enabled, enabled.language_for_path(Path::new("build.mk"))),
            "make"
        );
    }

    /// G2, in one test on purpose: [`reload`] swaps the *process-wide*
    /// registry, so splitting these into separate `#[test]` functions
    /// would let them race each other inside the shared test binary.
    ///
    /// Everything here is scoped to a language id no builtin uses, so a
    /// reload mid-flight cannot disturb the other tests in this module
    /// either.
    #[test]
    fn reload_rebuilds_the_registry_around_live_highlighters() {
        use crate::Highlighter;
        use std::fs;

        let config = tempfile::tempdir().unwrap();
        let lang_dir = config.path().join("languages").join("g2test");
        let queries_dir = lang_dir.join("queries");
        fs::create_dir_all(&queries_dir).unwrap();
        let manifest = |body: &str| fs::write(lang_dir.join("language.toml"), body).unwrap();
        let highlights = |body: &str| fs::write(queries_dir.join("highlights.scm"), body).unwrap();

        // An editor opened *before* any reload, on a builtin language.
        let json = "{\n  \"a\": \"one\",\n  \"b\": 2\n}";
        let mut open = Highlighter::new(language_by_id("json").expect("json"));
        let before = open.set_text(json);
        assert!(!before.is_empty(), "json highlights before the reload");
        assert!(
            !open.fold_ranges().is_empty(),
            "json folds before the reload"
        );

        // 1. A language that did not exist at startup is picked up.
        manifest("grammar = \"json\"\nextensions = [\"g2t\"]\n");
        highlights("(string) @string\n");
        assert_eq!(reload(config.path(), &[]), Vec::new());
        assert_eq!(language_for_path(Path::new("x.g2t")).id(), "g2test");
        let span_count = |language| {
            let mut h = Highlighter::new(language);
            h.set_text(json).len()
        };
        let with_strings = span_count(language_by_id("g2test").expect("g2test"));
        assert!(with_strings > 0, "the runtime query highlights something");

        // 2. Editing a query file and reloading picks the edit up — same
        //    id, different spans, no restart.
        highlights("(number) @number\n");
        assert_eq!(reload(config.path(), &[]), Vec::new());
        let with_numbers = span_count(language_by_id("g2test").expect("g2test"));
        assert!(with_numbers > 0 && with_numbers != with_strings);

        // 3. The `Highlighter` opened before either reload is untouched:
        //    it parses, edits incrementally and folds with the grammar it
        //    was built with (decision 3).
        assert_eq!(open.set_text(json), before);
        let at = json.find("one").expect("fixture") + 3;
        let edited = json.replace("one", "onex");
        let after_edit = open.edit(&edited, at, at, at + 1);
        assert!(!after_edit.is_empty(), "still highlighting after a reload");
        assert!(
            !open.fold_ranges().is_empty(),
            "still folding after a reload"
        );

        // 4. Load errors come back through the return value.
        let broken = config.path().join("languages").join("g2broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("language.toml"), "this is not toml =\n").unwrap();
        let errors = reload(config.path(), &[]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].id, "g2broken");
        assert!(matches!(
            errors[0].kind,
            crate::runtime::LoadErrorKind::MalformedManifest(_)
        ));
        // The good language still loaded alongside the broken one.
        assert!(language_by_id("g2test").is_some());

        // 5. Reloading while another thread parses must not deadlock: no
        //    registry lock may be held across a parse (`index-core`
        //    resolves languages from background indexing threads).
        std::thread::scope(|scope| {
            let path = config.path();
            scope.spawn(move || {
                for _ in 0..20 {
                    reload(path, &[]);
                }
            });
            let rust = language_by_id("rust").expect("rust");
            for _ in 0..20 {
                let mut h = Highlighter::new(rust);
                assert!(!h.set_text("fn main() { let x = 1; }").is_empty());
            }
        });

        // Leave the global registry as we found it.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(reload(empty.path(), &[]), Vec::new());
        assert!(language_by_id("g2test").is_none());
    }

    /// The leak G2 would otherwise have had: every reload used to
    /// `Box::leak` a fresh definition plus every `.scm` string it read,
    /// and a user iterating on a query file writes *new* content each
    /// time, so it grew per edit rather than converging. Runtime
    /// definitions are `Arc`d now, so a generation dies with the last
    /// registry snapshot that held it — while the `Arc<CompiledLanguage>`
    /// a live editor is parsing with keeps working.
    ///
    /// Built from local snapshots rather than the process-wide [`reload`]
    /// on purpose: another test in this binary may be holding a global
    /// snapshot while this one runs, which would keep an old generation
    /// alive and make the assertion flaky. The ownership under test is the
    /// same — `reload` is `runtime::load`, `with_runtime`, and dropping
    /// the `Arc` it replaced.
    #[test]
    fn a_reload_frees_the_generation_it_replaced() {
        use std::fs;
        use std::sync::Weak;

        let config = tempfile::tempdir().unwrap();
        let dir = config.path().join("languages").join("freed");
        fs::create_dir_all(dir.join("queries")).unwrap();
        fs::write(
            dir.join("language.toml"),
            "grammar = \"json\"\nextensions = [\"freed\"]\n",
        )
        .unwrap();

        let mut live: Option<Arc<LanguageRegistry>> = None;
        let mut still_open: Vec<Arc<CompiledLanguage>> = Vec::new();
        let mut generations: Vec<Weak<OwnedLanguageDef>> = Vec::new();

        for generation in 0..5 {
            // Different content every round: this is the case interning
            // the strings would *not* have bounded.
            fs::write(
                dir.join("queries").join("highlights.scm"),
                format!("(string) @string ; generation {generation}\n"),
            )
            .unwrap();

            let loaded = crate::runtime::load(config.path(), BUILTIN_LANGUAGES);
            assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
            let registry = Arc::new(LanguageRegistry::with_runtime(
                BUILTIN_LANGUAGES,
                &loaded.entries,
                &[],
            ));
            let language = registry.language_by_id("freed").expect("freed");
            // What an editor open across the reload holds on to.
            still_open.push(
                registry
                    .compiled(language)
                    .expect("a runtime language compiles")
                    .expect("its queries compile"),
            );
            let Some(Def::Runtime(def)) = registry.def(language) else {
                panic!("a language loaded from disk must not be a catalog row");
            };
            generations.push(Arc::downgrade(def));
            drop(loaded);
            // Swapping the snapshot drops the one before it, exactly as
            // `reload` does.
            live = Some(registry);

            assert!(
                generations[..generation]
                    .iter()
                    .all(|weak| weak.upgrade().is_none()),
                "a replaced generation is still alive after {} reload(s)",
                generation + 1
            );
            assert!(generations[generation].upgrade().is_some());
        }

        drop(live);
        assert!(
            generations.iter().all(|weak| weak.upgrade().is_none()),
            "the last generation outlived the registry that held it"
        );

        // The grammar and queries the open editors hold are untouched by
        // any of that — a `fn` pointer into a grammar that is deliberately
        // never unloaded, and `Query`s that own their source.
        let compiled = still_open.last().expect("five generations");
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&compiled.grammar).expect("grammar");
        assert!(
            parser.parse("{\"a\": 1}", None).is_some(),
            "a compiled language still parses after its definition is freed"
        );
        assert!(compiled.highlights.is_some());
    }

    /// The suffix step, on the real catalog: it widens the extensionless
    /// languages to their `<name>.<suffix>` spellings without letting a
    /// suffix that *is* an extension lose to it, and without claiming a
    /// file no language ever named.
    #[test]
    fn a_suffixed_file_name_falls_back_to_the_bare_name() {
        let resolve = |name: &str| language_for_path(Path::new(name)).id().to_string();

        // 1. exact file name
        assert_eq!(resolve("Dockerfile"), "dockerfile");
        assert_eq!(resolve("Makefile"), "make");
        // 2. extension
        assert_eq!(resolve("build.dockerfile"), "dockerfile");
        // 3. file name minus its final suffix
        assert_eq!(resolve("Dockerfile.dev"), "dockerfile");
        assert_eq!(resolve("Dockerfile.prod"), "dockerfile");
        assert_eq!(resolve("/srv/app/Makefile.local"), "make");
        // The conflicting case: `md` is a real extension, and step 2 runs
        // before step 3, so this is Markdown and not a Dockerfile.
        assert_eq!(resolve("Dockerfile.md"), "markdown");
        // Not a match: nothing claims the bare name, so the suffix rule
        // must not invent a claimant.
        assert_eq!(resolve("notes.dev"), "plaintext");
        assert_eq!(resolve("Dockerfilez.dev"), "plaintext");
    }

    #[test]
    fn extension_matching_is_case_insensitive_and_path_aware() {
        assert_eq!(language_for_path(Path::new("/src/A.RS")).id(), "rust");
        assert_eq!(language_for_path(Path::new("noext")), Language::PLAIN_TEXT);
        assert_eq!(language_for_path(Path::new("")), Language::PLAIN_TEXT);
    }
}
