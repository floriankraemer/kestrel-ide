# 0008. Project index: hybrid tantivy(ngram) + ripgrep-crates for text, name-based symbol/reference schema

## Status

Accepted.
The `windows-artifact` Docker target (MXE cross-build to `x86_64-pc-windows-gnu`) built
`app.exe` clean with `index-core` (tantivy + `zstd-sys` + ripgrep crates + `ignore`) in the
dependency tree, no Dockerfile changes needed. Verified via the resulting binary's PE import
table (`objdump -p`): no new runtime DLL dependency from this crate — `zstd-sys` and the
ripgrep crates are fully statically linked. The Linux `linux-builder` build was already
verified clean as of task G1. The gate this ADR was pending on is closed.


## Amendment: packed symbol documents, a size cap, and a parallel build

Three things this ADR describes changed once the build was actually measured (see [the index performance plan](../index-performance-plan.md) for the numbers — WordPress `wp-includes` took eight minutes).

**One symbol document per (file, name), not per occurrence.**
The original schema wrote a document per identifier occurrence, each repeating the file's whole path.
Occurrences of one name in one file now pack into a single document, with line, column, is-definition, kind and container carried as five multi-valued fields appended together so the nth value of each describes the same occurrence.
Because a packed document matches `sym_is_definition:1` when *any* of its rows is a definition, every definition query re-filters the expanded rows; that filter is correctness, not an optimisation.
`sym_kind`, `sym_container`, `sym_line` and `sym_col` lost their `INDEXED` flag, since nothing ever queried them.

**A size cap.**
A file past 2 MiB contributes no documents at all — no content, no symbols — while still counting as indexable so it keeps its place in the file-name tier.
A stamped empty-content document was tried first and rejected: its path re-entered the candidate list whenever the ngram stage fell back to "every text doc", so Find in Files reached into an over-cap file on some patterns and not on others.
Binary files are still dropped entirely; the difference is that a NUL byte in the first 8 KiB decides it before the whole file is read.

**The build is parallel, and reports progress.**
`IndexWriter::add_document` and `delete_term` take `&self`, so the walk splits into a cheap single-threaded pass that establishes the file list and a rayon pass that reads, parses and builds documents — no mutex over the writer.
Walking first is also what makes the total known before any file is read, which is the difference between a progress bar and a spinner; `index-core` reports it through a `&(dyn Fn(IndexProgress) + Sync)` and stays Qt-free.
A `stamps.tsv` sidecar next to the index replaces deserialising every stored document on a warm open, and is safe by construction: a stamp that is missing or does not match makes the file be re-read, so a stale sidecar costs work, never correctness.

## Amendment: where the index lives when the project's filesystem cannot lock it

Tantivy allows one `IndexWriter` per directory and enforces it with an advisory lock on `.tantivy-writer.lock` inside the index directory.
Some filesystems cannot take advisory locks at all — a Windows build reading a WSL tree over `\\wsl.localhost`, SMB and NFS shares, some FUSE mounts — and tantivy reports every lock failure as `LockBusy`, including that one.
The result was a project that could never be indexed, reported as "another IDE instance is already indexing" when no other instance existed.

So `index_dir_for` probes the directory first, taking and releasing a lock with the same crate tantivy uses, and when the directory cannot host a lock the index goes to `<cache_dir>/ide/index/<project path>` instead.
Two projects at the same path collide there and the index no longer travels with the project; both are better than a project that cannot be indexed.

The error text no longer claims a cause it cannot know. It names the lock file and gives both possibilities, because the one thing the user can do — check whether another instance is running — needs the path.

## Amendment: R8 — scoped Find in Files, and a real regex literal-prefix narrowing

`docs/architecture/intellij-parity-refinement-plan.md`'s R8 closes the two scope boundaries the base decision above named as accepted, not fixed: regex candidate narrowing falling back to "every indexed file", and there being no file mask or path scope at all.

