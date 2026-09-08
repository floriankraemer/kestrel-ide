# 0048. The test runner: TeamCity streaming, `test-core`, and diagnostics

## Status

Accepted

## Context

The PHP tooling plan (`docs/architecture/php-tooling-plan.md`, phase D) needs a real test tool window: a suite/class/method tree that fills in as PHPUnit runs, a failure pane, and rerun (all / failed / one node).
Three questions had to be answered before any of that could be built, each with a wrong-by-default answer the existing precedent (analyzers, B-phase) does not itself resolve:

1. **What does PHPUnit's output actually look like, and what does that force?**
   PHPUnit can emit a JUnit-XML report (`--log-junit`) or stream TeamCity service messages (`--teamcity`) to stdout as it runs.
   JUnit XML is a single well-formed document written once, at the end — reading it as it grows is reading invalid XML for the whole run.
   TeamCity's format is line-oriented (`##teamcity[testStarted name='...']`, one message per line) specifically so a consumer can act on each one as it arrives.
   A tree that "fills in while the run is in flight" is a requirement about *when* data is usable, not just what it contains, and only one of the two formats supports that.
2. **Does `process_exec::run` (B1/ADR-0047) support that?**
   No: it spawns, drains both pipes to completion on separate threads, waits for exit, and only then returns a finished `Output`.
   That is exactly right for an analyzer's batch report (checkstyle-xml, ADR-0047) and exactly wrong for a test run's live tree — a caller cannot act on stdout before the function that reads it has already returned.
3. **Is a test run's state shaped like anything that already exists?**
   `analysis-core`'s findings are a flat list per file, replaced wholesale each run (ADR-0047 decision 1's severity-mapped `Diagnostic`s).
   `build-core`'s diagnostics are the same shape.
   A test run is a tree with identity that persists *across* runs — rerunning one failed test must not erase every other test's last result — and each node carries its own live status, duration and (for a leaf) a failure's message/details.
   Neither existing shape fits without being bent into something a build or an analyzer would then also have to carry unused fields for.

## Decision

### 1. TeamCity service messages are the primary format; JUnit XML is a batch fallback

`test-core::teamcity::TeamCityParser` is a streaming, stateful parser: `feed(&mut self, chunk: &str) -> Vec<TeamCityEvent>` takes whatever bytes a pipe read handed over (not necessarily a whole line) and returns every complete service message found so far, buffering a trailing partial line for the next call — the same incremental shape `build_core::parser::DiagnosticParser::feed` already has for compiler output.
`test-core::junit::parse` is an ordinary whole-document parser (`quick_xml`, the same library and style as `analysis-core::checkstyle`) for a framework whose manifest names no streaming format; `TestTree::apply_junit` fills the tree from its complete result list in one pass rather than incrementally, because there is nothing incremental to fill it from — a JUnit report carries no per-test timeline, only the end state.
`php-tools`' `phpunit` row (D7) contributes `--teamcity` and `output-format = "teamcity"` only: PHPUnit supports TeamCity directly, so there is no fallback argv to add for it — the JUnit parser exists in `test-core` for a future test framework whose manifest cannot name a streaming format, not because PHPUnit itself ever needs it.

### 2. `process_exec` gains `spawn`/`Spawned`, a streamed sibling to `run`

Rather than give `test-core` its own second spawn-and-drain mechanism, `process_exec::spawn` returns a `Spawned` (an `Arc<Mutex<Child>>` wrapper) immediately after the child starts, with `take_stdout`/`take_stderr` to read pipes as bytes arrive and `kill`/`wait` callable from another thread — the same "read while running, kill from elsewhere" shape `build_core::runner` gets from `run_core::Supervisor`, but over pipes rather than a PTY, for the reason ADR-0047 already gives analyzers: a tty's column wrap would corrupt a TeamCity message split mid-line exactly as it would corrupt checkstyle-xml.
`test_core::runner::run` is the blocking loop built on top: it drains stderr on its own thread (the same pipe-fills-and-blocks deadlock `process_exec::run` avoids), reads stdout on the caller's thread through the `TeamCityParser`, and calls a `TestSink` trait (`output`/`event`) for each chunk and each parsed message as they happen — mirroring `build_core::runner::run`'s `BuildSink` shape so `ui-shell`'s `TestServiceRust` (D4) is translation-only: spawn a `std::thread`, forward `TestSink` callbacks into `CxxQtThread::queue()`, exactly as `BuildServiceRust`'s `QtSink` does for a build.

### 3. `test-core` is its own crate, not folded into `analysis-core` or `build-core`

A test run's tree has to persist identity across reruns (`TestId`, qualified by nested suite names — `Suite::method`, precisely PHPUnit's own `--filter` vocabulary) and update a node's status *and* its ancestors' aggregate status (a suite is `Failed` if anything under it is, `Running` if anything still is, otherwise the best of what is left) on every incoming event.
Neither `analysis-core` (flat findings, replaced wholesale) nor `build-core` (flat diagnostics, replaced wholesale) has a tree, a per-node identity, or an aggregation rule to reuse, and bolting one onto either would give every analyzer/build caller fields and invariants only a test run needs.
`test-core`'s dependency row is `diagnostics-core`, `process-exec`, `plugin-api` plus `quick-xml` — no `syntax-core` or `app-config` (nothing here needs a file-to-language join or a settings section yet, unlike `analysis-core`), and no tokio, matching every other background-work crate: `runner::run` blocks its caller's own `std::thread`.
Rerun's `--filter` pattern construction (`test_core::filter`, D6) lives in this crate rather than in the `ui-shell` adapter for the same reason `analysis-core`'s scheduler does: it is a rule ("a suite reruns as a prefix match, a test as an exact match, a regex metacharacter in a data-provider suffix gets escaped") that deserves a unit test in a Qt-free crate, not an untested `ui-shell` helper.

