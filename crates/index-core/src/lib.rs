//! Project-wide text index: hybrid tantivy(ngram-3 candidate narrowing) +
//! ripgrep-crate (exact verification) search, per ADR-0008.
//!
//! Qt-free by design (see `docs/architecture/layering.md`) — this crate only
//! moves bytes and offsets around. Wiring it into a background
//! `std::thread` + `CxxQtThread::queue()`, and driving re-indexing off
//! `project_model::ProjectWatcher`, is `ui-shell`'s job; progress crosses
//! that seam as [`IndexProgress`], not as anything Qt-shaped.
//!
//! # Two-stage search
//!
//! [`TextIndex::search`] narrows the whole project down to a small set of
//! candidate files using a tantivy index whose `content` field is tokenized
//! with an ngram(3) tokenizer (fast substring-candidate matching — NOT
//! tantivy's default word tokenizer, which would miss mid-word substring
//! matches). Each candidate file is then re-scanned with `grep-searcher` /
//! `grep-regex` / `grep-matcher` (ripgrep's own library crates) to produce
//! exact line numbers and byte-offset match spans — real regex semantics,
//! ripgrep-grade correctness, index-backed speed on large repos.
//!
//! # Three schemas, one index
//!
//! One `TextIndex` carries three document shapes in the same tantivy
//! `Schema`/on-disk index under `.ide-index/`, distinguished by a `doc_type`
//! field (`"text"`, `"symbol"`, `"inherit"`): the **text** schema above
//! (`path` + `content`); the **symbol/reference** schema (`sym_name`/
//! `sym_kind`/`path`/`sym_line`/`sym_col`/`sym_container`/
//! `sym_is_definition`) — one document per (file, name), carrying every
//! occurrence of that name in that file as index-aligned multi-valued
//! fields, so a definition query has to re-filter the expanded rows — fed by
//! `syntax_core::outline()` (definitions, with kind + container) and
//! `syntax_core::identifier_occurrences()` (every occurrence, references
//! included) — see [`TextIndex::find_definitions`]/[`find_usages`]/
//! [`TextIndex::resolve_declaration`]; and the **supertype-edge** schema
//! (`inh_type`/`inh_supertype`), fed by `syntax_core::supertype_edges()`,
//! behind [`TextIndex::find_implementations`]/[`find_supertypes`]. A single
//! tantivy `Schema` can't hold two independently-typed document kinds, so
//! this is the standard workaround: one shared schema, a discriminant field,
//! queries filter on it. Kept in the same index (not co-located ones)
//! because tantivy's schema model makes this straightforward — no extra
//! `IndexWriter`/`IndexReader` pairs, no extra on-disk directories, one
//! `build()`/`sync_paths()` walk populates all three. They also share the
//! `path` term, so `delete_term(path)` drops every document a file
//! produced, whatever its shape.
//!
//! Name-based, not type-resolved (ADR-0008): two unrelated `run()` methods
//! in different classes both matching a usage search for `run` is expected
//! behavior, not a bug — there is no cross-file type/binding inference here.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Utf32Str};
use rayon::prelude::*;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::{
    IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, INDEXED, STORED, STRING,
};
use tantivy::tokenizer::NgramTokenizer;

// F3-15: kept in its own file rather than grown into this one, which is
// grandfathered at a ratcheted line-count ceiling (`scripts/check-file-size.sh`)
// that may only shrink.
pub mod excludes;
pub mod lifecycle;
mod replace_preview;
pub use replace_preview::FileDiffPreview;
use tantivy::{doc, Index, IndexReader, IndexWriter, Term};

use syntax_core::{analyze_file, language_for_path, SymbolKind, SymbolNode};

/// Directory (relative to the project root) the tantivy index lives under.
const INDEX_DIR_NAME: &str = ".ide-index";

/// Bump when what gets *extracted* from a file changes (a query in
/// `syntax-core/queries`, [`analyze`]'s rules) without the tantivy schema
/// changing: an existing index is then rebuilt instead of serving symbols
/// the old extraction missed. 2: Java/C/C++ `type_identifier`, PHP type
/// positions, Zig anchored variable names. 3: one symbol document per
/// (file, name) instead of one per occurrence.
const EXTRACTION_VERSION: u32 = 3;
const EXTRACTION_VERSION_FILE: &str = "extraction.version";

/// Tantivy's own writer-lock file inside the index directory. Named here so
/// an error can point at the exact path rather than at the project root.
const WRITER_LOCK_FILE: &str = ".tantivy-writer.lock";

/// Sidecar file holding `mtime<TAB>size<TAB>path` for every indexed file.
///
/// Recovering those stamps from the index itself means deserialising every
/// stored document, which is the whole cost of opening a project that has not
/// changed. A stale or missing sidecar is safe by construction: a stamp that
/// is missing or does not match makes the file be re-read, so the worst it
/// can cause is work, never a stale index.
const STAMPS_FILE: &str = "stamps.tsv";

/// File the lock probe below takes and releases. Never read.
const LOCK_PROBE_FILE: &str = ".ide-lock-probe";

/// Ngram size used for the `content` field's tokenizer — see module docs.
const NGRAM_SIZE: usize = 3;

/// Heap tantivy may use per indexing thread before it flushes a segment.
///
/// The thread count matters more than the number itself: `Index::writer`
/// derives its thread count by dividing the *total* budget by tantivy's
/// 15 MB-per-thread minimum, so the old flat 50 MB total silently capped
/// indexing at three threads no matter how many cores were available.
const WRITER_HEAP_BYTES_PER_THREAD: usize = 32_000_000;

/// Tantivy's own ceiling (`MAX_NUM_THREAD`); asking for more is rejected.
const MAX_WRITER_THREADS: usize = 8;

/// Files larger than this are indexed by name only — no content, no symbols.
///
/// A minified bundle or a multi-megabyte log is ngram(3)-tokenized byte by
/// byte, so a handful of them can cost more than the entire rest of a
/// project, and nobody is reading them in an editor anyway.
// ponytail: a flat byte cap, in the same range as mainstream IDEs use. A
// per-language cap, or "index the first N bytes", would be the upgrade if
// something legitimate turns out to sit just above the line.
const MAX_INDEXED_BYTES: u64 = 2 * 1024 * 1024;

/// How much of a file is sniffed for a NUL byte before deciding it is binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// How long to keep retrying the writer lock before reporting it busy. An
/// instance that has just been closed still holds the lock for as long as
/// its `IndexWriter` needs to flush and drop, so "reopen the project right
/// after quitting the other window" should wait rather than fail.
const WRITER_LOCK_RETRIES: u32 = 10;
const WRITER_LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// Where this project's index lives.
///
/// Normally `<project_root>/.ide-index/`, which keeps a project's index with
/// the project. But tantivy takes an advisory lock on a file in that
/// directory, and some filesystems cannot do advisory locks at all — a
/// Windows build reading a WSL tree over `\\wsl.localhost`, an SMB or NFS
/// share, some FUSE mounts. There the lock attempt fails for a reason that
/// has nothing to do with another instance, tantivy reports it as
/// `LockBusy` all the same, and the project simply never indexes.
///
/// So the directory is probed first, and when it cannot host a lock the
/// index goes to the user's cache directory instead, keyed by the project's
/// path. Slightly worse (two projects at the same path collide, and the
/// cache is not portable with the project) and much better than not working.
/// W6-2 (ADR-0052): this already handles a WSL project root correctly —
/// `\\wsl.localhost\...` is exactly the "some FUSE mounts" case above, and
/// falls back to the cache directory the same as any other lock-incapable
/// filesystem. What it does not fix, and this plan does not either, is the
/// *speed* of the first index: every file is read once, over the 9P share,
/// which is the slow leg of this whole design (file I/O stays Windows-side
/// by design — see the plan's Context section). Stated, not fixed; no
/// upgrade path is proposed, since the fix would mean file I/O moving off
/// the share, which is the one thing this plan deliberately does not do.
pub fn index_dir_for(project_root: &Path) -> PathBuf {
    index_dir_with(project_root, supports_file_locks)
}

/// [`index_dir_for`]'s rule, with the probe injected.
///
/// The probe is a parameter because the condition it detects cannot be
/// created on a normal filesystem from inside a test: POSIX locks are held
/// per process, so a test cannot make its own directory refuse a lock to
/// itself. Injecting it tests the decision, which is the part with a choice
/// in it; `supports_file_locks` is covered separately against a real
/// directory.
fn index_dir_with(project_root: &Path, can_lock: impl Fn(&Path) -> bool) -> PathBuf {
    let in_project = project_root.join(INDEX_DIR_NAME);
    if can_lock(&in_project) {
        return in_project;
    }
    fallback_index_dir(project_root).unwrap_or(in_project)
}

/// Can a lock actually be taken in `dir`?
///
/// Creates the directory if needed and takes — then immediately releases —
/// an exclusive lock on a probe file, using the same crate tantivy uses, so
/// this tests the mechanism tantivy will rely on rather than a proxy for it.
/// Any failure, including one that reports itself as "busy", counts as "no":
/// the probe file is ours alone, so nothing else can legitimately hold it.
fn supports_file_locks(dir: &Path) -> bool {
    use fs4::FileExt;

    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(LOCK_PROBE_FILE);
    let Ok(file) = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&probe)
    else {
        return false;
    };
    let locked = file.try_lock_exclusive().is_ok();
    if locked {
        let _ = file.unlock();
    }
    // Leaving the probe file behind would put a stray dotfile in every
    // project; it has served its purpose the moment the lock was answered.
    drop(file);
    let _ = fs::remove_file(&probe);
    locked
}

/// `<cache_dir>/ide/index/<sanitised-project-path>`, the home for an index
/// whose project directory cannot hold one.
fn fallback_index_dir(project_root: &Path) -> Option<PathBuf> {
    let key: String = project_root
        .to_string_lossy()
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
        .collect();
    Some(dirs::cache_dir()?.join("ide").join("index").join(key))
}

/// Acquire the index directory's single writer lock, retrying briefly (see
/// [`WRITER_LOCK_RETRIES`]) before reporting it as held by someone else.
///
/// Tantivy allows exactly one `IndexWriter` per directory, enforced with an
/// OS lock on `.tantivy-writer.lock`, so a second IDE instance pointed at
/// the same project cannot index it. That is reported as
/// [`IndexError::Locked`] — never as a reason to rebuild, since rebuilding
/// deletes the very files the live writer is using.
fn acquire_writer(index: &Index, index_dir: &Path) -> Result<IndexWriter, IndexError> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_WRITER_THREADS);
    let mut attempts = 0;
    loop {
        match index.writer_with_num_threads(threads, threads * WRITER_HEAP_BYTES_PER_THREAD) {
            Ok(writer) => return Ok(writer),
            Err(e @ tantivy::TantivyError::LockFailure(_, _)) => {
                if attempts >= WRITER_LOCK_RETRIES {
                    // Deliberately not "another instance is running": tantivy
                    // maps *every* lock failure to LockBusy, including "this
                    // filesystem does not support advisory locks", so the
                    // cause is genuinely unknown here. Naming the lock file
                    // is what lets the user check for themselves.
                    return Err(IndexError::Locked(format!(
                        "could not acquire the index writer lock at {}. \
                         Either another IDE instance has this project open, \
                         or this filesystem does not support file locking \
                         (a network share or a \\\\wsl.localhost path). \
                         If no other instance is running, close the project \
                         and reopen it: {e}",
                        index_dir.join(WRITER_LOCK_FILE).display()
                    )));
                }
                attempts += 1;
                std::thread::sleep(WRITER_LOCK_RETRY_DELAY);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Typed error crossing this crate's API (ADR-0003's typed-error
/// convention, applied ahead of this crate ever reaching an FFI seam).
#[derive(Debug)]
pub enum IndexError {
    Io(String),
    Tantivy(String),
    Query(String),
    /// Another `IndexWriter` — another IDE instance, or a previous one that
    /// has not exited — holds the on-disk index lock. Distinct from
    /// [`IndexError::Tantivy`] because it must never be answered by
    /// rebuilding: that would delete an index a live writer is still using.
    Locked(String),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::Io(msg) => write!(f, "index I/O error: {msg}"),
            IndexError::Tantivy(msg) => write!(f, "tantivy error: {msg}"),
            IndexError::Query(msg) => write!(f, "invalid search query: {msg}"),
            IndexError::Locked(msg) => write!(f, "index is locked: {msg}"),
        }
    }
}

impl std::error::Error for IndexError {}

impl From<tantivy::TantivyError> for IndexError {
    fn from(e: tantivy::TantivyError) -> Self {
        match e {
            tantivy::TantivyError::LockFailure(_, _) => IndexError::Locked(e.to_string()),
            _ => IndexError::Tantivy(e.to_string()),
        }
    }
}

impl From<std::io::Error> for IndexError {
    fn from(e: std::io::Error) -> Self {
        IndexError::Io(e.to_string())
    }
}

/// One exact match produced by the verification stage. `line` is 1-based
/// (matching `grep-searcher`'s convention); `start`/`end` are byte offsets
/// into that line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub start: usize,
    pub end: usize,
    /// The whole line the match sits on, as the verification pass already
    /// had it in hand. Carried here so result lists never have to re-read
    /// the file to show a snippet.
    pub line_text: String,
}

/// One fuzzy file-name hit from [`TextIndex::find_files`]. `positions` are
/// character (not byte) offsets into `relative`, for match highlighting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    pub path: PathBuf,
    pub relative: String,
    pub score: u32,
    pub positions: Vec<u32>,
}

/// One indexed file, kept in memory for the file-name tier of Search
/// Everywhere. Fuzzy-matching a flat slice of paths is microseconds even at
/// 100k files, so this tier deliberately does not go through tantivy.
struct FileEntry {
    path: PathBuf,
    relative: String,
}

/// One span to rewrite, addressed exactly like the [`SearchMatch`] it came
/// from: 1-based `line`, byte offsets within that line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReplacement {
    pub path: PathBuf,
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// What [`TextIndex::replace_in_files`] actually did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplaceReport {
    pub files: usize,
    pub matches: usize,
    /// Files left untouched because they changed since the search, or could
    /// not be read/written.
    pub skipped_files: usize,
}

struct Fields {
    path: tantivy::schema::Field,
    content: tantivy::schema::Field,
    doc_type: tantivy::schema::Field,
    sym_name: tantivy::schema::Field,
    sym_kind: tantivy::schema::Field,
    sym_container: tantivy::schema::Field,
    sym_line: tantivy::schema::Field,
    sym_col: tantivy::schema::Field,
    sym_is_definition: tantivy::schema::Field,
    inh_type: tantivy::schema::Field,
    inh_supertype: tantivy::schema::Field,
    /// Text docs only: file modification time (seconds since the epoch) and
    /// byte length as of the last indexing pass. Together they are the
    /// change-detection key [`TextIndex::open_or_build`] uses to skip
    /// re-reading unchanged files on project open.
    mtime_secs: tantivy::schema::Field,
    size_bytes: tantivy::schema::Field,
}

/// Discriminant values for the `doc_type` field — see the module doc's
/// "Three schemas, one index" section.
const DOC_TYPE_TEXT: &str = "text";
const DOC_TYPE_SYMBOL: &str = "symbol";
const DOC_TYPE_INHERIT: &str = "inherit";

fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    // Exact-match term field, used both to store the file's path and to
    // address a single document for delete-then-reinsert on reindex.
    // Shared by both doc types, so `delete_term(path)` on reindex removes a
    // file's text doc *and* all of its symbol docs in one go.
    let path = builder.add_text_field("path", STRING | STORED);
    let content_indexing = TextFieldIndexing::default()
        .set_tokenizer("ngram3")
        .set_index_option(IndexRecordOption::Basic);
    let content = builder.add_text_field(
        "content",
        TextOptions::default().set_indexing_options(content_indexing),
    );

    let doc_type = builder.add_text_field("doc_type", STRING | STORED);
    // Exact term field, not tokenized — `find_usages` needs exact-name
    // term matching; `find_definitions`' substring match is done in Rust
    // over the (already tantivy-narrowed-to-definitions) stored values,
    // per the plan doc's "exact substring match is the minimum bar".
    //
    // One symbol document now covers every occurrence of one name in one
    // file, so `sym_name` is single-valued while the five row fields below
    // are multi-valued and index-aligned: the nth value of each describes
    // the same occurrence. `symbol_docs` appends all five per row so they
    // cannot drift, and `collect_symbol_matches` zips them back apart.
    let sym_name = builder.add_text_field("sym_name", STRING | STORED);
    // Stored, not indexed: nothing queries a kind or a container, they are
    // only read back off a document that some other field already matched.
    let sym_kind = builder.add_text_field("sym_kind", STORED);
    let sym_container = builder.add_text_field("sym_container", STORED);
    let sym_line = builder.add_u64_field("sym_line", STORED);
    // Byte column within the line. Without it a jump can only land at
    // column 0; Go to Declaration wants the caret *on* the identifier.
    let sym_col = builder.add_u64_field("sym_col", STORED);
    // Indexed, because `find_definitions` narrows on it. A packed document
    // matches `sym_is_definition:1` when *any* of its rows is a definition,
    // so the reader filters the rows again after expanding them.
    let sym_is_definition = builder.add_u64_field("sym_is_definition", INDEXED | STORED);
    // Supertype edges (`doc_type = "inherit"`): `inh_type` declares
    // `inh_supertype`. They reuse the shared `path` term, so an existing
    // `delete_term(path)` on reindex still wipes them in one call.
    let inh_type = builder.add_text_field("inh_type", STRING | STORED);
    let inh_supertype = builder.add_text_field("inh_supertype", STRING | STORED);
    let mtime_secs = builder.add_u64_field("mtime_secs", STORED);
    let size_bytes = builder.add_u64_field("size_bytes", STORED);

    (
        builder.build(),
        Fields {
            path,
            content,
            doc_type,
            sym_name,
            sym_kind,
            sym_container,
            sym_line,
            sym_col,
            sym_is_definition,
            inh_type,
            inh_supertype,
            mtime_secs,
            size_bytes,
        },
    )
}

/// File metadata used to decide whether an indexed file needs re-reading.
/// A file whose modification time *and* byte length both match what the
/// index recorded is assumed unchanged — the same heuristic every build
/// system uses, and the reason a warm project open costs a directory walk
/// rather than a full re-index.
// ponytail: second-granularity mtime plus size, so an edit that keeps a
// file's byte length and lands in the same second as the last indexing pass
// is missed on project open — the watcher-driven `reindex_file` path covers
// live edits, so this only affects changes made while the IDE was closed.
// Upgrade to a content hash if that ever bites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct FileStamp {
    mtime_secs: u64,
    size_bytes: u64,
}

fn stamp_of(path: &Path) -> FileStamp {
    match fs::metadata(path) {
        Ok(meta) => stamp_from(&meta),
        Err(_) => FileStamp::default(),
    }
}

/// Read the stamp sidecar. `None` when it is absent or unreadable — the
/// caller then falls back to recovering stamps from the index itself.
fn read_stamps(index_dir: &Path) -> Option<HashMap<String, FileStamp>> {
    let raw = fs::read_to_string(index_dir.join(STAMPS_FILE)).ok()?;
    let mut stamps = HashMap::new();
    for line in raw.lines() {
        // Path last, so a path containing a tab still round-trips.
        let mut parts = line.splitn(3, '\t');
        let (Some(mtime), Some(size), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let (Ok(mtime_secs), Ok(size_bytes)) = (mtime.parse(), size.parse()) else {
            return None;
        };
        stamps.insert(
            path.to_string(),
            FileStamp {
                mtime_secs,
                size_bytes,
            },
        );
    }
    Some(stamps)
}

/// Replace the stamp sidecar. Written to a temporary name and renamed, so a
/// crash mid-write leaves the previous file rather than a truncated one.
/// Failures are silent on purpose: the sidecar is a cache, and losing it
/// costs one slow open.
fn write_stamps(index_dir: &Path, stamps: &[(String, FileStamp)]) {
    let mut out = String::with_capacity(stamps.len() * 48);
    for (key, stamp) in stamps {
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            stamp.mtime_secs, stamp.size_bytes, key
        ));
    }
    let temp = index_dir.join(format!("{STAMPS_FILE}.tmp"));
    if fs::write(&temp, out).is_ok() {
        let _ = fs::rename(&temp, index_dir.join(STAMPS_FILE));
    }
}

fn stamp_from(meta: &fs::Metadata) -> FileStamp {
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    FileStamp {
        mtime_secs,
        size_bytes: meta.len(),
    }
}

fn symbol_kind_to_str(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Interface => "interface",
        SymbolKind::Method => "method",
        SymbolKind::Function => "function",
        SymbolKind::Field => "field",
        SymbolKind::Constant => "constant",
        SymbolKind::Property => "property",
        SymbolKind::Constructor => "constructor",
        SymbolKind::EnumMember => "enum_member",
    }
}

fn symbol_kind_from_str(s: &str) -> Option<SymbolKind> {
    match s {
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

/// One symbol definition or reference row, returned by
/// [`TextIndex::find_definitions`]/[`TextIndex::find_usages`]. `kind`/
/// `container` are `None` for occurrences `outline()` didn't also capture
/// as a definition (e.g. a plain reference with no `tags.scm` entry of its
/// own) — see [`TextIndex::index_symbols`] for how the two sources merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolMatch {
    pub name: String,
    pub kind: Option<SymbolKind>,
    pub path: PathBuf,
    /// 1-based, matching [`SearchMatch::line`].
    pub line: usize,
    /// Byte offset of the name token within its line (0-based), so a jump
    /// lands on the identifier rather than at the start of the line.
    pub col: usize,
    pub is_definition: bool,
    pub container: Option<String>,
}

/// Which tier of [`TextIndex::resolve_declaration`] produced the
/// candidates -- see that method's docs for why local-file candidates
/// outrank project-wide ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionTier {
    /// Definitions of the same name in the file the caret is in.
    LocalFile,
    /// Definitions elsewhere in the project, name-matched.
    Project,
    /// Nothing found -- no identifier under the caret, or no definition
    /// of it anywhere in the index.
    None,
}

/// What [`TextIndex::resolve_declaration`] found: the identifier under the
/// caret plus its candidate declaration sites, best first. `candidates` is
/// empty exactly when `tier` is [`ResolutionTier::None`]; `name` is empty
/// when the caret was not on an identifier at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub name: String,
    pub tier: ResolutionTier,
    pub candidates: Vec<SymbolMatch>,
}

/// Flattened `outline()` row, keyed by the *name token's* byte range
/// (`name_start..name_end`) — the same byte range `identifier_occurrences`
/// reports for that same identifier, which is how [`index_symbols`] merges
/// kind/container onto the matching occurrence instead of indexing it
/// twice.
struct FlatSymbol<'a> {
    kind: SymbolKind,
    container: Option<&'a str>,
}

/// One occurrence's row inside a packed symbol document.
struct SymbolRow<'a> {
    line: usize,
    col: usize,
    is_definition: bool,
    kind: Option<&'a str>,
    container: Option<&'a str>,
}

fn flatten_outline<'a>(
    nodes: &'a [SymbolNode],
    parent: Option<&'a str>,
    out: &mut BTreeMap<(usize, usize), FlatSymbol<'a>>,
) {
    for node in nodes {
        out.insert(
            (node.name_start, node.name_end),
            FlatSymbol {
                kind: node.kind,
                container: parent,
            },
        );
        flatten_outline(&node.children, Some(node.name.as_str()), out);
    }
}

/// 1-based line number (matching `grep-searcher`'s convention, as used by
/// [`SearchMatch::line`]) and 0-based byte column of `offset` within
/// `text`.
fn line_and_col_at(text: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(text.len());
    let before = &text.as_bytes()[..clamped];
    let line = 1 + before.iter().filter(|&&b| b == b'\n').count();
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    (line, clamped - line_start)
}

/// Byte offset of the first byte of every line in `text`, ascending, always
/// starting with 0.
///
/// Built once per file so that resolving a position costs a binary search
/// instead of a scan from byte 0. [`line_and_col_at`] on its own is fine for
/// the one-off callers, but the indexer resolves a position for *every*
/// identifier occurrence in a file, which made symbol extraction quadratic in
/// file size.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(text.len() / 32 + 1);
    starts.push(0);
    starts.extend(
        text.bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(i, _)| i + 1),
    );
    starts
}

/// Same result as [`line_and_col_at`], against a table from [`line_starts`].
fn line_and_col_from(starts: &[usize], offset: usize) -> (usize, usize) {
    let line = starts.partition_point(|&start| start <= offset).max(1);
    (line, offset - starts[line - 1])
}

use excludes::exclude_overrides;
pub use excludes::IndexOptions;
use lifecycle::no_progress;
pub use lifecycle::{IndexProgress, IndexSlot};

/// How much this rename knows about one of the sites it found.
///
/// The whole point of the type: without a language server this is name
/// matching, not binding resolution (ADR-0008), so the plan says which sites
/// it can stand behind and which it merely found — and the user decides the
/// rest. Presenting both as equally certain would be the dishonest option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteConfidence {
    /// The site is in the file that declares the symbol, and no other file
    /// declares anything by that name. Nothing else in the project can be
    /// what this token means.
    Resolved,
    /// The name matches. That is all that is known — it may be an unrelated
    /// symbol that happens to share it.
    Unverified,
}

/// One occurrence a name-based rename would rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameSite {
    pub path: PathBuf,
    /// 1-based, as [`SymbolMatch`] reports it.
    pub line: usize,
    /// Byte offset of the name within its line.
    pub col: usize,
    pub confidence: SiteConfidence,
    /// Whether this occurrence is a declaration rather than a use.
    pub is_definition: bool,
    /// Whether the site should start out ticked in the preview. Decided here
    /// rather than in the dialog, because it is a judgement about how much
    /// the rename knows, not about how the list is drawn.
    pub checked: bool,
}

/// What a rename without a language server would do, and how sure it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRenamePlan {
    pub name: String,
    pub sites: Vec<RenameSite>,
    /// Whether more than one symbol in the project is declared with this
    /// name. When true the sites cannot be told apart by name alone, so the
    /// unverified ones start unticked and the preview says why.
    pub ambiguous: bool,
}

impl IndexRenamePlan {
    /// The sites that start out ticked.
    pub fn checked_sites(&self) -> impl Iterator<Item = &RenameSite> {
        self.sites.iter().filter(|site| site.checked)
    }
}

/// Why a name-based rename cannot be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameRefusal {
    /// The caret is not on anything this index resolved to a declaration.
    /// Renaming every token that happens to be spelled the same is a
    /// find-and-replace, which this application already has, and offering it
    /// under the name "rename" would be a lie about what it does.
    Unresolved,
    /// The new name is not an identifier.
    InvalidName,
    /// A file is open with unsaved changes. The index reads from disk, so
    /// every line and column it reports for a modified buffer may be stale;
    /// rewriting from those would corrupt the file. One rule, one message,
    /// rather than per-file semantics nobody can predict.
    UnsavedChanges,
    /// The symbol was resolved, but nothing — not even its declaration —
    /// came back as an occurrence.
    NoSites,
}

impl std::fmt::Display for RenameRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameRefusal::Unresolved => write!(
                f,
                "There is no symbol under the caret to rename. Use Replace in Files to change every occurrence of a word."
            ),
            RenameRefusal::InvalidName => {
                write!(f, "That is not a valid name.")
            }
            RenameRefusal::UnsavedChanges => write!(
                f,
                "Save all files before renaming without a language server \u{2014} the project index reads from disk."
            ),
            RenameRefusal::NoSites => {
                write!(f, "No occurrences of this symbol were found in the project.")
            }
        }
    }
}

impl std::error::Error for RenameRefusal {}

/// Is `name` something that could be an identifier?
///
/// Deliberately conservative and language-agnostic: a leading letter or
/// underscore, then letters, digits and underscores. It exists to catch the
/// empty box and the pasted sentence, not to model 31 grammars — a name this
/// rejects that some language would accept is a smaller cost than a rename
/// that writes `foo bar` across a project.
pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_') && chars.all(|ch| ch.is_alphanumeric() || ch == '_')
}

/// What a name-based rename would change, or why it will not be offered.
///
/// `resolution` is what the caret resolved to ([`TextIndex::resolve_declaration`]),
/// `usages` every occurrence of that name ([`TextIndex::find_usages`]), and
/// `definitions` every declaration of it ([`TextIndex::find_definitions_exact`]).
/// `has_unsaved_changes` is the editor's answer about its own buffers, which
/// only it can know.
///
/// The confidence rule: a site is [`SiteConfidence::Resolved`] when it lives
/// in the file that declares the symbol *and* the project declares that name
/// exactly once — in which case nothing else it could refer to exists.
/// Everything else is [`SiteConfidence::Unverified`], and starts unticked
/// whenever the name is ambiguous, so the default action is the safe one and
/// widening it is a deliberate click.
pub fn plan_index_rename(
    resolution: &Resolution,
    usages: &[SymbolMatch],
    definitions: &[SymbolMatch],
    new_name: &str,
    has_unsaved_changes: bool,
) -> Result<IndexRenamePlan, RenameRefusal> {
    if resolution.tier == ResolutionTier::None || resolution.name.is_empty() {
        return Err(RenameRefusal::Unresolved);
    }
    if !is_valid_identifier(new_name) {
        return Err(RenameRefusal::InvalidName);
    }
    if has_unsaved_changes {
        return Err(RenameRefusal::UnsavedChanges);
    }

    let declaring_path = resolution.candidates.first().map(|c| c.path.as_path());
    let ambiguous = definitions.len() > 1;

    let sites: Vec<RenameSite> = usages
        .iter()
        .map(|usage| {
            let resolved = !ambiguous && declaring_path == Some(usage.path.as_path());
            let confidence = if resolved {
                SiteConfidence::Resolved
            } else {
                SiteConfidence::Unverified
            };
            RenameSite {
                path: usage.path.clone(),
                line: usage.line,
                col: usage.col,
                confidence,
                is_definition: usage.is_definition,
                checked: resolved || !ambiguous,
            }
        })
        .collect();

    if sites.is_empty() {
        return Err(RenameRefusal::NoSites);
    }
    Ok(IndexRenamePlan {
        name: resolution.name.clone(),
        sites,
        ambiguous,
    })
}

/// One ticked rename site addressed the way an open editor wants it:
/// 0-based line, UTF-16 characters, which is what `QTextCursor` counts.
///
/// The rename plan itself is byte-addressed, like everything else the index
/// reports. Converting here rather than in the view keeps the one place that
/// knows both unit systems in Qt-free code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferRenameEdit {
    pub path: PathBuf,
    /// 0-based, unlike [`RenameSite::line`] — the editor counts from zero.
    pub line: u32,
    pub start_character: u32,
    pub end_character: u32,
    pub text: String,
}

/// Take the ticked sites in `path` out of `plan`, as edits an open editor
/// can splice.
///
/// A file the user has open must not be rewritten behind their back: doing
/// so loses the undo history and makes the editor prompt about a change it
/// made itself. So the sites in open files are handed to the editor and the
/// rest go to disk — the same split `lsp_core::plan_edit` makes for a
/// server-driven edit, applied to the name-based path.
///
/// Reading the file to convert byte columns to UTF-16 is sound precisely
/// because [`plan_index_rename`] refuses to plan at all while any buffer is
/// unsaved: an open file is therefore identical to the one on disk. Sites
/// are removed from `plan` whether or not their file could be read, so
/// nothing is applied twice, and are returned last-first so a caller can
/// splice them in one pass.
pub fn take_buffer_edits(
    plan: &mut IndexRenamePlan,
    new_name: &str,
    path: &Path,
) -> Vec<BufferRenameEdit> {
    let name_len = plan.name.len();
    // An unticked site in an open file is dropped either way: the user said
    // no to it. Everything in another file stays for the disk pass.
    let (mine, remaining): (Vec<RenameSite>, Vec<RenameSite>) = std::mem::take(&mut plan.sites)
        .into_iter()
        .partition(|site| site.path == path);
    plan.sites = remaining;
    let taken: Vec<RenameSite> = mine.into_iter().filter(|site| site.checked).collect();

    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let mut edits: Vec<BufferRenameEdit> = taken
        .into_iter()
        .filter_map(|site| {
            let line = lines.get(site.line.checked_sub(1)?)?;
            let start = utf16_len(line.get(..site.col)?);
            let end = start + utf16_len(line.get(site.col..site.col + name_len)?);
            Some(BufferRenameEdit {
                path: site.path.clone(),
                line: site.line as u32 - 1,
                start_character: start,
                end_character: end,
                text: new_name.to_string(),
            })
        })
        .collect();
    // Last first, so each edit still addresses the text it was found in.
    edits.sort_by(|a, b| {
        b.line
            .cmp(&a.line)
            .then(b.start_character.cmp(&a.start_character))
    });
    edits
}

