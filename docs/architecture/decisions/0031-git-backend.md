# 0031. Git backend: `gix` for reads, the `git` binary for anything touching the user's world

## Status

Accepted.
Amended by [§7, what measurement changed](#7-what-measurement-changed-amendment), added after the first time anyone timed this crate rather than reasoning about it.

## Context

`docs/architecture/next-five-features-plan.md`'s F3 ("Git v1") lane needs a Git backend for `vcs-core`: repository discovery, status, HEAD reads, working-tree hunks, staging, commit, branches, remotes, history and blame.
Three backends exist in the Rust ecosystem: `gix` (pure Rust), `git2`/`libgit2` (a C library with Rust bindings), and shelling out to the user's own `git` binary.
None of them is a complete answer alone, and the plan's own ADR-0027 draft laid out why before this crate was written; this ADR records the decision as built, against the actual `gix 0.87.1`, not the `gix 0.87` the plan assumed.

Two things make this decision structural rather than a library choice.
The MXE Windows cross-build has already refused a C dependency once, for OpenSSL, on exactly this ground (ADR-0021).
`git2`/`libgit2` would reopen that refusal for a feature the plan ranks below editor ergonomics.
The gutter (`vcs_core::hunks::HunkCache`, F3-4) runs on every keystroke once a Git-aware editor exists.
A subprocess per keystroke is not a performance ceiling to tune later; it is a different architecture.

## Decision

### 1. Pure reads of object/index state go through `gix`, in-process

Repository discovery (`gix::discover`), HEAD resolution and reads (`Repository::head`, `Repository::head_blob`), status (`Repository::status`, via `gix::Repository::status(progress)`), local branch listing (`Repository::branches`, via `gix::Repository::references().local_branches()`), and commit history (`Repository::log`, `Repository::file_history`, via `Id::ancestors().all()`) never spawn a process.

Checked directly against `gix 0.87.1` rather than assumed from the plan's `0.87`-era description.
`Repository::status(progress)` is real, documented and non-experimental (`gix-0.87.1/src/status/mod.rs:99`), covering both index-vs-worktree and HEAD-vs-index in one call.
There is still no path-filtered revwalk anywhere in `gix-0.87.1/src/revision/` — `rev_walk().selected(pred)` takes a commit-id predicate, not a pathspec.
`Repository::file_history` is therefore the ~40-line walk-and-compare the plan anticipated: for each ancestor commit, look up the path's tree entry against its first parent's and record the commit when the object id differs.
No C dependency appears in the resolved dependency graph: `gix-zlib` routes compression through `zlib-rs`, pure Rust.
This was confirmed by inspection of the actual `cargo build -p vcs-core` output, not by reading `Cargo.toml` declarations — the MXE cross-build claim in the plan holds for the version actually pinned.

`vcs-core`'s own types (`HeadInfo`, `RepoStatus`, `FileStatus`, `ChangeKind`, `LogEntry`) wrap `gix`'s rather than re-exporting them.
This is the same reason `settings-model` wraps `syntax-core`'s and `lsp-core`'s vocabularies rather than leaking them past its own boundary: a future `gix` major-version bump, or a swap to a different read backend, stays inside this crate.

`gix` is taken with `default-features = false` and an explicit feature list (`max-performance-safe`, `sha1`, `status`, `revision`, `blob-diff`, `index`, `dirwalk`, `excludes`, `attributes`) — the crate's defaults pull in write paths (checkout, merge) this layer must never reach for.

### 2. Anything touching the user's configuration, credentials, hooks or signing shells out to `git`

Staging (`git add`, `git apply --cached`), commit, branch create/checkout/delete, and fetch/pull/push all go through `vcs_core::cli::run`, a thin wrapper around `std::process::Command`.
It always sets `GIT_TERMINAL_PROMPT=0`, so a missing credential fails fast on stderr instead of blocking on a prompt nothing can answer.
It applies a 60-second timeout, generous since fetch/push are network calls, not local reads.
It turns a nonzero exit into `VcsError::GitFailed { command, stderr }`, carrying `git`'s own message verbatim rather than a bare exit code.
It reports a missing `git` binary as a distinct `VcsError::GitNotInstalled`, never folded into some other failure.

This is the same reasoning the plan's ADR-0027 draft gave: re-implementing credential helpers, SSH agents, `insteadOf` rewriting, hooks and GPG signing is five different ways to be subtly wrong in a way that looks like this IDE's bug, and the user's already-configured `git` already gets all five right.

Per-hunk staging builds a real unified-diff patch (`staging::hunk_patch`) and feeds it to `git apply --cached`/`--reverse --cached`, tested against a real `git` binary in a scratch repository.
Testing found that a zero-context patch reliably fails with "patch does not apply" even when line numbers are exact, so the patch carries three lines of context on each side, matching `diff -u`'s own default — a deviation from the task doc's original framing ("no context lines"), documented in `staging.rs` itself.

### 3. Hunks are computed in-process, against a `gix`-read blob, never by a subprocess

`vcs_core::hunks::HunkCache` reads a file's `HEAD` blob via `gix` (§1) and diffs it against the caller-supplied working text with `editor_core::diff::diff_lines` — the same Git-free diff engine ADR-0028/0030 already established, not a second implementation.
The cache is keyed by `(path, head_oid, revision)`, where `revision` is a monotonic counter the caller supplies, since this crate has no idea about the live buffer on the other side of the FFI seam.

### 4. Blame shells out, deliberately, even though it is a read

`Repository::blame` runs `git blame --porcelain` rather than using `gix`'s own blame implementation.
This is one of the two reads in this crate that goes through `git` rather than `gix` (§7 makes `file_history` the other), for a different reason than write operations do: `gix`'s blame is young, blame is not on the hot path (nothing calls it per keystroke, unlike hunks), and `git`'s own rename-following is more mature than a reimplementation would be.
The maturity half of that reason has since expired and a better one has replaced it — see §7.
`blame::parse_porcelain` is a real parser against the documented porcelain format, tested against literal fixture output covering the format's real subtlety: a commit's full metadata block is only printed the first time that commit is seen in one invocation, and every later line from the same commit carries just the header and the tab-prefixed content line.

### 5. History and blame are cached, but with a simpler key than the gutter's

`HistoryCache` and `BlameCache` key on `(path, head_oid)` (or `(head_oid, max)` for the whole-repository log) rather than `HunkCache`'s `(path, head_oid, revision)`.
(`BlameCache`'s key gained a third component in §7: unlike a history, a blame is of the *working* file, so `HEAD` alone was not enough to invalidate it.)
A history or blame view is opened on demand by the user, not recomputed on every keystroke, so there is no live-buffer revision to invalidate against — copying the gutter's cache key here would imply a staleness problem this data does not have.

### 6. A hunk revert is an edit, not a write

`Repository::revert_hunk_edit` returns a `vcs_core::TextEdit` (a half-open line range plus replacement text) rather than writing a reverted file to disk.
This mirrors the shape `lsp_core::workspace_edit::TextEdit` already established for "one Ctrl+Z undoes it" (ADR-0019), in line units rather than UTF-16 characters, since a hunk never touches part of a line.
The future bridge task therefore has an edit-shaped value ready to splice into the open buffer inside one `beginEditBlock`, the same seam every other edit source in this IDE already uses.

### 7. What measurement changed (amendment)

The decisions above were made by reasoning about the backends.
This amendment records what changed the first time the operations were timed, on generated fixtures (`crates/vcs-core/tests/common/mod.rs`) through a harness that stays in the tree (`crates/vcs-core/tests/timings.rs`, `#[ignore]`d, release build).
The fixtures are `small` (50 files, 100 commits), `wide` (20 000 tracked files, 5 000 untracked) and `deep` (50 000 commits, target file touched in three of them).

**The core decision holds and was confirmed, not challenged.**
A `git` process costs 2.2 ms to start and run to completion (`cli_spawn_status_porcelain`).
Every operation this crate shells out for costs hundreds of milliseconds *inside* `git`, so the spawn is between 0.3 % and 2 % of it.
There is no version of "replace the subprocess with a library" that is worth anything, and `git2`/`libgit2` stays rejected on ADR-0021's grounds regardless.

**The hot path was never the problem.**
`head_blob` on a 2 000-line file costs 0.025 ms and a full gutter diff 0.14 ms.
Two real defects were found there and fixed — `HunkCache`'s hit branch was unreachable because the view passed a counter it bumped on every call rather than the document's revision, and the bridge read the `HEAD` blob twice per tick — and fixing both halved a settle tick, from 0.183 ms to 0.088 ms.
Halving a tenth of a millisecond is not what anyone was feeling.
Whatever makes a gutter feel slow on a large file is the 300 ms debounce it shares with five LSP requests, not this crate.
Both fixes landed anyway, because an unreachable cache branch and a duplicated read are defects whatever they cost.

**A `gix::Repository` handle needs its object cache switched on, per handle.**
`gix` starts every handle (and every clone of one) with the decoded-object cache empty; nothing in this crate had asked for one.
`Repository::discover` now sets a 16 MiB budget, worth 2.1× on `status` over `wide` (49.3 → 23.3 ms) and 2.5× on `small` (10.9 → 4.3 ms).

**`file_history` moves to `git log --follow`, and this is the real change.**
The walk-and-compare of §1 cost 3 427 ms on `deep`, because `max` can only cap the *matches*: a file touched in three of fifty thousand commits makes it decode a commit, its tree, its first parent and that parent's tree, fifty thousand times.
`gix 0.87.1` still has no path-filtered revwalk and no changed-path bloom filters (`gix-0.87.1/src/revision/walk.rs` exposes `sorting`, `first_parent_only`, `use_commit_graph`, `with_boundary`, `with_hidden` and a commit-id `selected` predicate — no pathspec), so this is structural rather than a tuning problem, and `use_commit_graph(true)` on the walk did not close it.
`git log` answers the same question in 632 ms, 5.4× faster, and `--follow` adds the rename tracking the walk explicitly did not do.
So §4's exception now covers two reads, not one, for the same reason in both cases: `git` has an implementation of this that `gix` does not yet have, and neither is on a hot path.
`Repository::log` (the whole-repository log, which *is* a plain revwalk) stays in `gix`, where it costs 1 ms.

**Blame's cost is `git`, and the seam is irrelevant.**
`git blame --porcelain` on `deep`'s target is 672 ms; parsing its multi-megabyte output is 0.67 ms, a thousandth of it.
Nothing about the porcelain format or the FFI marshalling behind it is worth optimising.

**`gix`'s own blame: the condition §4 set has been met, and a better blocker replaced it.**
Rename following landed in `gix-blame` 0.3.0 (`Options::rewrites`, which also carries `copies`), so "revisit when it is as capable as `git blame`" is no longer the question — but `Repository::blame_file(path, suspect, options)` takes a **commit id** and blames a committed revision.
This IDE blames the file in the working tree, where `git` marks uncommitted lines with the all-zero "not committed yet" commit; swapping would silently attribute the user's unsaved edits to `HEAD`, which is worse than slow.
Record this as the reason, so the question stops being reopened on a maturity argument that has already expired.

**`max-performance` is deprecated and equivalent to `max-performance-safe`.**
`gix` always compresses through `zlib-rs` and exposes no `zlib-ng-compat`/`libdeflate` feature; `max-control`'s own documentation states that no C toolchain is involved.
ADR-0021's constraint is satisfied by construction here, not by this crate's feature list being careful.

**A timed-out `git` child is now killed.**
`cli::run` kept the 60-second timeout but left the process running — a `git fetch` on a dead network held a connection open after the error was already reported, and behind the bridge's single job queue, everything queued after it.
It now drains both pipes in reader threads (the deadlock the old shape existed to avoid) while keeping the `Child` itself, so the timeout branch can kill it.

**Still open, deliberately.** `file_history` at 632 ms on a 50 000-commit repository is better but not fast; `status` at 23 ms per whole-worktree dirwalk still runs on every save and after every stage, uncoalesced and unscoped, and no filesystem-watcher event refreshes it at all.
Those are the next changes, and the second needs its own decision about the `project-model` watcher reaching `VcsService`.


## Consequences

- `vcs-core` has no Qt/cxx-qt dependency, direct or transitive (`docs/architecture/layering.md`'s new row), and depends only on `editor-core`, `gix`, `serde`, and std.
- Every `vcs-core` operation that shells out is tested against a real `git` binary in a `tempfile` scratch repository (staging round-trips, commit with a real rejecting hook, branch force-delete, fetch/pull/push over a real filesystem-transport clone) rather than against a mocked `Command`, matching this repo's stated preference for testing as close to real behaviour as the layer allows.
- `VcsError` carries stable numeric codes in the 700-799 range `next-five-features-plan.md` §5 reserves for `vcs-core`, laid out now even though no FFI seam crosses yet, so the future bridge (F3-12) does not have to translate a wrong shape.
- Two speculative error variants from the task breakdown were not built, and are recorded here rather than left as silent scope-narrowing.
  There is no distinct "hook rejected the commit" error: a pre-commit or commit-msg hook's own stderr is inherited by `git commit` and already lands verbatim in `VcsError::GitFailed`'s `stderr`, and there is no reliable, non-heuristic signal in `git`'s exit code that distinguishes "a hook said no" from any other commit failure — inventing one would mean guessing from stderr text, exactly the fragile parsing this crate exists to avoid doing to a future UI layer.
  There is no distinct "no upstream configured" error either: `vcs_core::Repository::push` always names an explicit remote and branch, and `git push` only refuses for a missing upstream on a bare `git push` with neither named, a shape this crate's own argv construction cannot produce — a `NoUpstream` variant would have been permanently unreachable dead code.

## Alternatives rejected

**Pure `gix`/`git2` for everything, including writes.**
Credential helpers, SSH agents, `insteadOf` rewriting, hooks and GPG signing are five separate re-implementations, each likely to fail in a way that looks like this IDE's bug rather than a known Git limitation, and some (a leaked credential helper invocation) fail by leaking rather than just erroring.

**Pure `git` CLI, including for hunks.**
A subprocess per keystroke for gutter diffs is not a ceiling to raise later; the gutter is the reason this split exists at all.

**`git2`/`libgit2` over `gix`.**
A C dependency in an MXE Windows cross-build ADR-0021 already refused on exactly this ground for OpenSSL.
`gix` and `editor_core::diff`'s `imara-diff` are both pure Rust, which is half of why `gix` was chosen.

**Bundling a `git` binary.**
Then this project owns `git`'s own CVEs and per-platform builds, and overrides the user's own configured `git` (their credential helpers, their `insteadOf` rewrites, their hooks) with one they did not choose.

**`gix`'s own blame implementation, once it matures.**
Rejected outright as of §7, having been merely deferred here.
The maturity condition was met (rename following landed in `gix-blame` 0.3.0), but `Repository::blame_file` blames a commit, and this IDE blames the working tree.