### 4. A failing test is a diagnostic, published into the one shared store (ADR-0046)

`test_core::diagnostics_by_file` walks every node currently carrying a failure and locates a `path:line` inside its free-text details, the same rule shape `run_core::links` documents (a path ends in a `.`-extension, the line half is plain digits) but implemented locally rather than by depending on `run_core`: neither TeamCity's `testFailed` message nor JUnit's `<failure>` element gives a structured location the way an LSP diagnostic does, only a message and a stack trace/diff, and `test-core`'s layering row carries no adapter-layer crate for `run_core` to be reached through.
`TestServiceRust` (D4) republishes this grouping into `DiagnosticsServiceRust`'s shared store under a `test:<framework>` key on every TeamCity event, the same "regroup wholesale, since a later event can add a failure to a file an earlier one already reported on" rule `BuildServiceRust::republish` and `AnalysisServiceRust::publish_result` already follow — so a failing assertion is underlined in the editor and listed in the Problems dock exactly like a linter warning or a compiler error, with no second Problems surface built for it.

## Alternatives considered

| Option | Why rejected |
|---|---|
| JUnit XML only, read after the process exits | Meets none of "the tree fills in while the run is in flight" — the whole point of a live test window, and the plan's explicit requirement (`docs/architecture/php-tooling-plan.md`'s "Decisions taken before work starts"). |
| Poll a growing JUnit XML file while the process is still writing it | The file is not well-formed until the closing `</testsuites>` is written; a partial-document parse is either a parser that tolerates truncation (fragile, format-specific) or frequent parse failures treated as "nothing new yet," neither of which TeamCity's line-oriented design needs at all. |
| Give `test-core` its own PTY-based spawn (`run_core::Supervisor`), matching every other long-running process in the app | Same corruption risk ADR-0047 rejected for analyzers: a tty hard-wraps at a fixed column, and a `##teamcity[...]` message split mid-line by a wrap is not a message the parser can recover. |
| Extend `process_exec::run` itself to take a per-chunk callback rather than adding `spawn`/`Spawned` | `run`'s callers (`vcs-core`, `analysis-core`) all want "wait for the whole thing, then decide" and none want a callback; adding one to `run` either forces every caller to pass a no-op or forks the function's contract in two directions for no shared benefit. `spawn` is the minimal addition: the same spawn/pipe/kill mechanism, handed back live instead of drained internally. |
| Fold test state into `analysis-core::AnalyzerDef`/a new "tree" variant of its findings | Every analyzer caller (present and future) would carry tree/aggregation fields it never sets, and `analysis-core`'s "replace wholesale each run" model is actively wrong for a test tree, where rerunning one failed test must leave every other node's last result alone. |
| Locate a failure's `file:line` by depending on `run_core::links::resolve_link` from `test-core` | Would add an adapter-layer crate to a support-layer crate's dependency row (`docs/architecture/layering.md`), the same kind of layering violation ADR-0002 exists to prevent; the small local heuristic costs a few lines and is unit-tested the same as the catalogue it mirrors. |

## Consequences

- Positive: the Tests dock's tree updates node-by-node as a run streams, not in one jump at the end — the UX requirement the plan states plainly, delivered by the format PHPUnit already speaks rather than by polling or guessing at partial XML.
- Positive: `process_exec::spawn` is a small, general addition (spawn/pipe/kill, streamed) that any future long-running-and-observable child process can reuse without a PTY, not a PHPUnit-specific mechanism.
- Positive: a failing test reaches the editor's squiggles and the Problems dock for free, through the store ADR-0046 already built — no second "where do test problems show up" surface, no second severity model.
- Negative / accepted: `test-core`'s failure-location heuristic (decision 4) is a last-line-first scan for a `path:line` shape, not `run_core::links`' full regex catalogue — a wrong file only means one failing test's squiggle lands in the wrong place, never a panic or a dropped tree update, and it is marked `ponytail:` in `test-core::diagnostics` with its upgrade path (promote the candidate-finding rule into a Qt-free crate both `run-core` and `test-core` could depend on, if this heuristic ever mislocates something that matters).
- Negative / accepted: only one test framework can be "the" project's run at a time (`TestServiceRust` detects the first contributed framework whose program resolves, mirroring `build-core`'s "one toolchain per project" rule) — a project with two test frameworks installed runs whichever is listed first in its plugin's manifest, not both. Acceptable for v1 with a single built-in test-frameworks contribution (`phpunit`); revisit if a second one is ever added.
- Negative / accepted: the JUnit-XML parser and `TestTree::apply_junit` currently have no built-in caller — `php-tools`' `phpunit` row never needs the fallback, since PHPUnit speaks TeamCity directly. It stays in `test-core`, fixture-tested, as the generalized primitive the plan asked for rather than dead code invented for its own sake: the very next test-frameworks contribution that cannot stream is a manifest change away from using it, not a new parser.

## Related

- [ADR-0046: one diagnostics model](0046-one-diagnostics-model.md) — the shared store a failing test's diagnostics publish into (D3), and the `(source, uri)` keying a test's rows use (`test:<framework-name>`).
- [ADR-0047: the `analyzers` contribution point](0047-analyzers-contribution-point.md) — the sibling contribution point (`TestFrameworkContribution`, D1) and the piped-not-PTY reasoning this ADR's decision 2 extends from a batch report to a streamed one.
- `docs/architecture/php-tooling-plan.md` — the plan this ADR's phase D belongs to.
