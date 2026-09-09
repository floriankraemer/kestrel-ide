# 0053. Working-tree status is read from `git status --porcelain=v2`

## Status

Accepted.
Amends [ADR-0031](0031-git-backend.md) §1: status moves from `gix::Repository::status(progress)` to the `git` subprocess ADR-0031 §2 already routes writes through.
Every other read `gix` performs — HEAD, blobs, branches, log, blame's own exception — is unaffected.

## Context

The Changes dock (`crates/ui-shell/cpp/changes_panel.cpp`, F3-17) reads `Repository::status`, an in-process `gix::Repository::status(progress)` walk (ADR-0031 §1).
Measured against a real `git status` in a terminal, on the same worktree, three kinds of answer disagreed:

- **Merge conflicts vanished.** `unstaged_kind` returned `None` for `EntryStatus::Conflict` — a file both sides had touched showed no change at all.
- **Renames read as untracked additions.** The old path was reported as deleted, the new path as untracked, never paired — `git status`'s own `--renames` detection was not asked for.
- **Ahead/behind never appeared.** `RepoStatus` carried no branch, upstream, or ahead/behind counts — `gix::Repository::status`'s object is a worktree diff, not `git status --branch`'s combined answer.

The fourth disagreement is Windows-specific, and the one that makes this a structural fix rather than a bug list.
For a project opened from `\\wsl.localhost\<distro>\...` (ADR-0052), `Repository::status` — an in-process `gix` walk — runs **Windows-side**, over the 9P share, reading an index that `git add`/`git commit` last wrote **from inside the distro**.
Staged state is therefore a Windows stat walk of a Linux-written index: `EntryStatus::NeedsUpdate` was mapped to "no change", so a file the user just edited inside the distro could show as clean in the dock while `git status` in a WSL shell called it modified.
ADR-0052 fixed this asymmetry for every write (staging, commit, branch, fetch/pull/push already shell out, ADR-0031 §2) and for every other read `process_exec::run`/`spawn` routes through `ExecHost` — but `Repository::status` bypassed both, being a `gix` call with no process to classify.

ADR-0031 §1 itself states the rule this falls under: "anything touching the user's configuration, credentials, hooks or signing shells out to `git`."
Status is configuration-dependent in every direction that matters here — `.gitattributes`, `core.autocrlf`, `core.excludesFile`, sparse-checkout, submodule config, and (per the above) *which host's git and index* are authoritative — so it was always closer to ADR-0031 §2's side of the line than §1's; the perceived cost (a subprocess per status read, on the same class of concern the gutter's per-keystroke diff exists to avoid) was the only pull toward the `gix` side, and §7 of ADR-0031 already asks for that pull to be measured rather than assumed.

## Decision

`Repository::status` (`crates/vcs-core/src/repo.rs`) now runs

```
git status --porcelain=v2 -z --branch --untracked-files=all --renames
```

through `vcs_core::cli::run` (the same wrapper ADR-0031 §2 uses for staging/commit/branch/remote) and hands the output to a new pure parser, `crates/vcs-core/src/status.rs::parse_porcelain_v2`.
One command now yields everything the dock needs in a single round trip: `# branch.head`/`# branch.upstream`/`# branch.ab +n -n`, ordinary `1`/`2` entries with both index and worktree letters, `2` rename/copy entries carrying the original path, `u` unmerged entries, and `?` untracked paths — replacing four previously separate concerns (`RepoStatus::untracked` was a second list the bridge already folded back together at the FFI seam, so absorbing it into `files` deletes code rather than adding it).

Because this goes through `cli::run`, it inherits `ExecHost`'s classification (ADR-0052) for free: a WSL project's status is now read by the *distro's* `git`, against the *distro's* index — the same `git` that performs the writes.
That is the Windows fix this ADR exists to record; nothing else in this decision is Windows-specific.

`ChangeKind` gains `Renamed`, `Copied`, `Conflicted`, `Untracked`; `FileStatus` gains `orig_path: Option<PathBuf>`; `RepoStatus` gains `branch: Option<String>`, `upstream: Option<String>`, `ahead: u32`, `behind: u32`.
The `gix`-side helpers this replaces (`unstaged_kind`, the status-specific half of `bstr_to_path`) are deleted, not kept as a fallback — a second status path reading a different combination of "index vs. worktree vs. HEAD" than the one the CLI reports would be exactly the kind of two-answers-that-can-disagree problem this ADR exists to close.