fn utf16_len(text: &str) -> u32 {
    text.chars().map(|ch| ch.len_utf16() as u32).sum()
}

/// The sites of `plan` that are ticked, as the spans
/// [`TextIndex::replace_in_files`] rewrites.
///
/// This is the one place an LSP-free rename and the existing Replace in
/// Files meet: a rename site is a single-line span of a known length, which
/// is exactly what [`FileReplacement`] describes, so the applier is reused
/// rather than reimplemented.
pub fn rename_replacements(plan: &IndexRenamePlan, new_name: &str) -> Vec<FileReplacement> {
    plan.checked_sites()
        .map(|site| FileReplacement {
            path: site.path.clone(),
            line: site.line,
            start: site.col,
            end: site.col + plan.name.len(),
            text: new_name.to_string(),
        })
        .collect()
}

/// The declaration at `line` of `path`, as the text to show in a tooltip.
///
/// The fallback for hover when no language server answers: there is no
/// stored signature anywhere in this index, and the honest substitute is the
/// declaration's own source text.
///
/// ponytail: bracket balance, capped at [`SIGNATURE_MAX_LINES`]. It reads a
/// multi-line signature correctly and stops at the body, and it will include
/// a trailing comment or miscount a bracket inside a string. A real answer
/// means asking the grammar for the declaration node's range — worth doing
/// if this proves annoying, and not before.
pub fn declaration_signature(path: &Path, line: usize) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    signature_from_text(&content, line)
}

/// How many lines of a declaration are worth showing before it stops being a
/// signature and starts being the function.
pub const SIGNATURE_MAX_LINES: usize = 5;

fn signature_from_text(content: &str, line: usize) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let first = lines.get(line.checked_sub(1)?)?;

    let mut out = vec![first.trim_end().to_string()];
    let mut depth = bracket_depth(first);
    // Keep taking continuation lines while a bracket is still open: a
    // signature broken across lines is unreadable when truncated at the
    // first one.
    while depth > 0 && out.len() < SIGNATURE_MAX_LINES {
        let Some(next) = lines.get(line - 1 + out.len()) else {
            break;
        };
        depth += bracket_depth(next);
        out.push(next.trim_end().to_string());
    }

    let mut signature = out.join("\n");
    // The body is not part of the signature. The brace that opens it is the
    // last one with nothing but the body's own close after it, which covers
    // both `fn f() {` and the one-line `fn f() {}`. A brace with anything
    // else after it belongs to the declaration — a struct literal in a
    // default argument, a block expression in an initialiser — and stays.
    if let Some(open) = signature.rfind('{') {
        if signature[open + 1..]
            .chars()
            .all(|ch| ch.is_whitespace() || ch == '}')
        {
            signature.truncate(open);
        }
    }
    let signature = signature.trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

/// How far the bracket nesting moves across one line.
fn bracket_depth(line: &str) -> i32 {
    line.chars().fold(0, |depth, ch| match ch {
        '(' | '[' | '<' => depth + 1,
        ')' | ']' | '>' => depth - 1,
        _ => depth,
    })
}

/// Turns "these matched spans, this pattern, this replacement" into the
/// concrete per-span replacement text [`TextIndex::replace_in_files`]
/// applies. Callers hand in the spans a search produced; a span whose line
/// no longer holds it (the file changed since) is dropped, and
/// `replace_in_files` then counts that file as skipped rather than
/// rewriting the wrong bytes.
///
/// The expansion runs `editor_core::replacements` over just the matched
/// slice, so a regex `$1` refers to the capture inside that match — the same
/// engine the in-editor Replace uses, not a second implementation.
pub fn resolve_replacements(
    edits: &[(PathBuf, usize, usize, usize)],
    pattern: &str,
    replacement: &str,
    opts: editor_core::SearchOptions,
) -> Result<Vec<FileReplacement>, String> {
    let mut out = Vec::with_capacity(edits.len());
    let mut cached: Option<(&Path, Vec<String>)> = None;
    for (path, line, start, end) in edits {
        if cached.as_ref().is_none_or(|(p, _)| *p != path.as_path()) {
            let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
            cached = Some((path.as_path(), content.lines().map(str::to_owned).collect()));
        }
        let lines = &cached.as_ref().expect("just populated").1;
        let Some(matched) = lines
            .get(line.wrapping_sub(1))
            .and_then(|l| l.get(*start..*end))
        else {
            // The file changed since the search; `replace_in_files` counts
            // this file as skipped rather than rewriting the wrong span.
            continue;
        };
        let expanded = editor_core::replacements(matched, pattern, replacement, opts)
            .map_err(|e| e.to_string())?;
        let Some(first) = expanded.into_iter().next() else {
            continue;
        };
        out.push(FileReplacement {
            path: path.clone(),
            line: *line,
            start: *start,
            end: *end,
            text: first.text,
        });
    }
    Ok(out)
}

/// A tantivy-backed text index for one project root, with ripgrep-crate
/// verification on top (see module docs for the two-stage design).
pub struct TextIndex {
    root: PathBuf,
    /// Where this index actually lives. Usually `<root>/.ide-index/`, but
    /// [`index_dir_for`] falls back to the user's cache directory on a
    /// filesystem that cannot take an advisory lock, so it cannot be
    /// re-derived from the root without probing for a lock all over again.
    index_dir: PathBuf,
    /// What this index skips beyond `.gitignore`, kept so the incremental
    /// pass applies the same rules the build did.
    options: IndexOptions,
    index: Index,
    writer: IndexWriter,
    reader: IndexReader,
    fields: Fields,
    files: Vec<FileEntry>,
}

impl TextIndex {
    /// Build a fresh index at `<project_root>/.ide-index/`, walking
    /// `project_root` via the `ignore` crate (gitignore-aware, same default
    /// behavior as ripgrep — hidden files and `.gitignore`/`.ignore`
    /// entries excluded unless explicitly un-ignored). Any existing index
    /// directory is replaced.
    pub fn build(project_root: &Path) -> Result<Self, IndexError> {
        Self::build_with_progress(project_root, &IndexOptions::default(), &no_progress)
    }

    /// [`build`](Self::build), reporting how far along it is. See
    /// [`IndexProgress`].
    pub fn build_with_progress(
        project_root: &Path,
        options: &IndexOptions,
        progress: &(dyn Fn(IndexProgress) + Sync),
    ) -> Result<Self, IndexError> {
        let index_dir = index_dir_for(project_root);
        if index_dir.exists() {
            fs::remove_dir_all(&index_dir)?;
        }
        fs::create_dir_all(&index_dir)?;

        let (schema, fields) = build_schema();
        let index = Index::create_in_dir(&index_dir, schema)?;
        fs::write(
            index_dir.join(EXTRACTION_VERSION_FILE),
            EXTRACTION_VERSION.to_string(),
        )?;
        index.tokenizers().register(
            "ngram3",
            NgramTokenizer::new(NGRAM_SIZE, NGRAM_SIZE, false)?,
        );

        let writer = acquire_writer(&index, &index_dir)?;
        let reader = index.reader()?;
        let mut this = Self {
            root: project_root.to_path_buf(),
            index_dir: index_dir.clone(),
            options: options.clone(),
            index,
            writer,
            reader,
            fields,
            files: Vec::new(),
        };
        this.sync_from_disk(&HashMap::new(), progress)?;
        Ok(this)
    }

    /// Open the index already stored under `<project_root>/.ide-index/` and
    /// bring it up to date, re-reading only files whose modification time or
    /// size changed and dropping files that disappeared. Falls back to a
    /// full [`build`](Self::build) when there is no usable index there (first
    /// run, a schema change, or a corrupt directory).
    ///
    /// This is what a project open should call: an unchanged repository costs
    /// one directory walk plus a `stat` per file, not a full re-index.
    pub fn open_or_build(project_root: &Path) -> Result<Self, IndexError> {
        Self::open_or_build_with_progress(project_root, &IndexOptions::default(), &no_progress)
    }

    /// [`open_or_build`](Self::open_or_build), reporting how far along it is.
    /// See [`IndexProgress`].
    pub fn open_or_build_with_progress(
        project_root: &Path,
        options: &IndexOptions,
        progress: &(dyn Fn(IndexProgress) + Sync),
    ) -> Result<Self, IndexError> {
        match Self::open_existing(project_root, options) {
            Ok(mut index) => {
                let sidecar = read_stamps(&index.index_dir);
                let stamps = match sidecar {
                    Some(ref stamps) => stamps.clone(),
                    None => index.indexed_stamps()?,
                };
                index.sync_from_disk(&stamps, progress)?;
                if sidecar.is_none() {
                    // The pass only rewrites the sidecar when it changed
                    // something; a missing or damaged one has to be healed
                    // here, or every open pays the slow fallback forever.
                    index.write_current_stamps();
                }
                Ok(index)
            }
            // A busy lock is never answered by rebuilding: `build` wipes the
            // index directory, which would pull the files out from under the
            // writer that holds the lock and leave two writers disagreeing
            // about `meta.json`.
            Err(err @ IndexError::Locked(_)) => Err(err),
            Err(_) => Self::build_with_progress(project_root, options, progress),
        }
    }

    fn open_existing(project_root: &Path, options: &IndexOptions) -> Result<Self, IndexError> {
        let index_dir = index_dir_for(project_root);
        // The (mtime, size) stamps only detect changed *files*. When the
        // extraction itself changes — a locals.scm learns a node kind it
        // used to drop — every stamp still matches and the stale symbols
        // would be served forever. The version file is the switch that
        // forces the rebuild instead.
        let stored_version = fs::read_to_string(index_dir.join(EXTRACTION_VERSION_FILE))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        if stored_version != Some(EXTRACTION_VERSION) {
            return Err(IndexError::Tantivy(
                "index extraction version mismatch".to_string(),
            ));
        }
        let (schema, fields) = build_schema();
        let index = Index::open_in_dir(&index_dir)?;
        if index.schema() != schema {
            // An index written by an older schema is not readable field for
            // field; rebuilding is the only safe answer.
            return Err(IndexError::Tantivy("index schema mismatch".to_string()));
        }
        index.tokenizers().register(
            "ngram3",
            NgramTokenizer::new(NGRAM_SIZE, NGRAM_SIZE, false)?,
        );
        let writer = acquire_writer(&index, &index_dir)?;
        let reader = index.reader()?;
        Ok(Self {
            root: project_root.to_path_buf(),
            index_dir: index_dir.clone(),
            options: options.clone(),
            index,
            writer,
            reader,
            fields,
            files: Vec::new(),
        })
    }

    /// `path -> (mtime, size)` for every text doc currently in the index.
    fn indexed_stamps(&self) -> Result<HashMap<String, FileStamp>, IndexError> {
        let searcher = self.reader.searcher();
        let num_docs = searcher.num_docs() as usize;
        if num_docs == 0 {
            return Ok(HashMap::new());
        }
        let text_only = TermQuery::new(
            Term::from_field_text(self.fields.doc_type, DOC_TYPE_TEXT),
            IndexRecordOption::Basic,
        );
        let top_docs = searcher.search(&text_only, &TopDocs::with_limit(num_docs))?;
        let mut stamps = HashMap::with_capacity(top_docs.len());
        for (_score, address) in top_docs {
            let retrieved: tantivy::TantivyDocument = searcher.doc(address)?;
            let Some(path) = retrieved
                .get_first(self.fields.path)
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let u64_field = |field| {
                retrieved
                    .get_first(field)
                    .and_then(|v: &tantivy::schema::OwnedValue| v.as_u64())
                    .unwrap_or(0)
            };
            stamps.insert(
                path.to_string(),
                FileStamp {
                    mtime_secs: u64_field(self.fields.mtime_secs),
                    size_bytes: u64_field(self.fields.size_bytes),
                },
            );
        }
        Ok(stamps)
    }

    /// Walk the project, re-indexing every file whose stamp differs from
    /// `known` and removing indexed files that no longer exist, then rebuild
    /// the in-memory file list. One commit for the whole pass.
    ///
    /// Two passes on purpose. The walk is cheap and single-threaded, and
    /// finishing it before any file is read is what makes the number of files
    /// to index known up front — a progress report without a total is a
    /// spinner, not progress. The second pass is where all the cost is (read,
    /// tree-sitter parse, document build) and it runs in parallel.
    fn sync_from_disk(
        &mut self,
        known: &HashMap<String, FileStamp>,
        progress: &(dyn Fn(IndexProgress) + Sync),
    ) -> Result<(), IndexError> {
        let index_dir = self.index_dir.clone();
        let mut unchanged: Vec<(PathBuf, String, FileStamp)> = Vec::new();
        let mut stale: Vec<(PathBuf, String, FileStamp)> = Vec::new();

        let mut walker = ignore::WalkBuilder::new(&self.root);
        if let Some(overrides) = exclude_overrides(&self.root, &self.options.excludes) {
            walker.overrides(overrides);
        }
        for entry in walker.build() {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.starts_with(&index_dir) {
                continue;
            }
            // The walker already stat'ed this entry; re-stat'ing it here was
            // a second syscall per file for the same two numbers.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let key = path.to_string_lossy().into_owned();
            let stamp = stamp_from(&metadata);
            if known.get(&key) == Some(&stamp) {
                unchanged.push((path.to_path_buf(), key, stamp));
            } else {
                stale.push((path.to_path_buf(), key, stamp));
            }
        }

        let total = stale.len();
        progress(IndexProgress { done: 0, total });

        let done = AtomicUsize::new(0);
        // Whether this pass actually changed the index, as opposed to merely
        // looking at files. A file the index has no record of has nothing to
        // delete, and a file past the size cap contributes no documents, so
        // neither makes the pass dirty — which is what keeps a reopen of an
        // unchanged project from committing and reloading for nothing.
        let mutated = AtomicBool::new(false);
        let writer = &self.writer;
        let fields = &self.fields;
        let indexed: Vec<Option<(PathBuf, String, FileStamp)>> = stale
            .into_par_iter()
            .map(|(path, key, stamp)| {
                if known.contains_key(&key) {
                    writer.delete_term(Term::from_field_text(fields.path, key.as_str()));
                    mutated.store(true, Ordering::Relaxed);
                }
                let documents = file_docs(fields, &key, &path, stamp);
                let indexable = documents.is_some();
                for document in documents.unwrap_or_default() {
                    writer.add_document(document)?;
                    mutated.store(true, Ordering::Relaxed);
                }
                progress(IndexProgress {
                    done: done.fetch_add(1, Ordering::Relaxed) + 1,
                    total,
                });
                Ok(indexable.then_some((path, key, stamp)))
            })
            .collect::<Result<Vec<_>, IndexError>>()?;

        let mut seen: Vec<(PathBuf, String, FileStamp)> = unchanged;
        seen.extend(indexed.into_iter().flatten());

        let live: std::collections::HashSet<&str> =
            seen.iter().map(|(_, key, _)| key.as_str()).collect();
        let mut dirty = mutated.into_inner();
        for key in known.keys() {
            if !live.contains(key.as_str()) {
                self.writer
                    .delete_term(Term::from_field_text(self.fields.path, key));
                dirty = true;
            }
        }

        if dirty {
            self.writer.commit()?;
            self.reader.reload()?;
        }

        if dirty {
            let stamps: Vec<(String, FileStamp)> = seen
                .iter()
                .map(|(_, key, stamp)| (key.clone(), *stamp))
                .collect();
            write_stamps(&index_dir, &stamps);
        }

        self.files = seen
            .into_iter()
            .map(|(path, _, _)| FileEntry {
                relative: relative_display(&self.root, &path),
                path,
            })
            .collect();
        Ok(())
    }

