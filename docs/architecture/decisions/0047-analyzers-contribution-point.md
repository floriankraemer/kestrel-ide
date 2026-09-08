# 0047. The `analyzers` contribution point, and `analysis-core`

## Status

Accepted

## Context

The PHP tooling plan (`docs/architecture/php-tooling-plan.md`, phase B) needs PHPStan and PHP_CodeSniffer findings to reach the editor and the Problems dock through the one diagnostics model ADR-0046 built, without either tool being wired into the host as a one-off.
Four questions had to be answered before any PHP-specific code could be written, because each has a wrong-by-construction answer that only PHP happening to work would hide:

1. **Can an analyzer be a wasm plugin, the way a command or a preview can be?**
   `wit/plugin.wit` gives a guest no way to spawn a process, and PHPStan/PHPCS are always native processes.
   So an analyzer contribution cannot be "code a plugin runs" — it has to be data a native host acts on, the same shape `language-servers` already has for exactly the same reason (`plugin-api/src/manifest.rs`'s `LanguageServerContribution` doc comment).
2. **Where does an output format's parser live?**
   A manifest can name a format id, but a wasm guest can neither spawn PHPStan to get its output nor be trusted with the raw bytes if it somehow received them — parsing untrusted tool output is exactly the kind of logic ADR-0028's sandbox exists to keep out of a guest's reach anyway, and simpler to just not put there.
3. **What transport does an analyzer run over?**
   Every other long-running child process in this app — a terminal, a run configuration, a debuggee — is given a PTY (`run_core::Supervisor`, `PtySize { rows: 24, cols: 120 }`).
   Reading `supervisor.rs` shows why that default exists (line discipline, signals, a shell that behaves like a shell) and reading PHPStan's and PHPCS's own docs shows why it is wrong here: both emit machine-readable output — `--error-format=checkstyle`'s XML, `--error-format=json` — that a 120-column tty hard-wraps mid-tag or mid-token. PHPCS's `--stdin-path` mode also wants to *read* from stdin, which a PTY cannot give cleanly (there is no controlling process writing to the far end the way a shell's user would).
4. **Is there already a piped-exec mechanism to reuse?**
   `vcs_core::cli` (`crates/vcs-core/src/cli.rs`) turned out to be the only process path in the repo that already had a wall-clock timeout, concurrent stdout/stderr drain threads (needed or a child that fills a pipe buffer deadlocks against a parent that hasn't started reading the other one), and optional stdin — precisely what an analyzer run needs, previously written for `git apply`/`git diff`.

## Decision

### 1. `analyzers` is a declarative contribution point; the format table is native code

`plugin-api` gains `ContributionPoint::Analyzers` and `AnalyzerContribution` (B2): an id, a display name, program candidates to probe, fixed argv, an output-format id, and a severity-word map — the same shape `LanguageServerContribution` already established for "a native process, no `[wasm]` component needed, api_version unchanged" (ADR-0033's precedent for `previews`).
The format id (`"checkstyle-xml"`) is free-form data to `plugin-api`, which stays a leaf; it resolves to an actual parser only in `analysis-core`, native code this build ships, never inside the wasm sandbox and never inside a manifest.
Concretely: `crates/analysis-core/src/checkstyle.rs` is a full XML parser with its own severity mapping, fixture-tested, and it is the *only* place a byte of PHPStan's or PHPCS's output is ever interpreted.

### 2. A new Qt-free crate, `analysis-core`, not a growth of `build-core`

A build addresses a whole project on demand and has one shape of invocation and cancellation; an analyzer addresses one file on a keystroke (`OnType`), or a save, or the whole project on request (`Manual`) — the opposite shape, and a second concurrency model bolted onto `build-core::BuildHandle` would have needed `BuildHandle` to grow a per-file key it has no other use for.
`analysis-core` owns detection (`find_program`/`find_config_file`/`composer_require_dev`/`status`, B3), the checkstyle-xml parser (B4), the trigger scheduler (B5), and the unsaved-buffer strategies (B6).
Its dependency row is `diagnostics-core`, `process-exec`, `plugin-api`, `syntax-core`, `app-config` plus `serde`/`serde_json`/`quick-xml` — no tokio, matching every other background-work crate's rule that long work runs on a `std::thread` and reports back through a `Send` closure `ui-shell` forwards to `CxxQtThread::queue()`.

### 3. Analyzers run over pipes, never a PTY

`process_exec::run` (see decision 4) spawns with `Stdio::piped()` for stdout/stderr and, when a buffer strategy calls for it, `Stdio::piped()` for stdin too — never a `portable_pty::PtySize`.
This is the one place in the codebase that deliberately does *not* follow the PTY-for-everything default `run_core::Supervisor` sets for terminals and run configurations: a tty's line discipline and column wrapping are exactly what a terminal user wants and exactly what corrupts a checkstyle-xml or JSON payload mid-token.
PHPCS's `--stdin-path` mode (the `Stdin` unsaved-buffer strategy, B6) also needs to *write* the buffer to the child's stdin and then read its stdout to EOF, which is what a pipe gives directly and a PTY does not give at all in the shape a batch tool expects.

### 4. `process-exec` is extracted from `vcs-core`, not copied

B1 (already landed, `d4bd4f2`) moved `vcs-core/src/cli.rs`'s spawn/pipe/drain/timeout/kill mechanism into its own crate, `process-exec` (std-only, a leaf), and migrated `vcs-core` onto it in the same commit — the same "extract, don't duplicate" move `stdio-framing` made when `dap-core` needed `lsp-core`'s framing (D1-1 precedent).
`analysis-core` depends on `process-exec` directly rather than growing a second copy of drain-thread deadlock-avoidance logic that a `git apply` caller and a `phpstan` caller would otherwise each maintain their own subtly-different version of.
`process-exec` itself carries no opinion about what a failure *means*: `Failure::NotFound`/`TimedOut`/`Io` is the whole vocabulary, and the caller (a `VcsError::GitNotInstalled` here, an `analysis_core::detect::AnalyzerStatus`/`scheduler::RunFailure` there) maps it onto its own meaning — the same split the framing/protocol boundary draws in `stdio-framing`/`lsp-core`/`dap-core`.

### 5. The trigger model: `OnType` (debounced) / `OnSave` / `Manual`, and what "cancels" means

Three triggers, matching the plan's decision recorded before B-phase work started: `OnType` and `OnSave` analyze one file, `Manual` analyzes the whole project.
`analysis_core::Scheduler::schedule_file_run` keeps exactly one in-flight run per `(analyzer_id, file)` via a per-key generation counter — a new keystroke bumps the counter, and a debounce timer or a finished process that finds the counter has moved on drops its own result rather than delivering a stale one.
This is cooperative cancellation, not `Child::kill()`: `process_exec::run` does not hand back a live handle to the spawned child (decision 4's mechanism returns only the finished `Output`), so a superseded run's process keeps running to completion or its own timeout and only its *result* is discarded.
`run_manual` separately refuses to start a second project-wide run while one is in flight — manual runs are serialized, never concurrent with each other or with themselves — which `AnalysisService::inspect_project` (B8) uses to run a batch of enabled analyzers one at a time rather than needing its own second serialization mechanism.
An analyzer whose manifest can only see the saved file (`BufferStrategy::SavedOnly`, B6) has `OnType` silently mapped to `OnSave` by `effective_trigger`, and `degradation_reason` gives the sentence the Analysis settings page shows for it (B9) — a stated property, not an unexplained analyzer that never seems to fire while typing.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Analyzer as a wasm plugin that shells out itself | `wit/plugin.wit` gives a guest no process-spawn capability at all; adding one would widen the sandbox's attack surface for every existing plugin to solve a problem native contribution data already solves. |
| Parse an analyzer's output inside the wasm guest, host only spawns the process | Requires piping untrusted, potentially large tool output into a fuel-metered 64 MiB sandbox for no isolation benefit — the host already fully trusts the analyzer's own manifest-declared argv, so there is nothing left to sandbox by the time output parsing happens. |
| Keep analyzers on a PTY like every other process, accept the wrapping risk | Corrupts exactly the machine-readable output formats analyzers exist to produce; a 120-column terminal is a UX default for a human reading a shell, not a serialization contract, and treating it as one would make PHPStan JSON parsing "usually work" rather than always work. |
| Copy `vcs-core/src/cli.rs`'s mechanism into `analysis-core` rather than extracting it | Produces two independently-maintained implementations of the same deadlock-avoidance logic (concurrent drain threads, wall-clock timeout, kill-on-timeout); a fix to one (as `vcs-core`'s already needed once) would not reach the other. |
| One `Trigger::OnType` debounce timer shared across every open file | A keystroke in file A would reset file B's pending run, which is wrong the moment two files are open in split panes; the per-`(analyzer, file)` generation counter is the same granularity the in-flight-run rule already needs. |

## Consequences

- Positive: adding ESLint, Ruff, or Clippy as an analyzer later is a manifest (C-phase specifics) plus, if its output format is new, one parser function — never a second scheduler, a second buffer strategy, or a second contribution point.
- Positive: `analysis-core`'s checkstyle-xml parser is reusable by any future linter that speaks Checkstyle's report shape, PHP or not, because it was written against the format rather than against PHPCS specifically.
- Positive: the debounce/cancellation logic has unit tests that never spawn a real analyzer (`process_exec::run` against `/bin/sh`/nonexistent binaries), so CI needs neither PHP nor Composer to prove the scheduling rules hold (Risk 7 in the plan).
- Negative / accepted: cancellation is cooperative — a superseded `OnType` run's process is not killed, only its result is discarded — so a slow analyzer typed over repeatedly burns CPU it did not need to. Documented as a `ponytail:`-marked ceiling in `scheduler.rs`; the upgrade path is giving `process_exec::run` a way to hand back a live `Child` to kill, not attempted here because nothing yet measures this as a real cost.
- Negative / accepted: `AnalyzerStatus::DeclaredNotInstalled`'s composer-package matching is not wired into B8's `AnalysisService::analyzer_rows` yet — it always passes an empty package list until C2 adds the PHP-specific `analyzer id -> composer package` mapping, so a tool that resolves to nothing reports the more generic `NotDetected` in the meantime rather than the more specific "declared but not installed" sentence.

## Related

- [ADR-0046: one diagnostics model](0046-one-diagnostics-model.md) — the shared store `analysis-core`'s findings publish into (B8), and the `(source, uri)` keying an analyzer's rows use (`analysis:<analyzer-id>`).
- [ADR-0033: markdown preview](0033-markdown-preview.md) — the `previews` contribution point precedent this ADR's decision 1 follows for "native data, no `[wasm]` component needed, `api_version` unchanged."
- [ADR-0028: the wasm plugin tier](0028-wasm-plugin-tier.md) — why a guest cannot spawn a process, and why untrusted output parsing does not belong inside the sandbox either.
- `docs/architecture/php-tooling-plan.md` — the plan this ADR's phase B belongs to; phase C (the `php-tools` built-in manifest) and phase D (the PHPUnit test runner) build on the crates and contribution point this ADR establishes.