### What changed, measured (`crates/vcs-core/tests/timings.rs`, `#[ignore]`d, release build)

ADR-0031 §7 set the standard this decision is held to: reason first, then measure, and record what the measurement actually said rather than what seemed likely.

A `git` process costs 2.245 ms (p50) to spawn and run `status --porcelain` to completion on the `small` fixture (`cli_spawn_status_porcelain`) — consistent with §7's own 2.2 ms figure for the same measurement, so the spawn cost itself has not moved.
The full `--porcelain=v2 --branch --renames` status this ADR adds costs 2.251 ms (p50) on `small` and 31.682 ms (p50) on `wide` (20 000 tracked files, 5 000 untracked) — both are the `status` row in `timings.rs`'s own output, which now measures the CLI path directly, since `Repository::status` *is* this path after this change.

That `wide` number is **not** a straightforward win over the `gix` walk it replaces: ADR-0031 §7 recorded the pre-existing `gix` walk, after its own object-cache fix, at 23.3 ms on the same fixture — the CLI path is slower on a large, all-tracked worktree.
This is accepted, not overlooked: the correctness gap (conflicts, renames, ahead/behind, and the Windows asymmetry above) is not something a faster wrong answer is worth trading for, and `refreshStatus` is already watcher-driven and coalesced (ADR-0031 §7's own fix) rather than called per keystroke — the class of cost this crate's `gix`-first default exists to avoid does not apply to a whole-worktree status read that already happens at most once per settle window.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Keep `gix::Repository::status`, add ad-hoc handling for conflicts/renames/ahead-behind | `gix`'s status object has no unmerged-entry representation to read a conflict from, and no branch/upstream/ahead-behind fields to add ahead-behind to — this is not a gap in how the existing code reads `gix`'s answer, it is a gap in what `gix::status` computes. It also does nothing for the Windows asymmetry, which is a *host*, not a *library*, problem. |
| Classify `Repository::status` through `ExecHost` directly, keep it as a `gix` call executed conditionally on a remote root | `gix` has no way to run "inside" a WSL distro at all — the object database it reads is a set of files on whichever side opened them. The fix requires an actual subprocess on the distro side; there is no `gix`-only version of this decision. |
| A second, WSL-only status path, `gix` everywhere else | Two status implementations that can disagree is the exact class of bug ADR-0031 §1's original split was supposed to avoid, and is worse than the bug this ADR fixes: at least the old code was *consistently* wrong on Windows. |
| Scope the CLI call to a pathspec instead of a whole-worktree read | ADR-0031 §7 already measured and rejected this for the `gix` walk (24 ms whole-worktree vs. a scoped read, on `wide`, "did not justify teaching `RepoStatus` to merge a partial answer into a cached whole one") — the same reasoning applies unchanged to the CLI form; nothing about moving to a subprocess changes that trade-off. |

## Consequences

- Positive: the Changes dock's Merge Conflicts group, rename display, and Push/Pull ahead/behind counts are now real — none of the three existed on the `gix` walk without incorrect or missing data.
- Positive: a WSL project's status agrees with `git status` run in a WSL shell, for the same reason every write already did (ADR-0052) — status was the one read left outside that guarantee.
- Positive: `RepoStatus::untracked` as a separate list is deleted; the bridge's own fold-together at the FFI seam (`vcs/mod.rs`) goes with it.
- Negative / accepted: a whole-worktree status read now costs a process spawn plus `git`'s own walk instead of an in-process one, measured at parity on a small worktree (2.25 ms vs. 2.2 ms baseline) and slower on a large, all-tracked one (31.7 ms vs. 23.3 ms) — accepted because correctness (conflicts, renames, ahead/behind, Windows) outweighs a status read that already runs at most once per coalesced settle window, never per keystroke.
- Negative / accepted: `vcs-core` now has one more read that shells out (blame and, on a large repo, `file_history` were already the two ADR-0031 §4/§7 named) — still zero Qt/cxx-qt dependency, and still tested against a real `git` binary per this repo's stated preference (ADR-0031's own "Consequences").

## Related

- ADR-0031 (the split this amends §1 of; §2's `cli::run` wrapper, §7's measurement discipline and its `cli_spawn_status_porcelain` baseline)
- ADR-0052 (the WSL execution asymmetry this closes the last gap in)
- ADR-0003 (FFI seam: `FfiChangeKind`'s append-only additions, `FfiBranchStatus`)