    /// Delete any existing docs for `path` and re-add them from disk,
    /// without committing. Returns whether the file was indexable (readable
    /// UTF-8 text) — binary/unreadable files are dropped from the index and
    /// from the file list, since neither text nor file search can serve
    /// them.
    fn write_file_doc(&mut self, path: &Path, stamp: FileStamp) -> Result<bool, IndexError> {
        let key = path.to_string_lossy().into_owned();
        self.writer
            .delete_term(Term::from_field_text(self.fields.path, &key));
        let Some(documents) = file_docs(&self.fields, &key, path, stamp) else {
            return Ok(false);
        };
        for document in documents {
            self.writer.add_document(document)?;
        }
        Ok(true)
    }

    /// Rewrite the stamp sidecar from the file list currently in memory.
    fn write_current_stamps(&self) {
        let stamps: Vec<(String, FileStamp)> = self
            .files
            .iter()
            .map(|f| (f.path.to_string_lossy().into_owned(), stamp_of(&f.path)))
            .collect();
        write_stamps(&self.index_dir, &stamps);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `path` is part of the index's own on-disk storage.
    ///
    /// The index lives *inside* the project it indexes, so a filesystem
    /// watcher on the project root sees every commit this index makes. Acting
    /// on those events would re-index the index — which writes more index
    /// files, which produces more events, forever. Every mutating entry point
    /// filters through here so no caller can reintroduce that loop.
    pub fn is_index_internal(&self, path: &Path) -> bool {
        path.starts_with(&self.index_dir)
    }

    /// Number of files currently held in the file-name tier.
    pub fn indexed_file_count(&self) -> usize {
        self.files.len()
    }

    /// Re-index a single file: drops any existing entry for `path` — its
    /// text doc *and* every symbol/reference doc it produced, since they
    /// all share the `path` term — and re-adds it from disk if it
    /// currently exists and is readable UTF-8 text, including a fresh
    /// symbol/reference pass. Callers (the eventual `ui-shell` watcher
    /// integration) pass the same path form used when the file was first
    /// indexed.
    pub fn reindex_file(&mut self, path: &Path) -> Result<(), IndexError> {
        if self.is_index_internal(path) {
            return Ok(());
        }
        let indexable = self.write_file_doc(path, stamp_of(path))?;
        self.writer.commit()?;
        self.reader.reload()?;
        self.track_file(path, indexable);
        Ok(())
    }

    /// Bring a batch of paths up to date in one pass: each path that still
    /// exists is re-indexed, each that does not is dropped, and the whole
    /// batch shares a single commit and reader reload.
    ///
    /// This is what the filesystem watcher should call. One save can produce
    /// several events, and a commit per event is both the expensive part and
    /// a write lock the whole application waits behind.
    pub fn sync_paths(&mut self, paths: &[PathBuf]) -> Result<(), IndexError> {
        let mut touched: Vec<(PathBuf, bool)> = Vec::with_capacity(paths.len());
        for path in paths {
            if self.is_index_internal(path) {
                continue;
            }
            if path.exists() {
                let indexable = self.write_file_doc(path, stamp_of(path))?;
                touched.push((path.clone(), indexable));
            } else {
                let key = path.to_string_lossy().into_owned();
                self.writer
                    .delete_term(Term::from_field_text(self.fields.path, &key));
                touched.push((path.clone(), false));
            }
        }
        if touched.is_empty() {
            return Ok(());
        }
        self.writer.commit()?;
        self.reader.reload()?;
        for (path, present) in touched {
            self.track_file(&path, present);
        }
        Ok(())
    }

    /// Keep the file-name tier in step with a single-file index change.
    fn track_file(&mut self, path: &Path, present: bool) {
        let existing = self.files.iter().position(|f| f.path == path);
        match (existing, present) {
            (None, true) => self.files.push(FileEntry {
                relative: relative_display(&self.root, path),
                path: path.to_path_buf(),
            }),
            (Some(i), false) => {
                self.files.swap_remove(i);
            }
            _ => {}
        }
    }

    /// Write whole new contents for files a refactoring changed, and
    /// re-index each one.
    ///
    /// The counterpart of [`replace_in_files`](Self::replace_in_files) for
    /// edits that came from a language server: an LSP range spans lines, so
    /// there is nothing single-line to rewrite in place, and the caller has
    /// already produced the finished text with
    /// `lsp_core::workspace_edit::apply_to_text` — which validates every
    /// range before it produces anything, so a file arriving here is
    /// complete or was never built.
    ///
    /// Files that cannot be written are counted as skipped, exactly as the
    /// span-based path counts them, so one unwritable file does not abandon
    /// the rest of the refactoring.
    ///
    /// Open editors learn about this the same way they learn about Replace
    /// in Files: through the project watcher, not from here.
    pub fn write_files(
        &mut self,
        files: &[(PathBuf, String)],
    ) -> Result<ReplaceReport, IndexError> {
        let mut report = ReplaceReport::default();
        for (path, content) in files {
            if fs::write(path, content).is_err() {
                report.skipped_files += 1;
                continue;
            }
            report.files += 1;
            report.matches += 1;
            self.reindex_file(path)?;
        }
        Ok(report)
    }

    /// Drop `path`'s entry from the index so it no longer appears in
    /// subsequent `search`/`find_definitions`/`find_usages` results (its
    /// text doc and every symbol/reference doc it produced, all keyed by
    /// the shared `path` term).
    pub fn remove_file(&mut self, path: &Path) -> Result<(), IndexError> {
        if self.is_index_internal(path) {
            return Ok(());
        }
        let key = path.to_string_lossy().into_owned();
        self.writer
            .delete_term(Term::from_field_text(self.fields.path, &key));
        self.writer.commit()?;
        self.reader.reload()?;
        self.track_file(path, false);
        Ok(())
    }

    /// Go-to-file: fuzzy-rank the indexed file list against `query`, best
    /// first, at most `limit` hits. An empty query lists the first `limit`
    /// files, which is what an empty Search Everywhere box shows.
    ///
    /// Scores are computed first and only the winners get their match
    /// positions resolved, so a large project pays one cheap scoring pass
    /// rather than an allocation per candidate.
    pub fn find_files(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        if limit == 0 {
            return Vec::new();
        }
        if query.is_empty() {
            return self
                .files
                .iter()
                .take(limit)
                .map(|f| FileMatch {
                    path: f.path.clone(),
                    relative: f.relative.clone(),
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
        }

        let mut matcher = nucleo_matcher::Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();

        // Min-heap of the best `limit` hits so far: cheaper than sorting
        // every match on a big repo. `Reverse` puts the weakest survivor on
        // top, which is exactly the one to evict.
        let mut best: BinaryHeap<Reverse<(u32, Reverse<usize>, usize)>> = BinaryHeap::new();
        for (i, entry) in self.files.iter().enumerate() {
            let haystack = Utf32Str::new(&entry.relative, &mut buf);
            let Some(score) = pattern.score(haystack, &mut matcher) else {
                continue;
            };
            // Ties break towards the shorter path — the more specific hit.
            let key = Reverse((score, Reverse(entry.relative.len()), i));
            if best.len() < limit {
                best.push(key);
            } else if best.peek().is_some_and(|worst| key.0 > worst.0) {
                best.pop();
                best.push(key);
            }
        }

        let mut ranked: Vec<(u32, Reverse<usize>, usize)> =
            best.into_iter().map(|Reverse(key)| key).collect();
        ranked.sort_unstable_by(|a, b| b.cmp(a));

        ranked
            .into_iter()
            .map(|(score, _, i)| {
                let entry = &self.files[i];
                let mut positions = Vec::new();
                let haystack = Utf32Str::new(&entry.relative, &mut buf);
                pattern.indices(haystack, &mut matcher, &mut positions);
                positions.sort_unstable();
                positions.dedup();
                FileMatch {
                    path: entry.path.clone(),
                    relative: entry.relative.clone(),
                    score,
                    positions,
                }
            })
            .collect()
    }

    /// Find-in-Files: narrow candidate files via the ngram(3) tantivy index,
    /// then re-verify each candidate with `grep-searcher`/`grep-regex` for
    /// exact line/span matches. `pattern` is a literal substring unless
    /// `is_regex` is set, in which case it's a regex per the `regex` crate's
    /// syntax (what `grep-regex` implements).
    ///
    /// Every occurrence is reported, including several on one line — Replace
    /// in Files applies the spans this returns, so missing the second match
    /// on a line would silently leave it behind.
    pub fn search(
        &self,
        pattern: &str,
        is_regex: bool,
        case_sensitive: bool,
    ) -> Result<Vec<SearchMatch>, IndexError> {
        self.search_with(
            pattern,
            is_regex,
            case_sensitive,
            usize::MAX,
            &AtomicBool::new(false),
        )
    }

    /// [`search`](Self::search) with a result ceiling and a cancellation
    /// flag. Both matter for search-as-you-type: an abandoned keystroke's
    /// scan stops as soon as `cancel` flips rather than running the whole
    /// project to completion, and `limit` keeps a three-character query on a
    /// huge repo from materialising a million matches nobody will read.
    pub fn search_with(
        &self,
        pattern: &str,
        is_regex: bool,
        case_sensitive: bool,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<SearchMatch>, IndexError> {
        let owned_pattern;
        let regex_pattern: &str = if is_regex {
            pattern
        } else {
            owned_pattern = escape_literal(pattern);
            &owned_pattern
        };
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!case_sensitive)
            .build(regex_pattern)
            .map_err(|e| IndexError::Query(e.to_string()))?;

        let mut matches = Vec::new();
        for path in self.candidate_files(pattern, case_sensitive)? {
            if cancel.load(Ordering::Relaxed) || matches.len() >= limit {
                break;
            }
            let mut searcher = Searcher::new();
            let path_for_sink = path.clone();
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
                        if matches.len() >= limit {
                            return Ok(false);
                        }
                        matches.push(SearchMatch {
                            path: path_for_sink.clone(),
                            line: line_number as usize,
                            start: m.start(),
                            end: m.end(),
                            line_text: trimmed.to_string(),
                        });
                        // A zero-width match would otherwise pin `from` and
                        // spin forever on this line.
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
            // A candidate file that vanished/changed between the tantivy
            // narrowing step and now is skipped rather than failing the
            // whole search.
            let _ = search_result;
        }
        Ok(matches)
    }

    /// Candidate files for `pattern`: narrowed via the ngram tantivy index
    /// when the pattern is long enough to produce ngram terms, otherwise
    /// (or on a query-parse failure, e.g. regex metacharacters the parser
    /// rejects) every indexed file — narrowing is a speed optimization, not
    /// a correctness requirement, since `search` re-verifies every
    /// candidate with a real matcher.
    // ponytail: no literal-prefix extraction for regex narrowing, so regex
    // searches always fall back to "every indexed file" as candidates —
    // upgrade to extracting a literal prefix/substring from the regex if
    // Find-in-Files needs to stay fast on very large repos.
    // ponytail: a case-insensitive search skips ngram narrowing entirely
    // and scans every indexed file — upgrade to a lowercased companion
    // ngram field if case-insensitive Find in Files gets slow on big repos.
    fn candidate_files(
        &self,
        pattern: &str,
        case_sensitive: bool,
    ) -> Result<Vec<PathBuf>, IndexError> {
        let searcher = self.reader.searcher();
        let num_docs = searcher.num_docs() as usize;
        if num_docs == 0 {
            return Ok(Vec::new());
        }

        // The `content` ngram tokenizer is case-sensitive (no lowercase
        // filter), so narrowing a case-insensitive query would drop files
        // that only differ in case. Narrowing is an optimisation, not a
        // correctness requirement — fall back to every text doc.
        let narrowed = if case_sensitive && pattern.chars().count() >= NGRAM_SIZE {
            let query_parser = QueryParser::for_index(&self.index, vec![self.fields.content]);
            query_parser.parse_query(pattern).ok()
        } else {
            None
        };

        let top_docs = match narrowed {
            // `content` is a text-doc-only field, so a content-field query
            // naturally never matches a symbol doc — no extra filter needed
            // on this branch.
            Some(query) => searcher.search(&query, &TopDocs::with_limit(num_docs))?,
            // Short pattern: no ngram terms to narrow on, fall back to
            // "every indexed file" — but the index now also holds symbol/
            // reference docs (E1), so `AllQuery` alone would return those
            // too (each sharing its file's `path`, producing duplicate/
            // spurious candidates). Restrict explicitly to text docs.
            None => {
                let text_only = TermQuery::new(
                    Term::from_field_text(self.fields.doc_type, DOC_TYPE_TEXT),
                    IndexRecordOption::Basic,
                );
                searcher.search(&text_only, &TopDocs::with_limit(num_docs))?
            }
        };

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

    /// Go-to-symbol: definition sites whose name contains `name_query`
    /// (case-sensitive substring — the minimum bar per ADR-0008; no fuzzy
    /// scoring). Narrowed to `is_definition=true` docs via tantivy first,
    /// then substring-filtered in Rust over that (already small) set.
    pub fn find_definitions(&self, name_query: &str) -> Result<Vec<SymbolMatch>, IndexError> {
        let searcher = self.reader.searcher();
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                doc_type_query(self.fields.doc_type, DOC_TYPE_SYMBOL),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(self.fields.sym_is_definition, 1),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let mut matches = self.collect_symbol_matches(&searcher, &query, true)?;
        matches.retain(|m| m.name.contains(name_query));
        matches.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        Ok(matches)
    }

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
            b.0.cmp(&a.0)
                .then_with(|| a.1.name.len().cmp(&b.1.name.len()))
                .then_with(|| (&a.1.path, a.1.line).cmp(&(&b.1.path, b.1.line)))
        });
        Ok(scored.into_iter().take(limit).map(|(_, m)| m).collect())
    }

    /// Find-usages: every occurrence (definitions and references alike) of
    /// the exact name `exact_name`, across every indexed file. Name-based
    /// per ADR-0008 — no cross-file type/binding resolution, so unrelated
    /// symbols that merely share a name are indistinguishable here by
    /// design.
    pub fn find_usages(&self, exact_name: &str) -> Result<Vec<SymbolMatch>, IndexError> {
        let searcher = self.reader.searcher();
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                doc_type_query(self.fields.doc_type, DOC_TYPE_SYMBOL),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.sym_name, exact_name),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let mut matches = self.collect_symbol_matches(&searcher, &query, false)?;
        matches.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        Ok(matches)
    }