**Regex literal-prefix narrowing.**
`regex-syntax` (already a transitive dependency via `grep-regex`, now promoted to direct) parses the pattern into an `Hir` and `regex_syntax::hir::literal::Extractor` (`ExtractKind::Prefix`) extracts what a match is *required* to start with.
Only the single-alternative case narrows: `Seq::literals()` for an alternation like `(abc|xyz)` holds every alternative a match may start with, not one prefix shared by all of them, so narrowing on just one of them would wrongly drop a file that only contains the other.
A prefix under three characters produces no ngram terms either, so it is treated the same as "no prefix" — still correct, just not narrowed.
This is additive: `TextIndex::search` (the pre-R8 entry point) is untouched, and `search_scoped` (the new one, in `crates/index-core/src/search_scope.rs`) is what the bridge now calls.

**`SearchScope`: a file mask and a path allow-list.**
`globset` (already used by `lsp-core`, now also a direct `index-core` dependency) backs `FileMask::parse`, an IntelliJ-style comma/semicolon-separated glob list with `!pattern` negation (`*.rs, !*_test.rs`).
The path allow-list is how the view expresses Project/Directory/Open Files/Current File scope crossing the FFI seam: rather than a scope-kind enum reaching into `index-core`, the bridge resolves "Open Files" or "Current File" to an actual path list itself (it already has to, to show the choice), and an empty list means "the whole project" — `index-core` only ever sees paths, never a scope *kind*.
Both the mask and the path allow-list are applied as a post-filter on the ngram-narrowed candidate set, in `TextIndex::candidate_files_scoped` — narrowing first, then filtering, keeps a scoped search no slower than an unscoped one on the files it was always going to skip.

**`total_hint`.**
`search_scoped`'s result carries a match count that keeps incrementing past the 10,000-match cap instead of stopping the scan the instant the cap is hit, so the view can show "showing N of M — refine your search" rather than a bare, possibly-misleading count.

See `crates/index-core/src/search_scope.rs` for the implementation and its unit tests.

## Context

The language-folding/Class-View/terminal/search plan (task G, tracks G1→H and E1→J) calls for
project-wide search covering both "Find in Files" (text) and symbols/references (go-to-symbol,
find-usages).
A naive full-text index (index every substring or every word) is either too slow to build on a
large repo or too imprecise to give exact match spans; a naive on-demand `grep`-style scan over
the whole repo on every keystroke doesn't scale past a small project.
Task G1 builds the text half of this; the symbol/reference half is task E1, deliberately not
built yet.

## Decision

New Qt-free crate `index-core` (`crates/index-core`, mirrors `editor-core`/`project-model`/
`syntax-core`/`pty-core`), two-stage hybrid search rather than a single full-text engine:

- **Index build** (`TextIndex::build`): walk the project root via the `ignore` crate (the same
  crate ripgrep itself uses), which is gitignore-aware and skips hidden files by default —
  deliberately not reusing `project-model::DirectoryTree::build()`'s raw walk, which has no
  ignore-file awareness and would waste an index on `node_modules`/`target`/etc. Each readable
  UTF-8 text file becomes one tantivy document: a `path` field (`STRING | STORED`, exact-match
  term for delete/update) and a `content` field indexed with a custom **ngram(3) tokenizer** —
  not tantivy's default word tokenizer, which would miss mid-word substring matches. The index
  lives at `<project_root>/.ide-index/`.