    /// Definition sites whose name is *exactly* `name`.
    ///
    /// Unlike [`find_definitions`](Self::find_definitions), which fetches
    /// every definition doc in the index and substring-filters in Rust,
    /// this narrows to the name inside tantivy -- the difference matters
    /// on Go to Declaration, which runs per click on a project-sized
    /// index rather than per keystroke on a short prefix.
    pub fn find_definitions_exact(&self, name: &str) -> Result<Vec<SymbolMatch>, IndexError> {
        let searcher = self.reader.searcher();
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                doc_type_query(self.fields.doc_type, DOC_TYPE_SYMBOL),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.sym_name, name),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(self.fields.sym_is_definition, 1),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let mut matches = self.collect_symbol_matches(&searcher, &query, true)?;
        matches.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        Ok(matches)
    }

    /// Go to Declaration: where is the identifier at `byte_offset` in
    /// `current_content` declared?
    ///
    /// Two tiers, cheapest and most precise first:
    ///
    /// 1. **Local file.** [`resolve_declaration_in_buffer`]: definition-
    ///    position occurrences of the same name in `current_content`
    ///    itself, ranked nearest-preceding-first. Index-free, so a caller
    ///    without a built index can run that tier on its own.
    ///    This is what makes a local binding, a parameter, or a private
    ///    method resolve correctly: an inner `let x` sits nearer the caret
    ///    than an outer one, so shadowing falls out of the ordering
    ///    instead of needing a scope graph. Local candidates win outright
    ///    -- if the name is declared in this file, a same-named symbol in
    ///    another file is not what the caret meant.
    /// 2. **Project.** Otherwise, exact-name definitions from the index,
    ///    excluding `current_path` (tier 1 already covered it).
    ///
    /// Name-based per ADR-0008: two unrelated `run()` methods are
    /// indistinguishable, so tier 2 legitimately returns several
    /// candidates and the caller is expected to let the user pick. This is
    /// a documented boundary short of a language server, not an oversight.
    ///
    /// `current_content` is passed in rather than read from disk so an
    /// unsaved buffer resolves against what the user is actually looking
    /// at.
    pub fn resolve_declaration(
        &self,
        current_path: &Path,
        current_content: &str,
        byte_offset: usize,
    ) -> Result<Resolution, IndexError> {
        let local = resolve_declaration_in_buffer(current_path, current_content, byte_offset);
        // Tier 1 answered (or there was nothing under the caret to answer
        // about): the index has nothing to add.
        if local.name.is_empty() || local.tier == ResolutionTier::LocalFile {
            return Ok(local);
        }
        let name = local.name;

        let current_key = current_path.to_string_lossy().into_owned();
        let mut candidates = self.find_definitions_exact(&name)?;
        candidates.retain(|m| m.path.to_string_lossy() != current_key);
        let tier = if candidates.is_empty() {
            ResolutionTier::None
        } else {
            ResolutionTier::Project
        };
        Ok(Resolution {
            name,
            tier,
            candidates,
        })
    }

    /// Go to Implementation: every type that declares `supertype` as a
    /// base class, implemented interface, or (in Rust) an implemented
    /// trait. Each result's `name` is the *implementing* type and its
    /// `container` is `supertype`, so a result list reads
    /// "Circle in Shape".
    pub fn find_implementations(&self, supertype: &str) -> Result<Vec<SymbolMatch>, IndexError> {
        self.inherit_matches(self.fields.inh_supertype, supertype, self.fields.inh_type)
    }

    /// Go to Interface: every supertype `type_name` declares. Each
    /// result's `name` is the *supertype* and its `container` is
    /// `type_name`.
    ///
    /// The location reported is where the edge was *declared* (the
    /// subtype's own name token), not where the supertype is defined --
    /// resolving that is [`resolve_declaration`](Self::resolve_declaration)'s
    /// job, and chaining the two is the caller's choice.
    pub fn find_supertypes(&self, type_name: &str) -> Result<Vec<SymbolMatch>, IndexError> {
        self.inherit_matches(self.fields.inh_type, type_name, self.fields.inh_supertype)
    }

    /// Shared body of [`find_implementations`](Self::find_implementations)
    /// and [`find_supertypes`](Self::find_supertypes): match `doc_type =
    /// inherit` docs whose `match_field` is `value`, and report the
    /// opposite end of the edge (`name_field`) as the result's name.
    fn inherit_matches(
        &self,
        match_field: tantivy::schema::Field,
        value: &str,
        name_field: tantivy::schema::Field,
    ) -> Result<Vec<SymbolMatch>, IndexError> {
        let searcher = self.reader.searcher();
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                doc_type_query(self.fields.doc_type, DOC_TYPE_INHERIT),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(match_field, value),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let num_docs = searcher.num_docs() as usize;
        if num_docs == 0 {
            return Ok(Vec::new());
        }
        let top_docs = searcher.search(&query, &TopDocs::with_limit(num_docs))?;
        let mut matches = Vec::with_capacity(top_docs.len());
        for (_score, doc_address) in top_docs {
            let retrieved: tantivy::TantivyDocument = searcher.doc(doc_address)?;
            let get_str = |field| {
                retrieved
                    .get_first(field)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let get_u64 = |field| {
                retrieved
                    .get_first(field)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize
            };
            let (Some(name), Some(path)) = (get_str(name_field), get_str(self.fields.path)) else {
                continue;
            };
            matches.push(SymbolMatch {
                name,
                kind: None,
                path: PathBuf::from(path),
                line: get_u64(self.fields.sym_line),
                col: get_u64(self.fields.sym_col),
                is_definition: true,
                container: Some(value.to_string()),
            });
        }
        matches.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        Ok(matches)
    }

    /// Expand every packed symbol document `query` matches back into one
    /// [`SymbolMatch`] per occurrence.
    ///
    /// `definitions_only` is not an optimisation: a packed document matches
    /// the `sym_is_definition:1` term when *any* of its rows is a definition,
    /// so a definition query that did not filter the expanded rows would
    /// report every reference in the same file as a definition too.
    fn collect_symbol_matches(
        &self,
        searcher: &tantivy::Searcher,
        query: &dyn tantivy::query::Query,
        definitions_only: bool,
    ) -> Result<Vec<SymbolMatch>, IndexError> {
        let num_docs = searcher.num_docs() as usize;
        if num_docs == 0 {
            return Ok(Vec::new());
        }
        let top_docs = searcher.search(query, &TopDocs::with_limit(num_docs))?;
        let mut matches = Vec::with_capacity(top_docs.len());
        for (_score, doc_address) in top_docs {
            let retrieved: tantivy::TantivyDocument = searcher.doc(doc_address)?;
            let all_u64 = |field| {
                retrieved
                    .get_all(field)
                    .map(|v| v.as_u64().unwrap_or(0))
                    .collect::<Vec<_>>()
            };
            let all_str = |field| {
                retrieved
                    .get_all(field)
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
            };
            let first_str = |field| {
                retrieved
                    .get_first(field)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let Some(name) = first_str(self.fields.sym_name) else {
                continue;
            };
            let Some(path) = first_str(self.fields.path) else {
                continue;
            };
            let lines = all_u64(self.fields.sym_line);
            let cols = all_u64(self.fields.sym_col);
            let definitions = all_u64(self.fields.sym_is_definition);
            let kinds = all_str(self.fields.sym_kind);
            let containers = all_str(self.fields.sym_container);

            for (row, &line) in lines.iter().enumerate() {
                let is_definition = definitions.get(row).copied().unwrap_or(0) == 1;
                if definitions_only && !is_definition {
                    continue;
                }
                matches.push(SymbolMatch {
                    name: name.clone(),
                    kind: kinds.get(row).and_then(|k| symbol_kind_from_str(k)),
                    path: PathBuf::from(&path),
                    line: line as usize,
                    col: cols.get(row).copied().unwrap_or(0) as usize,
                    is_definition,
                    container: containers
                        .get(row)
                        .filter(|c| !c.is_empty())
                        .map(|c| c.to_string()),
                });
            }
        }
        Ok(matches)
    }
}

fn doc_type_query(field: tantivy::schema::Field, value: &str) -> Box<dyn tantivy::query::Query> {
    Box::new(TermQuery::new(
        Term::from_field_text(field, value),
        IndexRecordOption::Basic,
    ))
}

/// Every document one file contributes to the index, or `None` when the file
/// is not indexable at all (unreadable, or not UTF-8 text) — those are
/// dropped from the index *and* from the file-name tier, since neither text
/// nor file search can serve them.
///
/// A file past [`MAX_INDEXED_BYTES`] contributes no documents at all but is
/// still reported as indexable, so it stays in the file-name tier and stays
/// findable by name — it is simply not searchable by content or by symbol.
/// Deliberately no stamped, empty-content text document: that document would
/// carry the file's path back into the candidate list whenever the ngram
/// stage falls back to "every text doc", so Find in Files would reach into an
/// over-cap file on some patterns and not others. The cost of leaving it out
/// is that each open re-decides it, which is a size comparison against a
/// stat the walk already did.
///
/// Free-standing rather than a method so the indexing pass can run it on
/// many files in parallel — it borrows nothing mutable.
fn file_docs(
    fields: &Fields,
    path_key: &str,
    path: &Path,
    stamp: FileStamp,
) -> Option<Vec<tantivy::TantivyDocument>> {
    let text_doc = |content: String| {
        doc!(
            fields.path => path_key.to_string(),
            fields.doc_type => DOC_TYPE_TEXT,
            fields.content => content,
            fields.mtime_secs => stamp.mtime_secs,
            fields.size_bytes => stamp.size_bytes,
        )
    };

    if stamp.size_bytes > MAX_INDEXED_BYTES {
        return Some(Vec::new());
    }
    let bytes = fs::read(path).ok()?;
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let content = String::from_utf8(bytes).ok()?;

    let mut documents = symbol_docs(fields, path_key, &content);
    documents.push(text_doc(content));
    Some(documents)
}

/// The definition/reference rows for one file's already-read `content` as
/// symbol/reference-schema documents (see the module doc's "Three schemas,
/// one index" section). Merges `outline()`'s definitions (kind + container)
/// onto the matching `identifier_occurrences()` row by shared name-token
/// byte range rather than indexing both separately, so a definition site
/// appears exactly once in `find_usages` results, not twice.
fn symbol_docs(fields: &Fields, path_key: &str, content: &str) -> Vec<tantivy::TantivyDocument> {
    let language = language_for_path(Path::new(path_key));

    // One parse for all three extractions (outline, occurrences, supertype
    // edges) instead of three -- see `syntax_core::analyze_file`.
    let analysis = analyze_file(language, content);
    let mut flat: BTreeMap<(usize, usize), FlatSymbol<'_>> = BTreeMap::new();
    flatten_outline(&analysis.outline, None, &mut flat);

    // One table for the whole file: every occurrence and edge below resolves
    // its position by binary search instead of counting newlines from byte 0.
    let starts = line_starts(content);

    // Group by name first: one document per (file, name) rather than one per
    // occurrence. A file mentioning `self` two hundred times used to cost two
    // hundred documents, each repeating the file's whole path.
    let mut by_name: BTreeMap<String, Vec<SymbolRow<'_>>> = BTreeMap::new();
    for occurrence in &analysis.occurrences {
        let (line, col) = line_and_col_from(&starts, occurrence.start);
        let flat_symbol = flat.get(&(occurrence.start, occurrence.end));
        by_name
            .entry(occurrence.name.clone())
            .or_default()
            .push(SymbolRow {
                line,
                col,
                is_definition: occurrence.is_definition,
                kind: flat_symbol.map(|f| symbol_kind_to_str(f.kind)),
                container: flat_symbol.and_then(|f| f.container),
            });
    }

    let mut documents = Vec::with_capacity(by_name.len() + analysis.supertype_edges.len());
    for (name, rows) in by_name {
        let mut document = doc!(
            fields.path => path_key.to_string(),
            fields.doc_type => DOC_TYPE_SYMBOL,
            fields.sym_name => name,
        );
        // All five, every row, in row order — that is the whole contract
        // that keeps the parallel fields aligned when they are read back.
        for row in rows {
            document.add_u64(fields.sym_line, row.line as u64);
            document.add_u64(fields.sym_col, row.col as u64);
            document.add_u64(fields.sym_is_definition, row.is_definition as u64);
            document.add_text(fields.sym_kind, row.kind.unwrap_or(""));
            document.add_text(fields.sym_container, row.container.unwrap_or(""));
        }
        documents.push(document);
    }

    for edge in &analysis.supertype_edges {
        let (line, col) = line_and_col_from(&starts, edge.type_start);
        documents.push(doc!(
            fields.path => path_key.to_string(),
            fields.doc_type => DOC_TYPE_INHERIT,
            fields.inh_type => edge.type_name.clone(),
            fields.inh_supertype => edge.supertype_name.clone(),
            fields.sym_line => line as u64,
            fields.sym_col => col as u64,
        ));
    }
    documents
}

/// Tier 1 of [`TextIndex::resolve_declaration`] on its own: declarations of
/// the identifier at `byte_offset` found *inside `current_content` itself*,
/// nearest-preceding-first.
///
/// Split out of the index because it needs no index: a local binding, a
/// parameter or a same-file function resolves from the buffer alone. That is
/// what lets Go to Declaration answer a Ctrl+Click before the project index
/// has finished building, or with no project open at all (a lone file), where
/// the alternative is the gesture doing nothing at all.
///
/// The returned tier is [`ResolutionTier::LocalFile`] when this file declares
/// the name, and [`ResolutionTier::None`] otherwise -- with `name` still set,
/// so a caller with an index can go on to search the project for it, and a
/// caller without one can say *which* name it could not place. An empty
/// `name` means the caret was not on an identifier.
pub fn resolve_declaration_in_buffer(
    current_path: &Path,
    current_content: &str,
    byte_offset: usize,
) -> Resolution {
    let language = language_for_path(current_path);
    let no_identifier = || Resolution {
        name: String::new(),
        tier: ResolutionTier::None,
        candidates: Vec::new(),
    };
    // One parse, then the `locals` walk alone -- the caret has to be on an
    // identifier before the `tags` walk (which costs about as much as the
    // parse) is worth running at all.
    let Some(parsed) = syntax_core::ParsedFile::parse(language, current_content) else {
        return no_identifier();
    };
    let occurrences = parsed.occurrences();
    let Some(target) = occurrences
        .iter()
        .find(|o| byte_offset >= o.start && byte_offset < o.end)
    else {
        return no_identifier();
    };
    let name = target.name.clone();

    let outline = parsed.outline();
    let mut flat: BTreeMap<(usize, usize), FlatSymbol<'_>> = BTreeMap::new();
    flatten_outline(&outline, None, &mut flat);

    let mut local: Vec<(usize, SymbolMatch)> = occurrences
        .iter()
        .filter(|o| o.is_definition && o.name == name)
        .map(|o| {
            let (line, col) = line_and_col_at(current_content, o.start);
            let flat_symbol = flat.get(&(o.start, o.end));
            (
                o.start,
                SymbolMatch {
                    name: name.clone(),
                    kind: flat_symbol.map(|f| f.kind),
                    path: current_path.to_path_buf(),
                    line,
                    col,
                    is_definition: true,
                    container: flat_symbol.and_then(|f| f.container).map(str::to_string),
                },
            )
        })
        .collect();

    if local.is_empty() {
        return Resolution {
            name,
            tier: ResolutionTier::None,
            candidates: Vec::new(),
        };
    }

    // Nearest preceding declaration first (the innermost shadowing binding),
    // then the nearest one *after* the caret -- a method called before its own
    // declaration in the same class is ordinary in every language here.
    local.sort_by_key(|(start, _)| {
        if *start <= byte_offset {
            (0usize, byte_offset - *start)
        } else {
            (1usize, *start - byte_offset)
        }
    });
    Resolution {
        name,
        tier: ResolutionTier::LocalFile,
        candidates: local.into_iter().map(|(_, m)| m).collect(),
    }
}

/// Project-relative, forward-slashed display form of `path` — what the file
/// tier matches against and shows, so a query like `src/main` behaves the
/// same on Windows as on Linux.
fn relative_display(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
}

fn escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, content: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn a_project_whose_index_is_still_building_is_not_reported_as_no_project() {
        let building = IndexSlot::Building(PathBuf::from("/p"))
            .unavailable_reason()
            .unwrap();
        assert!(building.contains("still being built"), "{building}");
        assert_ne!(
            building,
            IndexSlot::NoProject.unavailable_reason().unwrap(),
            "an open project must never be reported as no project at all"
        );
    }

    #[test]
    fn a_failed_index_build_reports_its_own_error() {
        let reason = IndexSlot::Failed("disk on fire".to_string())
            .unavailable_reason()
            .unwrap();
        assert!(reason.contains("disk on fire"), "{reason}");
    }

    #[test]
    fn a_ready_index_has_no_reason_and_answers_queries() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "let needle = 2;\n");
        let slot = IndexSlot::Ready(Box::new(TextIndex::build(dir.path()).unwrap()));
        assert_eq!(slot.unavailable_reason(), None);
        assert!(slot.ready().is_some());
    }

    #[test]
    fn only_a_ready_slot_hands_out_the_index() {
        assert!(IndexSlot::default().ready().is_none());
        assert!(IndexSlot::Building(PathBuf::from("/p")).ready().is_none());
        assert!(IndexSlot::Building(PathBuf::from("/p"))
            .ready_mut()
            .is_none());
        assert!(IndexSlot::Failed("x".to_string()).ready().is_none());
    }

    #[test]
    fn search_match_carries_the_line_it_was_found_on() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "let x = 1;\nlet needle = 2;\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let matches = index.search("needle", false, true).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_text, "let needle = 2;");
    }

    #[test]
    fn search_with_stops_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "hit\nhit\nhit\nhit\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let matches = index
            .search_with("hit", false, true, 2, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn search_with_returns_nothing_once_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "hit\nhit\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let cancelled = AtomicBool::new(true);
        let matches = index
            .search_with("hit", false, true, usize::MAX, &cancelled)
            .unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn find_files_ranks_the_closer_filename_first() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/main.rs", "");
        write(dir.path(), "src/vendor/mainframe_adapter.rs", "");
        let index = TextIndex::build(dir.path()).unwrap();

        let hits = index.find_files("main.rs", 10);
        assert_eq!(hits[0].relative, "src/main.rs");
    }

    #[test]
    fn find_files_matches_across_path_segments_and_reports_positions() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/widgets/button.rs", "");
        let index = TextIndex::build(dir.path()).unwrap();

        let hits = index.find_files("widbut", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].relative, "src/widgets/button.rs");
        assert!(!hits[0].positions.is_empty());
        // Positions must address the string they highlight.
        let chars: Vec<char> = hits[0].relative.chars().collect();
        for position in &hits[0].positions {
            assert!((*position as usize) < chars.len());
        }
    }

    #[test]
    fn find_files_honours_the_limit_and_lists_files_for_an_empty_query() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(dir.path(), &format!("f{i}.txt"), "x");
        }
        let index = TextIndex::build(dir.path()).unwrap();

        assert_eq!(index.find_files("", 3).len(), 3);
        assert_eq!(index.find_files("txt", 2).len(), 2);
        assert_eq!(index.indexed_file_count(), 5);
    }

    #[test]
    fn the_file_tier_follows_reindex_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let kept = write(dir.path(), "kept.txt", "a");
        let index_path = dir.path().join("added.txt");
        let mut index = TextIndex::build(dir.path()).unwrap();
        assert_eq!(index.indexed_file_count(), 1);

        fs::write(&index_path, "b").unwrap();
        index.reindex_file(&index_path).unwrap();
        assert_eq!(index.indexed_file_count(), 2);
        assert_eq!(index.find_files("added", 5).len(), 1);

        index.remove_file(&kept).unwrap();
        assert_eq!(index.indexed_file_count(), 1);
        assert!(index.find_files("kept", 5).is_empty());
    }

    #[test]
    fn an_index_from_an_older_extraction_is_rebuilt_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let content = "final class ConfigurationException extends BcCheckException {}\n";
        write(dir.path(), "src/A.java", content);
        drop(TextIndex::build(dir.path()).unwrap());
        // An index written before this extraction version existed has no
        // version file at all — same as one whose number no longer matches.
        fs::remove_file(index_dir_for(dir.path()).join(EXTRACTION_VERSION_FILE)).unwrap();

        let index = TextIndex::open_or_build(dir.path()).unwrap();
        let usages = index.find_usages("BcCheckException").unwrap();
        assert_eq!(usages.len(), 1, "rebuild must re-extract: {usages:?}");
        assert_eq!(
            fs::read_to_string(index_dir_for(dir.path()).join(EXTRACTION_VERSION_FILE))
                .unwrap()
                .trim(),
            EXTRACTION_VERSION.to_string()
        );
    }

    #[test]
    fn open_or_build_reuses_the_index_and_applies_the_delta() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "stable.txt", "unchanged marker\n");
        let changed = write(dir.path(), "changed.txt", "old\n");
        let doomed = write(dir.path(), "doomed.txt", "doomed marker\n");
        drop(TextIndex::build(dir.path()).unwrap());

        // Different byte length, so the (mtime, size) stamp differs even
        // within the same second.
        fs::write(&changed, "brand new content\n").unwrap();
        fs::remove_file(&doomed).unwrap();
        write(dir.path(), "fresh.txt", "fresh marker\n");

        let index = TextIndex::open_or_build(dir.path()).unwrap();
        assert_eq!(
            index.search("unchanged marker", false, true).unwrap().len(),
            1
        );
        assert_eq!(index.search("brand new", false, true).unwrap().len(), 1);
        assert!(index.search("old", false, true).unwrap().is_empty());
        assert!(index
            .search("doomed marker", false, true)
            .unwrap()
            .is_empty());
        assert_eq!(index.search("fresh marker", false, true).unwrap().len(), 1);
        assert_eq!(index.indexed_file_count(), 3);
    }

    #[test]
    fn open_or_build_reports_a_busy_lock_and_leaves_the_live_index_intact() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "needle\n");
        // Stands in for the other IDE instance: one live writer on the
        // directory, held for the whole test.
        let live = TextIndex::open_or_build(dir.path()).unwrap();

        let Err(err) = TextIndex::open_or_build(dir.path()) else {
            panic!("a second writer on one index directory must not succeed");
        };
        assert!(
            matches!(err, IndexError::Locked(_)),
            "a held writer lock must be reported as such, got {err:?}"
        );
        assert_eq!(live.search("needle", false, true).unwrap().len(), 1);
    }

    #[test]
    fn open_or_build_builds_from_scratch_when_no_index_exists() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "hello\n");

        let index = TextIndex::open_or_build(dir.path()).unwrap();
        assert_eq!(index.search("hello", false, true).unwrap().len(), 1);
    }

    #[test]
    fn find_definitions_ranked_puts_the_best_fuzzy_hit_first() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "a.rs",
            "fn open_file() {}\nfn open_project_file_dialog() {}\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let hits = index.find_definitions_ranked("openfile", 10).unwrap();
        assert_eq!(hits[0].name, "open_file");
        assert_eq!(index.find_definitions_ranked("", 1).unwrap().len(), 1);
    }

    #[test]
    fn resolve_replacements_expands_captures_against_the_matched_slice() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        fs::write(&file, "let alpha = 1;\nlet beta = 2;\n").unwrap();

        let resolved = resolve_replacements(
            &[(file.clone(), 1, 4, 9)],
            "al(pha)",
            "om$1",
            editor_core::SearchOptions {
                regex: true,
                case_sensitive: true,
            },
        )
        .unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].text, "ompha");
        assert_eq!(
            (resolved[0].line, resolved[0].start, resolved[0].end),
            (1, 4, 9)
        );
    }

    #[test]
    fn resolve_replacements_drops_a_span_the_file_no_longer_has() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        fs::write(&file, "short\n").unwrap();

        // Span points past the end of the (since-shortened) line.
        let resolved = resolve_replacements(
            &[(file, 1, 40, 45)],
            "alpha",
            "omega",
            editor_core::SearchOptions {
                regex: false,
                case_sensitive: true,
            },
        )
        .unwrap();

        assert!(resolved.is_empty());
    }

    #[test]
    fn substring_search_finds_file_line_and_span() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "src/main.rs",
            "fn main() {\n    println!(\"hello world\");\n}\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let matches = index.search("hello world", false, true).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, file);
        assert_eq!(matches[0].line, 2);
        let line = "    println!(\"hello world\");";
        assert_eq!(&line[matches[0].start..matches[0].end], "hello world");
    }

    #[test]
    fn case_insensitive_search_finds_a_differently_cased_hit() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "Widget factory\n");
        let index = TextIndex::build(dir.path()).unwrap();

        assert!(index.search("widget", false, true).unwrap().is_empty());
        assert_eq!(index.search("widget", false, false).unwrap().len(), 1);
    }

    #[test]
    fn every_occurrence_on_a_line_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "one two one two one\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let matches = index.search("one", false, true).unwrap();
        assert_eq!(matches.len(), 3);
        assert_eq!(
            matches.iter().map(|m| m.start).collect::<Vec<_>>(),
            vec![0, 8, 16]
        );
    }

    // `replace_in_files`/`preview_replacements` and their tests live in
    // `replace_preview.rs` now (F3-15) — this file is grandfathered at a
    // ratcheted line-count ceiling that may only shrink.

    #[test]
    fn regex_search_finds_matching_lines() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "foo123\nbar\nfoo456\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let mut matches = index.search(r"foo\d+", true, true).unwrap();
        matches.sort_by_key(|m| m.line);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].line, 1);
        assert_eq!(matches[1].line, 3);
    }

    #[test]
    fn gitignored_files_are_excluded_from_the_index() {
        let dir = tempfile::tempdir().unwrap();
        // `ignore::WalkBuilder` only honors `.gitignore` inside an actual
        // git work tree by default (matching git's/ripgrep's own
        // behavior) — a bare `.gitignore` file with no `.git` directory
        // next to it is not enough.
        std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir.path())
            .status()
            .unwrap();
        write(dir.path(), ".gitignore", "ignored.txt\n");
        write(dir.path(), "ignored.txt", "needle here");
        write(dir.path(), "kept.txt", "needle here too");
        let index = TextIndex::build(dir.path()).unwrap();

        let matches = index.search("needle", false, true).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].path.ends_with("kept.txt"));
    }

    #[test]
    fn reindex_file_picks_up_modified_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "a.txt", "original content");
        let mut index = TextIndex::build(dir.path()).unwrap();
        assert_eq!(index.search("updated", false, true).unwrap().len(), 0);

        fs::write(&file, "updated content").unwrap();
        index.reindex_file(&file).unwrap();

        let matches = index.search("updated", false, true).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, file);
    }

    #[test]
    fn remove_file_drops_it_from_subsequent_searches() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "a.txt", "findme");
        let mut index = TextIndex::build(dir.path()).unwrap();
        assert_eq!(index.search("findme", false, true).unwrap().len(), 1);

        index.remove_file(&file).unwrap();

        assert_eq!(index.search("findme", false, true).unwrap().len(), 0);
    }

    // --- stamp sidecar and batched watcher updates (IP4) ---

    #[test]
    fn the_stamp_sidecar_carries_the_same_answer_as_reading_the_index() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn add() {}\n");
        write(dir.path(), "src/other.rs", "fn other() {}\n");
        let index = TextIndex::build(dir.path()).unwrap();
        let from_index = index.indexed_stamps().unwrap();
        let index_dir = index.index_dir.clone();
        drop(index);

        let from_sidecar = read_stamps(&index_dir).expect("sidecar written by the build");
        assert_eq!(from_sidecar, from_index);
    }

    #[test]
    fn a_missing_sidecar_still_opens_correctly_from_the_index_itself() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn add() {}\n");
        let index = TextIndex::build(dir.path()).unwrap();
        let index_dir = index.index_dir.clone();
        drop(index);

        fs::write(index_dir.join(STAMPS_FILE), "not a stamp file at all").unwrap();
        assert!(read_stamps(&index_dir).is_none());

        // The unparseable sidecar sends the open down the tantivy fallback,
        // and the result is the same index either way.
        let index = TextIndex::open_or_build(dir.path()).unwrap();
        assert_eq!(index.indexed_file_count(), 1);
        assert_eq!(index.find_definitions_exact("add").unwrap().len(), 1);
    }

    #[test]
    fn sync_paths_applies_a_whole_watcher_batch_in_one_pass() {
        let dir = tempfile::tempdir().unwrap();
        let kept = write(dir.path(), "src/kept.rs", "fn kept() {}\n");
        let doomed = write(dir.path(), "src/doomed.rs", "fn doomed() {}\n");
        let mut index = TextIndex::build(dir.path()).unwrap();

        fs::write(&kept, "fn renamed() {}\n").unwrap();
        fs::remove_file(&doomed).unwrap();
        let added = write(dir.path(), "src/added.rs", "fn added() {}\n");

        index
            .sync_paths(&[kept.clone(), doomed.clone(), added.clone()])
            .unwrap();

        assert_eq!(index.find_definitions_exact("kept").unwrap().len(), 0);
        assert_eq!(index.find_definitions_exact("renamed").unwrap().len(), 1);
        assert_eq!(index.find_definitions_exact("doomed").unwrap().len(), 0);
        assert_eq!(index.find_definitions_exact("added").unwrap().len(), 1);
        assert_eq!(index.indexed_file_count(), 2);
    }

    #[test]
    fn reopening_an_unchanged_project_writes_nothing_at_all() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn add() {}\n");
        write(dir.path(), "src/other.rs", "fn other() {}\n");
        let index = TextIndex::build(dir.path()).unwrap();
        let index_dir = index.index_dir.clone();
        drop(index);

        let listing = |dir: &Path| {
            let mut entries: Vec<(String, u64)> = fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        e.metadata().map(|m| m.len()).unwrap_or(0),
                    )
                })
                .collect();
            entries.sort();
            entries
        };
        let before = listing(&index_dir);

        let index = TextIndex::open_or_build(dir.path()).unwrap();
        assert_eq!(index.indexed_file_count(), 2);
        drop(index);

        // No commit, no new segment, no rewritten sidecar: a project that did
        // not change costs a walk and a stat per file, and nothing else.
        assert_eq!(listing(&index_dir), before);
    }

    // --- packed symbol documents, size cap, binary sniff (IP1/IP3) ---

    #[test]
    fn line_and_col_from_a_table_matches_the_scanning_version() {
        let text = "alpha\nbeta\n\ngamma delta\n";
        let starts = line_starts(text);
        for offset in 0..=text.len() {
            assert_eq!(
                line_and_col_from(&starts, offset),
                line_and_col_at(text, offset),
                "offset {offset}"
            );
        }
    }

    #[test]
    fn one_document_carries_every_occurrence_of_a_name_with_its_rows_aligned() {
        let dir = tempfile::tempdir().unwrap();
        // `add` is defined on line 1 and referenced on lines 2 and 3, so all
        // three occurrences pack into a single document.
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32) -> i32 { x }\nfn a() -> i32 { add(1) }\nfn b() -> i32 { add(2) }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let searcher = index.reader.searcher();
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                doc_type_query(index.fields.doc_type, DOC_TYPE_SYMBOL),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(index.fields.sym_name, "add"),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let docs = searcher.search(&query, &TopDocs::with_limit(10)).unwrap();
        assert_eq!(docs.len(), 1, "one document per (file, name)");

        let usages = index.find_usages("add").unwrap();
        assert_eq!(
            usages.iter().map(|u| u.line).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // The rows stayed aligned: only the row on line 1 is the definition,
        // and only it carries the kind the outline supplied.
        assert_eq!(
            usages.iter().map(|u| u.is_definition).collect::<Vec<_>>(),
            vec![true, false, false]
        );
        assert_eq!(usages[0].kind, Some(SymbolKind::Function));
        assert_eq!(usages[1].kind, None);
        assert_eq!(usages[2].kind, None);
    }

    #[test]
    fn a_definition_query_reports_only_the_definition_rows_of_a_packed_document() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32) -> i32 { x }\nfn a() -> i32 { add(1) }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let defs = index.find_definitions_exact("add").unwrap();
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].line, 1);
        assert!(defs[0].is_definition);

        let substring = index.find_definitions("ad").unwrap();
        assert_eq!(substring.len(), 1, "{substring:?}");
        assert_eq!(substring[0].line, 1);
    }

    #[test]
    fn a_file_past_the_size_cap_is_findable_by_name_but_not_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let big = format!(
            "fn zqmarker_huge() {{}}\n// {}\n",
            "x".repeat(MAX_INDEXED_BYTES as usize)
        );
        write(dir.path(), "src/big.rs", &big);
        write(dir.path(), "src/small.rs", "fn small() {}\n");
        let index = TextIndex::build(dir.path()).unwrap();

        // Still in the file-name tier, and still stamped, so a warm open
        // does not keep rediscovering it.
        assert!(index
            .find_files("big.rs", 10)
            .iter()
            .any(|m| m.path.ends_with("big.rs")));

        // The documented ceiling: no content in the ngram index, so the
        // candidate stage never nominates it, and no symbols at all.
        let hits = index.search("zqmarker_huge", false, true).unwrap();
        assert!(hits.is_empty(), "{hits:?}");
        let defs = index.find_definitions_exact("zqmarker_huge").unwrap();
        assert!(defs.is_empty(), "{defs:?}");
    }

    #[test]
    fn a_binary_file_is_dropped_from_the_index_entirely() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn add() {}\n");
        fs::write(dir.path().join("blob.bin"), b"MZ\x00\x00binary payload").unwrap();
        let index = TextIndex::build(dir.path()).unwrap();

        assert!(
            index.find_files("blob", 10).is_empty(),
            "a binary file is not worth listing by name either"
        );
        assert_eq!(index.indexed_file_count(), 1);
    }

    #[test]
    fn progress_counts_up_to_the_total_it_announced() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(dir.path(), &format!("src/f{i}.rs"), "fn f() {}\n");
        }
        let seen = std::sync::Mutex::new(Vec::new());
        let index = TextIndex::build_with_progress(
            dir.path(),
            &IndexOptions::default(),
            &|p: IndexProgress| {
                seen.lock().unwrap().push((p.done, p.total));
            },
        )
        .unwrap();
        drop(index);

        let seen = seen.into_inner().unwrap();
        let total = seen[0].1;
        assert_eq!(total, 5);
        assert_eq!(
            seen[0].0, 0,
            "the total is announced before any file is read"
        );
        let mut done: Vec<usize> = seen.iter().map(|(d, _)| *d).collect();
        done.sort();
        assert_eq!(done, vec![0, 1, 2, 3, 4, 5]);
        assert!(seen.iter().all(|(d, t)| *d <= *t && *t == total));
    }

    // --- symbol/reference schema (E1) ---

    const JAVA_FIXTURE: &str =
        "public class Greeter {\n    public String greet() {\n        return \"hi\";\n    }\n}\n";

    #[test]
    fn find_definitions_locates_a_rust_function_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let defs = index.find_definitions("add").unwrap();
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].name, "add");
        assert_eq!(defs[0].kind, Some(SymbolKind::Function));
        assert_eq!(defs[0].path, file);
        assert!(defs[0].is_definition);
        assert_eq!(defs[0].line, 1);
    }

    #[test]
    fn find_definitions_round_trips_the_four_new_symbol_kinds() {
        // Task 4a: guards symbol_kind_to_str/from_str's round trip.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "const MAX: i32 = 10;\n");
        write(
            dir.path(),
            "src/Person.cs",
            "class Person {\n    public string Name { get; set; }\n    public Person() {}\n}\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        assert_eq!(
            index.find_definitions("MAX").unwrap()[0].kind,
            Some(SymbolKind::Constant)
        );
        assert_eq!(
            index.find_definitions("Name").unwrap()[0].kind,
            Some(SymbolKind::Property)
        );
        let ctor = index.find_definitions("Person").unwrap();
        let ctor = ctor
            .iter()
            .find(|d| d.kind == Some(SymbolKind::Constructor));
        assert!(ctor.is_some(), "expected a Person constructor definition");
    }

    #[test]
    fn find_definitions_locates_a_java_class_by_name_proving_language_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        write(dir.path(), "src/Greeter.java", JAVA_FIXTURE);
        let index = TextIndex::build(dir.path()).unwrap();

        let defs = index.find_definitions("Greeter").unwrap();
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].kind, Some(SymbolKind::Class));
        assert!(defs[0].path.ends_with("Greeter.java"));

        // Java method nested under the class carries `container`.
        let methods = index.find_definitions("greet").unwrap();
        assert_eq!(methods.len(), 1, "{methods:?}");
        assert_eq!(methods[0].kind, Some(SymbolKind::Method));
        assert_eq!(methods[0].container.as_deref(), Some("Greeter"));
    }

    #[test]
    fn find_definitions_does_substring_matching() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        assert_eq!(index.find_definitions("ad").unwrap().len(), 1);
        assert_eq!(index.find_definitions("nonexistent").unwrap().len(), 0);
    }

    #[test]
    fn find_usages_finds_every_occurrence_of_an_exact_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32) -> i32 { x + x }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let usages = index.find_usages("x").unwrap();
        assert_eq!(usages.len(), 3, "1 definition + 2 references: {usages:?}");
        assert_eq!(usages.iter().filter(|m| m.is_definition).count(), 1);
        assert_eq!(usages.iter().filter(|m| !m.is_definition).count(), 2);
        assert!(usages.iter().all(|m| m.path == file));
    }

    #[test]
    fn find_usages_is_exact_not_substring() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        assert_eq!(index.find_usages("ad").unwrap().len(), 0);
        assert_eq!(index.find_usages("add").unwrap().len(), 1);
    }

    #[test]
    fn find_usages_spans_multiple_files_and_languages() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn greet() -> i32 { 1 }\n");
        write(dir.path(), "src/Greeter.java", JAVA_FIXTURE);
        let index = TextIndex::build(dir.path()).unwrap();

        // Name-based, not type-resolved (ADR-0008): the unrelated Rust
        // `greet` function and the Java `greet` method both match — that's
        // expected, not a bug.
        let usages = index.find_usages("greet").unwrap();
        assert_eq!(usages.len(), 2, "{usages:?}");
        assert!(usages.iter().any(|m| m.path.ends_with("lib.rs")));
        assert!(usages.iter().any(|m| m.path.ends_with("Greeter.java")));
    }

    #[test]
    fn find_definitions_empty_query_lists_every_definition() {
        // Class View's project-wide tier (Task I) relies on this: an empty
        // substring query (`str::contains("")` is always true) lists every
        // indexed definition project-wide, with no separate "list all"
        // method needed.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        write(dir.path(), "src/Greeter.java", JAVA_FIXTURE);
        let index = TextIndex::build(dir.path()).unwrap();

        let defs = index.find_definitions("").unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"add"), "{names:?}");
        assert!(names.contains(&"Greeter"), "{names:?}");
        assert!(names.contains(&"greet"), "{names:?}");
        assert!(defs.iter().all(|d| d.is_definition));
    }

    #[test]
    fn reindex_file_updates_symbol_data_on_rename() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        let mut index = TextIndex::build(dir.path()).unwrap();
        assert_eq!(index.find_definitions("add").unwrap().len(), 1);

        fs::write(&file, "fn sum(x: i32, y: i32) -> i32 { x + y }\n").unwrap();
        index.reindex_file(&file).unwrap();

        assert_eq!(
            index.find_definitions("add").unwrap().len(),
            0,
            "old name must be gone"
        );
        let defs = index.find_definitions("sum").unwrap();
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].kind, Some(SymbolKind::Function));
    }

    #[test]
    fn remove_file_drops_its_symbol_data_too() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32, y: i32) -> i32 { x + y }\n",
        );
        let mut index = TextIndex::build(dir.path()).unwrap();
        assert_eq!(index.find_definitions("add").unwrap().len(), 1);

        index.remove_file(&file).unwrap();

        assert_eq!(index.find_definitions("add").unwrap().len(), 0);
        assert_eq!(index.find_usages("add").unwrap().len(), 0);
    }

    // --- code navigation: columns, exact lookup, resolution, hierarchy ---

    #[test]
    fn a_definitions_column_points_at_the_name_token() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn add(x: i32) -> i32 { x }\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let defs = index.find_definitions_exact("add").unwrap();
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].line, 1);
        // "fn " is three bytes, so the name token starts at column 3.
        assert_eq!(defs[0].col, 3);
    }

    #[test]
    fn find_definitions_exact_is_not_substring() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/lib.rs",
            "fn add(x: i32) -> i32 { x }\nfn add_all(v: i32) -> i32 { v }\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        assert_eq!(index.find_definitions("add").unwrap().len(), 2);
        let exact = index.find_definitions_exact("add").unwrap();
        assert_eq!(exact.len(), 1, "{exact:?}");
        assert_eq!(exact[0].name, "add");
    }

    #[test]
    fn resolve_declaration_prefers_the_nearest_preceding_local_binding() {
        let dir = tempfile::tempdir().unwrap();
        // Two `value` bindings; the caret sits on the use after the second.
        let content =
            "fn outer() {\n    let value = 1;\n    {\n        let value = 2;\n        use_it(value);\n    }\n}\n";
        let file = write(dir.path(), "src/lib.rs", content);
        let index = TextIndex::build(dir.path()).unwrap();

        let caret = content.rfind("value)").unwrap();
        let resolution = index.resolve_declaration(&file, content, caret).unwrap();

        assert_eq!(resolution.name, "value");
        assert_eq!(resolution.tier, ResolutionTier::LocalFile);
        // The inner shadowing binding is nearest, so it comes first.
        assert_eq!(resolution.candidates[0].line, 4);
        assert_eq!(resolution.candidates[0].path, file);
    }

    #[test]
    fn a_java_type_in_an_extends_clause_resolves_and_plans_a_rename() {
        // The user's actual gesture: caret on `BcCheckException` in the
        // extends clause of another file, then Rename. The name is a
        // `type_identifier` in tree-sitter-java, which the occurrence query
        // once missed entirely — the caret "was not on a symbol".
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/BcCheckException.java",
            "public class BcCheckException extends RuntimeException {}\n",
        );
        let content = "final class ConfigurationException extends BcCheckException {}\n";
        let file = write(dir.path(), "src/ConfigurationException.java", content);
        let index = TextIndex::build(dir.path()).unwrap();

        let caret = content.find("BcCheckException").unwrap();
        let resolution = index.resolve_declaration(&file, content, caret).unwrap();
        assert_eq!(resolution.tier, ResolutionTier::Project, "{resolution:?}");
        assert_eq!(
            resolution.candidates[0].path,
            dir.path().join("src/BcCheckException.java")
        );

        let usages = index.find_usages("BcCheckException").unwrap();
        let definitions = index.find_definitions_exact("BcCheckException").unwrap();
        let plan = plan_index_rename(
            &resolution,
            &usages,
            &definitions,
            "BcConfigException",
            false,
        )
        .unwrap();
        let paths: Vec<&Path> = plan.sites.iter().map(|s| s.path.as_path()).collect();
        assert!(
            paths.contains(&dir.path().join("src/BcCheckException.java").as_path())
                && paths.contains(&file.as_path()),
            "both the declaration and the extends clause must be sites: {plan:?}"
        );
        assert!(!plan.ambiguous);
        assert!(plan.sites.iter().all(|s| s.checked));
    }

    #[test]
    fn resolve_declaration_falls_back_to_the_project_index() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/math.rs", "fn add(x: i32) -> i32 { x }\n");
        let content = "fn main() {\n    add(1);\n}\n";
        let file = write(dir.path(), "src/main.rs", content);
        let index = TextIndex::build(dir.path()).unwrap();

        let caret = content.find("add(1)").unwrap();
        let resolution = index.resolve_declaration(&file, content, caret).unwrap();

        assert_eq!(resolution.tier, ResolutionTier::Project);
        assert_eq!(resolution.candidates.len(), 1, "{resolution:?}");
        assert_eq!(
            resolution.candidates[0].path,
            dir.path().join("src/math.rs")
        );
        assert_eq!(resolution.candidates[0].kind, Some(SymbolKind::Function));
        assert_eq!(resolution.candidates[0].line, 1);
        assert_eq!(resolution.candidates[0].col, 3, "{resolution:?}");
    }

    #[test]
    fn resolve_declaration_returns_every_ambiguous_project_candidate() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "fn run(x: i32) -> i32 { x }\n");
        write(dir.path(), "src/b.rs", "fn run(y: i32) -> i32 { y }\n");
        let content = "fn main() {\n    run(1);\n}\n";
        let file = write(dir.path(), "src/main.rs", content);
        let index = TextIndex::build(dir.path()).unwrap();

        let caret = content.find("run(1)").unwrap();
        let resolution = index.resolve_declaration(&file, content, caret).unwrap();

        assert_eq!(resolution.tier, ResolutionTier::Project);
        assert_eq!(resolution.candidates.len(), 2, "{resolution:?}");
    }

    #[test]
    fn resolve_declaration_on_a_caret_that_is_not_an_identifier_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let content = "fn add(x: i32) -> i32 { x }\n";
        let file = write(dir.path(), "src/lib.rs", content);
        let index = TextIndex::build(dir.path()).unwrap();

        // The caret sits on the `{`.
        let caret = content.find('{').unwrap();
        let resolution = index.resolve_declaration(&file, content, caret).unwrap();

        assert_eq!(resolution.tier, ResolutionTier::None);
        assert!(resolution.name.is_empty());
        assert!(resolution.candidates.is_empty());
    }

    #[test]
    fn resolve_declaration_reports_nothing_for_an_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        let content = "fn main() {\n    nowhere(1);\n}\n";
        let file = write(dir.path(), "src/main.rs", content);
        let index = TextIndex::build(dir.path()).unwrap();

        let caret = content.find("nowhere").unwrap();
        let resolution = index.resolve_declaration(&file, content, caret).unwrap();

        assert_eq!(resolution.name, "nowhere");
        assert_eq!(resolution.tier, ResolutionTier::None);
        assert!(resolution.candidates.is_empty());
    }

    #[test]
    fn resolve_declaration_uses_the_passed_buffer_not_the_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "src/lib.rs", "fn stale() {}\n");
        let index = TextIndex::build(dir.path()).unwrap();

        // An unsaved buffer: the declaration exists only in memory.
        let buffer = "fn fresh() {}\nfn caller() {\n    fresh();\n}\n";
        let caret = buffer.rfind("fresh").unwrap();
        let resolution = index.resolve_declaration(&file, buffer, caret).unwrap();

        assert_eq!(resolution.tier, ResolutionTier::LocalFile);
        assert_eq!(resolution.candidates[0].line, 1);
    }

    #[test]
    fn resolve_declaration_in_buffer_answers_a_same_file_declaration_without_an_index() {
        let content = "fn helper() -> u32 {\n    1\n}\n\nfn main() {\n    helper();\n}\n";
        let caret = content.rfind("helper").unwrap();

        let resolution =
            resolve_declaration_in_buffer(Path::new("/nowhere/main.rs"), content, caret);

        assert_eq!(resolution.name, "helper");
        assert_eq!(resolution.tier, ResolutionTier::LocalFile);
        assert_eq!(resolution.candidates.len(), 1, "{resolution:?}");
        assert_eq!(resolution.candidates[0].line, 1);
        assert_eq!(resolution.candidates[0].col, 3);
    }

    #[test]
    fn resolve_declaration_in_buffer_names_a_symbol_it_cannot_place() {
        let content = "fn main() {\n    elsewhere();\n}\n";
        let caret = content.find("elsewhere").unwrap();

        let resolution =
            resolve_declaration_in_buffer(Path::new("/nowhere/main.rs"), content, caret);

        // Named but unplaced: an index-backed caller searches the project
        // for it, an index-less one can say which name it could not find.
        assert_eq!(resolution.name, "elsewhere");
        assert_eq!(resolution.tier, ResolutionTier::None);
        assert!(resolution.candidates.is_empty());
    }

    #[test]
    fn resolve_declaration_in_buffer_finds_nothing_off_an_identifier() {
        let content = "fn main() {\n    1 + 1;\n}\n";
        let caret = content.find('+').unwrap();

        let resolution =
            resolve_declaration_in_buffer(Path::new("/nowhere/main.rs"), content, caret);

        assert!(resolution.name.is_empty());
        assert_eq!(resolution.tier, ResolutionTier::None);
        assert!(resolution.candidates.is_empty());
    }

    #[test]
    fn find_implementations_lists_every_type_declaring_a_supertype() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/circle.rs",
            "struct Circle;\nimpl Shape for Circle {}\n",
        );
        write(
            dir.path(),
            "src/square.rs",
            "struct Square;\nimpl Shape for Square {}\n",
        );
        write(dir.path(), "src/other.rs", "struct Loose;\nimpl Loose {}\n");
        let index = TextIndex::build(dir.path()).unwrap();

        let implementations = index.find_implementations("Shape").unwrap();
        let mut names: Vec<&str> = implementations.iter().map(|m| m.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["Circle", "Square"]);
        assert!(implementations
            .iter()
            .all(|m| m.container.as_deref() == Some("Shape")));
        assert!(implementations.iter().all(|m| m.line == 2));
    }

    #[test]
    fn find_supertypes_is_the_inverse_direction() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/Circle.java",
            "class Circle extends Shape implements Drawable {}\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        let mut names: Vec<String> = index
            .find_supertypes("Circle")
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["Drawable", "Shape"]);
    }

    #[test]
    fn supertype_edges_are_dropped_with_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "src/circle.rs",
            "struct Circle;\nimpl Shape for Circle {}\n",
        );
        let mut index = TextIndex::build(dir.path()).unwrap();
        assert_eq!(index.find_implementations("Shape").unwrap().len(), 1);

        index.remove_file(&file).unwrap();
        assert!(index.find_implementations("Shape").unwrap().is_empty());
    }

    #[test]
    fn find_usages_ignores_supertype_edge_documents() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/circle.rs",
            "struct Circle;\nimpl Shape for Circle {}\n",
        );
        let index = TextIndex::build(dir.path()).unwrap();

        // `Circle` appears in a symbol doc twice (the struct definition and
        // the impl target) — the extra `inherit` doc must not show up as a
        // third usage.
        let usages = index.find_usages("Circle").unwrap();
        assert_eq!(usages.len(), 2, "{usages:?}");
    }

    #[test]
    fn reindexing_the_index_directory_itself_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "fn add(x: i32) -> i32 { x }\n");
        let mut index = TextIndex::build(dir.path()).unwrap();

        // A watcher driving reindex_file sees the index's own commits as
        // project changes; if those re-entered the writer, the callback
        // would feed itself forever.
        let own = dir.path().join(INDEX_DIR_NAME).join("meta.json");
        assert!(own.exists(), "the index writes into its own directory");
        index.reindex_file(&own).unwrap();
        index.remove_file(&own).unwrap();

        // And the real file's rows are untouched by that no-op.
        assert_eq!(index.find_definitions_exact("add").unwrap().len(), 1);
    }

    #[test]
    fn index_internal_paths_are_never_reindexed() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "hello\n");
        let mut index = TextIndex::build(dir.path()).unwrap();
        let before = index.indexed_file_count();

        // What the project watcher reports when the index commits its own
        // segment files.
        let internal = dir.path().join(".ide-index").join("meta.json");
        assert!(index.is_index_internal(&internal));
        index.reindex_file(&internal).unwrap();
        index.remove_file(&internal).unwrap();

        assert_eq!(index.indexed_file_count(), before);
        assert!(index.find_files("meta", 5).is_empty());
    }
    fn symbol(path: &Path, line: usize, col: usize, is_definition: bool) -> SymbolMatch {
        SymbolMatch {
            name: "target".into(),
            kind: None,
            path: path.to_path_buf(),
            line,
            col,
            is_definition,
            container: None,
        }
    }

    fn resolution(path: &Path, tier: ResolutionTier) -> Resolution {
        Resolution {
            name: "target".into(),
            tier,
            candidates: vec![symbol(path, 1, 3, true)],
        }
    }

    #[test]
    fn an_unresolved_caret_is_refused_rather_than_renamed_by_name() {
        let nothing = Resolution {
            name: String::new(),
            tier: ResolutionTier::None,
            candidates: Vec::new(),
        };
        assert_eq!(
            plan_index_rename(&nothing, &[], &[], "fresh", false),
            Err(RenameRefusal::Unresolved),
            "renaming every token spelled the same is Replace in Files, not rename",
        );
    }

    #[test]
    fn an_invalid_new_name_is_refused_before_anything_is_planned() {
        let home = PathBuf::from("/p/a.rs");
        let resolved = resolution(&home, ResolutionTier::LocalFile);
        for bad in ["", " ", "two words", "9lives", "has-dash"] {
            assert_eq!(
                plan_index_rename(&resolved, &[symbol(&home, 1, 3, true)], &[], bad, false),
                Err(RenameRefusal::InvalidName),
                "{bad:?} is not an identifier",
            );
        }
        assert!(is_valid_identifier("_private2"));
        assert!(is_valid_identifier("Ünicode"));
    }

    #[test]
    fn unsaved_buffers_refuse_the_rename_because_the_index_reads_disk() {
        let home = PathBuf::from("/p/a.rs");
        let resolved = resolution(&home, ResolutionTier::LocalFile);
        assert_eq!(
            plan_index_rename(&resolved, &[symbol(&home, 1, 3, true)], &[], "fresh", true),
            Err(RenameRefusal::UnsavedChanges),
        );
    }

    #[test]
    fn a_uniquely_named_symbol_resolves_every_site_in_its_own_file() {
        let home = PathBuf::from("/p/a.rs");
        let elsewhere = PathBuf::from("/p/b.rs");
        let resolved = resolution(&home, ResolutionTier::LocalFile);
        let usages = vec![
            symbol(&home, 1, 3, true),
            symbol(&home, 7, 8, false),
            symbol(&elsewhere, 2, 4, false),
        ];

        let plan = plan_index_rename(
            &resolved,
            &usages,
            &[symbol(&home, 1, 3, true)],
            "fresh",
            false,
        )
        .unwrap();

        assert!(!plan.ambiguous);
        assert_eq!(plan.sites[0].confidence, SiteConfidence::Resolved);
        assert_eq!(plan.sites[1].confidence, SiteConfidence::Resolved);
        assert_eq!(
            plan.sites[2].confidence,
            SiteConfidence::Unverified,
            "another file's occurrence is a name match, nothing more",
        );
        assert!(
            plan.sites.iter().all(|s| s.checked),
            "with one declaration of the name there is nothing to mistake it for",
        );
    }

    #[test]
    fn a_shared_name_leaves_the_uncertain_sites_unticked() {
        let home = PathBuf::from("/p/a.rs");
        let elsewhere = PathBuf::from("/p/b.rs");
        let resolved = resolution(&home, ResolutionTier::Project);
        let usages = vec![symbol(&home, 1, 3, true), symbol(&elsewhere, 9, 2, true)];
        let definitions = vec![symbol(&home, 1, 3, true), symbol(&elsewhere, 9, 2, true)];

        let plan = plan_index_rename(&resolved, &usages, &definitions, "fresh", false).unwrap();

        assert!(plan.ambiguous, "two symbols share this name");
        assert!(
            plan.sites
                .iter()
                .all(|s| s.confidence == SiteConfidence::Unverified),
            "name matching cannot tell two same-named symbols apart",
        );
        assert_eq!(
            plan.checked_sites().count(),
            0,
            "the safe default is to change nothing until the user says which",
        );
    }

    #[test]
    fn a_resolved_symbol_with_no_occurrences_is_refused() {
        let home = PathBuf::from("/p/a.rs");
        assert_eq!(
            plan_index_rename(
                &resolution(&home, ResolutionTier::LocalFile),
                &[],
                &[],
                "fresh",
                false
            ),
            Err(RenameRefusal::NoSites),
        );
    }

    #[test]
    fn ticked_sites_become_spans_the_existing_applier_understands() {
        let home = PathBuf::from("/p/a.rs");
        let resolved = resolution(&home, ResolutionTier::LocalFile);
        let usages = vec![symbol(&home, 4, 11, false)];
        let plan = plan_index_rename(
            &resolved,
            &usages,
            &[symbol(&home, 1, 3, true)],
            "fresh",
            false,
        )
        .unwrap();

        let spans = rename_replacements(&plan, "fresh");
        assert_eq!(
            spans,
            vec![FileReplacement {
                path: home,
                line: 4,
                start: 11,
                end: 17,
                text: "fresh".into(),
            }],
            "the span covers the old name exactly, which is what replace_in_files rewrites",
        );
    }

    #[test]
    fn write_files_rewrites_whole_files_and_follows_them_into_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "a.rs", "fn old_name() {}\n");
        let mut index = TextIndex::build(dir.path()).unwrap();

        let report = index
            .write_files(&[(file.clone(), "fn new_name() {}\n".to_string())])
            .unwrap();

        assert_eq!(report.files, 1);
        assert_eq!(report.skipped_files, 0);
        assert_eq!(fs::read_to_string(&file).unwrap(), "fn new_name() {}\n");
        assert!(
            index.find_definitions_exact("old_name").unwrap().is_empty(),
            "the index followed the write",
        );
        assert_eq!(index.find_definitions_exact("new_name").unwrap().len(), 1);
    }

    #[test]
    fn write_files_skips_what_it_cannot_write_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let good = write(dir.path(), "a.rs", "fn a() {}\n");
        let unwritable = dir.path().join("no-such-dir").join("b.rs");
        let mut index = TextIndex::build(dir.path()).unwrap();

        let report = index
            .write_files(&[
                (unwritable, "fn b() {}\n".to_string()),
                (good.clone(), "fn c() {}\n".to_string()),
            ])
            .unwrap();

        assert_eq!(report.skipped_files, 1);
        assert_eq!(report.files, 1);
        assert_eq!(
            fs::read_to_string(&good).unwrap(),
            "fn c() {}\n",
            "one unwritable file must not abandon the rest of the refactoring",
        );
    }

    #[test]
    fn a_signature_is_the_declaration_line() {
        let text =
            "use std::fmt;\n\npub fn open(path: &Path) -> io::Result<Self> {\n    todo!()\n}\n";
        assert_eq!(
            signature_from_text(text, 3).unwrap(),
            "pub fn open(path: &Path) -> io::Result<Self>",
            "the trailing brace is the body starting, not part of the signature",
        );
    }

    #[test]
    fn a_signature_broken_across_lines_is_followed_to_its_close() {
        let text = "fn wide(\n    first: u32,\n    second: u32,\n) -> u32 {\n    0\n}\n";
        assert_eq!(
            signature_from_text(text, 1).unwrap(),
            "fn wide(\n    first: u32,\n    second: u32,\n) -> u32",
        );
    }

    #[test]
    fn a_runaway_declaration_stops_at_the_cap() {
        let text = format!("fn many(\n{}) {{\n", "    arg: u32,\n".repeat(20));
        let signature = signature_from_text(&text, 1).unwrap();
        assert_eq!(
            signature.lines().count(),
            SIGNATURE_MAX_LINES,
            "a tooltip shows a signature, not a function: {signature}",
        );
    }

    #[test]
    fn a_declaration_with_no_body_needs_no_trimming() {
        let text = "interface Greeter {\n    void greet(String name);\n}\n";
        assert_eq!(
            signature_from_text(text, 2).unwrap(),
            "void greet(String name);",
        );
    }

    #[test]
    fn a_brace_that_is_not_the_body_is_kept() {
        let text = "fn f(opts: Opts = Opts { retry: true }) {}\n";
        assert_eq!(
            signature_from_text(text, 1).unwrap(),
            "fn f(opts: Opts = Opts { retry: true })",
            "only the brace that opens the body is cut",
        );
    }

    #[test]
    fn there_is_no_signature_off_the_end_of_the_file_or_on_a_blank_line() {
        let text = "fn a() {}\n\n";
        assert!(signature_from_text(text, 99).is_none());
        assert!(signature_from_text(text, 0).is_none(), "lines are 1-based");
        assert!(
            signature_from_text(text, 2).is_none(),
            "a blank line says nothing"
        );
    }

    #[test]
    fn declaration_signature_reads_the_file_it_is_pointed_at() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "a.rs", "fn first() {}\nfn second(x: u8) {}\n");
        assert_eq!(declaration_signature(&file, 2).unwrap(), "fn second(x: u8)",);
        assert!(declaration_signature(&dir.path().join("missing.rs"), 1).is_none());
    }
    #[test]
    fn open_files_are_taken_out_of_the_plan_as_buffer_edits() {
        let dir = tempfile::tempdir().unwrap();
        let open = write(dir.path(), "a.rs", "fn target() {}\nlet x = target();\n");
        let closed = dir.path().join("b.rs");
        let resolved = Resolution {
            name: "target".into(),
            tier: ResolutionTier::LocalFile,
            candidates: vec![symbol(&open, 1, 3, true)],
        };
        let usages = vec![
            symbol(&open, 1, 3, true),
            symbol(&open, 2, 8, false),
            symbol(&closed, 4, 2, false),
        ];
        let mut plan = plan_index_rename(
            &resolved,
            &usages,
            &[symbol(&open, 1, 3, true)],
            "fresh",
            false,
        )
        .unwrap();

        let edits = take_buffer_edits(&mut plan, "fresh", &open);

        assert_eq!(
            edits,
            vec![
                BufferRenameEdit {
                    path: open.clone(),
                    line: 1,
                    start_character: 8,
                    end_character: 14,
                    text: "fresh".into(),
                },
                BufferRenameEdit {
                    path: open.clone(),
                    line: 0,
                    start_character: 3,
                    end_character: 9,
                    text: "fresh".into(),
                },
            ],
            "0-based lines, UTF-16 columns, last edit first",
        );
        assert_eq!(
            plan.sites.len(),
            1,
            "the open file's sites are gone, so the disk pass cannot apply them twice",
        );
        assert_eq!(plan.sites[0].path, closed);
    }

    #[test]
    fn buffer_columns_are_counted_in_utf16() {
        let dir = tempfile::tempdir().unwrap();
        // "𝄞" is one char but two UTF-16 code units, so `target` starts at
        // byte 11 and at UTF-16 unit 9 — the number an editor counts.
        let open = write(dir.path(), "a.rs", "let 𝄞 = target();\n");
        let resolved = Resolution {
            name: "target".into(),
            tier: ResolutionTier::LocalFile,
            candidates: vec![symbol(&open, 1, 11, true)],
        };
        let usages = vec![symbol(&open, 1, 11, false)];
        let mut plan = plan_index_rename(
            &resolved,
            &usages,
            &[symbol(&open, 1, 11, true)],
            "fresh",
            false,
        )
        .unwrap();

        let edits = take_buffer_edits(&mut plan, "fresh", &open);
        assert_eq!(edits[0].start_character, 9);
        assert_eq!(edits[0].end_character, 15);
    }

    #[test]
    fn an_unreadable_open_file_still_loses_its_sites() {
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("gone.rs");
        let resolved = Resolution {
            name: "target".into(),
            tier: ResolutionTier::LocalFile,
            candidates: vec![symbol(&open, 1, 0, true)],
        };
        let usages = vec![symbol(&open, 1, 0, true)];
        let mut plan = plan_index_rename(
            &resolved,
            &usages,
            &[symbol(&open, 1, 0, true)],
            "fresh",
            false,
        )
        .unwrap();

        assert!(take_buffer_edits(&mut plan, "fresh", &open).is_empty());
        assert!(
            plan.sites.is_empty(),
            "a file we could not read must not fall through to the disk pass",
        );
    }
    #[test]
    fn a_normal_directory_can_hold_the_index_lock() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(INDEX_DIR_NAME);

        assert!(supports_file_locks(&index_dir));
        assert_eq!(index_dir_for(dir.path()), index_dir);
        assert!(
            !index_dir.join(LOCK_PROBE_FILE).exists(),
            "the probe must not leave a file behind in the project",
        );
    }

    #[test]
    fn an_index_directory_that_cannot_lock_moves_the_index_out_of_the_project() {
        // What a Windows build reading a WSL tree over \\wsl.localhost hits:
        // the directory is there and writable, and no lock can be taken in
        // it, so tantivy could never work there.
        let dir = tempfile::tempdir().unwrap();

        let chosen = index_dir_with(dir.path(), |_| false);
        assert_ne!(
            chosen,
            dir.path().join(INDEX_DIR_NAME),
            "the index must not stay where it cannot lock",
        );
        assert!(
            chosen.starts_with(dirs::cache_dir().unwrap()),
            "it belongs in the cache dir, got {chosen:?}",
        );
    }

    #[test]
    fn an_index_directory_that_can_lock_keeps_the_index_with_the_project() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            index_dir_with(dir.path(), |_| true),
            dir.path().join(INDEX_DIR_NAME),
        );
    }

    #[test]
    fn a_project_whose_index_directory_cannot_even_be_created_still_gets_an_index() {
        // A path that cannot hold a directory at all — here a regular file
        // where .ide-index would go.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(INDEX_DIR_NAME), "in the way").unwrap();

        let chosen = index_dir_for(&root);
        assert!(
            chosen.starts_with(dirs::cache_dir().unwrap()),
            "got {chosen:?}",
        );
    }

    #[test]
    fn two_projects_do_not_share_a_fallback_directory() {
        let a = fallback_index_dir(Path::new("/home/x/alpha")).unwrap();
        let b = fallback_index_dir(Path::new("/home/x/beta")).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_lock_held_by_someone_else_names_the_lock_file_it_could_not_take() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn main() {}\n");

        // The first index holds the writer lock for as long as it lives.
        let _live = TextIndex::build(dir.path()).unwrap();
        let err = match TextIndex::open_or_build(dir.path()) {
            Err(err) => err,
            Ok(_) => panic!("a second writer cannot have the same index"),
        };

        let message = err.to_string();
        assert!(
            matches!(err, IndexError::Locked(_)),
            "a held lock is never a reason to rebuild: {message}",
        );
        assert!(
            message.contains(WRITER_LOCK_FILE),
            "the message must name the lock file so the user can check it: {message}",
        );
        assert!(
            message.contains("does not support file locking"),
            "it must not claim another instance when it cannot know that: {message}",
        );
    }
}