- **Search** (`TextIndex::search(pattern, is_regex)`): the tantivy ngram index narrows the whole
  project down to a small set of candidate files (or, if the pattern is too short to produce
  ngram terms, or the query fails to parse — regex metacharacters aren't literal ngram terms —
  every indexed file, correctness-preserving but not narrowed). Each candidate is then
  re-scanned with `grep-searcher`/`grep-regex`/`grep-matcher` (ripgrep's own library crates) to
  produce exact line numbers and byte-offset match spans. This two-stage split is what gives
  ripgrep-grade correctness (real regex semantics, exact spans) with index-backed speed on large
  repos, instead of either a slow full-repo regex scan every keystroke or an imprecise
  pure-tantivy-scored result.
- **Incremental updates**: `reindex_file`/`remove_file` delete-then-reinsert (or just delete) a
  single document by its `path` term and commit — no full rebuild. The eventual `ui-shell`
  integration (task H) calls these from `project_model::ProjectWatcher`'s callback, same
  structural-vs-content-only distinction (`is_structural_change`) the project tree sidebar
  already uses.
- **Symbol/reference schema is out of scope for this task.** `index-core`'s module doc marks
  where it lands (Task E1: name/kind/file/line/container/`is_definition` fields fed by
  `syntax-core::outline()` and `identifier_occurrences()`) so E1 can add a second tantivy
  schema/segment without reworking the text half built here.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| Pure full-text tantivy index (word tokenizer, tantivy-scored results only) | Wrong tool for exact substring/regex search — word tokenization misses mid-identifier substrings (e.g. searching `Handler` inside `RequestHandlerFactory`), and tantivy's own match scoring doesn't give exact byte-offset spans a "Find in Files" UI needs to highlight. |
| Pure on-demand `grep`-style scan (no index at all) | Correct and simple, but doesn't scale — a full-repo regex scan on every keystroke is the exact "fastest IDE on earth" ceiling this plan is trying to avoid on a large repo. |
| Reusing `project-model::DirectoryTree`'s walk for indexing | Not confirmed gitignore-aware, and indexing `node_modules`/`target`/build output would be both slow to build and full of noise in search results. The `ignore` crate is ripgrep's own dependency for exactly this walk. |
| Cross-file type/binding resolution for symbols (deferred to E1, noted here since it shapes the schema) | A real language-server-grade binding resolver is out of scope for this plan — the symbol schema is deliberately name-based (same-name matches across files), a documented scope boundary short of a language server, not an oversight. |

## Consequences

- Positive: text search stays correct (ripgrep-grade regex/span semantics) while getting
  index-backed candidate narrowing instead of a full-repo scan per query.
- Positive: incremental re-index is a single-document delete+insert, not a full rebuild, so a
  save doesn't cost the whole project's indexing time.
- Negative / accepted risk: tantivy pulls in `zstd-sys`, a C dependency, whose Windows/MXE
  cross-build behavior is not yet verified — this is the open item this ADR stays Proposed on.
  Linux build/tests are clean under Docker `linux-builder` as of this task.
- Negative / accepted scope boundary: regex search candidate narrowing falls back to "every
  indexed file" (no speed gain, correctness unaffected) whenever the tantivy query parser can't
  parse the raw regex pattern as a query string — extracting a literal prefix/substring from the
  regex to narrow with instead is a documented future upgrade, not built here (see the
  `ponytail:` comment in `crates/index-core/src/lib.rs`).
- `.ide-index/` is written inside the project root; it is not gitignored by this crate itself —
  whichever task wires this into `ui-shell` (task H) should make sure a project's own
  `.gitignore` excludes it, or `index-core` will index its own index directory's stale copies on
  a later rebuild of a *different* project that happens to nest inside this one. Not a concern
  for a single-root project indexing itself, since `build()` always removes and rebuilds
  `.ide-index/` before walking.

## Related

- [ADR-0003: FFI seam conventions](0003-ffi-conventions.md) — typed-error convention `index-core`
  follows internally now, ahead of crossing the FFI seam in task H.
- [ADR-0007: Embedded terminal](0007-embedded-terminal.md) — same "Qt-free crate, `std::thread` +
  `CxxQtThread::queue()` for background work" shape this crate's eventual `ui-shell` integration
  will reuse for background indexing.
- `crates/index-core/src/lib.rs` — the crate this ADR documents.
- `docs/architecture/language-folding-classview-terminal-search-plan.md` — decision 4 and the
  G1/E1/H/I/J task breakdown this ADR covers.
