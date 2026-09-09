# Changes tab & Git feature revision

## Context

The Changes dock (`crates/ui-shell/cpp/changes_panel.cpp`) shipped as F3-17 and had not been revisited since.
Measured against Visual Studio's Git Changes window and JetBrains' Commit tool window, it was missing most of the surface a user expects.

- No toolbar at all: no Refresh, no Fetch/Pull/Push, no branch indicator, no ahead/behind counts, no bulk stage/unstage.
- No per-row context menu: double-click opened a diff, a checkbox staged — nothing else.
- Status was a full word ("Modified", "Type Changed") in a stretched column, where both reference IDEs use a single letter.
- The state it showed was wrong, and on Windows visibly so: `Repository::status` was an in-process `gix` walk that dropped merge conflicts on the floor, reported renames as untracked additions, never reported ahead/behind, and — on a `\\wsl.localhost\...` project — ran Windows-side over the 9P share while `git add`/`git commit` ran inside the distro, so a genuinely modified file could show as clean.

The intended outcome: a Changes dock that reads like VS/JetBrains — branch plus ahead/behind, remote buttons, letter status codes, a real context menu — sitting on a status source that agrees with `git status` in a terminal on every platform, WSL included.

Design decisions taken with the user before implementation: git/VS Code standard status letters; a letter column plus filename plus dimmed directory; the full toolbar; and switching the status read to the `git` CLI.
[ADR-0053](decisions/0053-git-status-via-porcelain-v2.md) records the status-source decision in full; this doc tracks delivery.

## The mockup

`.ide/changes-panel-mockup.html` renders both states below as committed HTML, the way `.ide/git-history-mockup.html` already does for the File History dock.

### Dock, normal state

```
┌ Changes ─────────────────────────────────────────────────────────────────┐
│ ⎇ main ↓2 ↑1 │ ⟳  ⇩ Fetch  ⇩ Pull 2  ⇧ Push 1 │ ✓ Stage all  ✗ Unstage all│
├──────────────────────────────────────────────────────────────────────────┤
│ ▾ Staged Changes (2)                                                     │
│    M   bridge.rs               crates/ui-shell/src                       │
│    A   status.rs               crates/vcs-core/src                       │
│ ▾ Unstaged Changes (3)                                                   │
│    M   main_window.cpp         crates/ui-shell/cpp                       │
│    R   changes_panel.cpp       crates/ui-shell/cpp   ← was: changes.cpp   │
│    D   old_notes.md            docs                                      │
│ ▾ Untracked Files (1)                                                    │
│    U   scratch.md              docs                                      │
├──────────────────────────────────────────────────────────────────────────┤
│ Commit message                                                           │
│                                                                          │
├──────────────────────────────────────────────────────────────────────────┤
│ [ Commit ]  [ Commit and Push ]  [ Amend ]                               │
└──────────────────────────────────────────────────────────────────────────┘
```

### Dock, mid-merge

```
│ ⎇ feature/x ↓0 ↑3 │ ⟳  ⇩ Fetch  ⇩ Pull  ⇧ Push 3 │ ✓  ✗                  │
├──────────────────────────────────────────────────────────────────────────┤
│ ▾ Merge Conflicts (1)          ← group shown only when non-empty, listed  │
│    C   parser.rs               crates/syntax-core/src        first        │
│ ▾ Staged Changes (1)                                                     │
│    M   lib.rs                  crates/syntax-core/src                    │
```

A conflicted row has no checkbox: staging a conflict is not a thing the panel offers.

### Status letters (git / VS Code convention)

| Letter | Meaning | Colour token (`dark.toml`) |
|---|---|---|
| `M` | Modified | `diff.modified_marker` |
| `A` | Added | `diff.added_marker` |
| `D` | Deleted | `diff.deleted_marker` |
| `R` | Renamed | `diff.modified_marker` |
| `C` | Conflict | `semantic.error`, bold |
| `U` | Untracked | `diff.added_marker` |
| `T` | Type changed | `diff.modified_marker` |

## Approach

Two halves: a correct status source in `vcs-core`, then the panel rebuilt on top of it.
The backend half fixes Windows; the UI half is what the user sees.

**A. Status comes from `git status --porcelain=v2` (backend).**
`Repository::status` shells out through the CLI wrapper ADR-0031 §2 already established for writes, rather than walking `gix`'s own status object.
A new pure parser, `crates/vcs-core/src/status.rs::parse_porcelain_v2`, reads branch/upstream/ahead-behind, ordinary changes, renames with their original path, and unmerged (conflicted) entries from one command's output.
See ADR-0053 for the full reasoning and the measured cost.

**B. Windows path matching.**
`to_repo_relative` and `file_status` compared a Qt path against `gix`'s `work_dir()` with a case-sensitive, exact-spelling `strip_prefix`; on Windows the two disagree on drive-letter case and UNC spelling.
Fixed once, in one shared helper both call sites use: normalise separators, compare case-insensitively on Windows only.

**C. The FFI seam (ADR-0003).**
`FfiChangeKind` gains `Renamed`, `Copied`, `Conflicted` (append-only).
`FfiChangedFile` gains `orig_path`.
A new `FfiBranchStatus` (`branch`, `upstream`, `ahead`, `behind`, `has_upstream`, `detached`) plus `branchStatus()` on `VcsService`, refreshed by the existing `statusChanged` signal.
New invokables `stageAll`/`unstageAll`, following `stage_file`'s existing shape.

**D. The panel.**
`changes_toolbar.{h,cpp}` split out of `changes_panel.{h,cpp}` (the `diff_toolbar.{h,cpp}` precedent): a branch chip, Refresh/Fetch/Pull/Push, Stage all/Unstage all.
The tree gains a fixed-width letter column, a dimmed directory after the filename, `(n)` group counts, and a Merge Conflicts group shown first and hidden when empty like the other three.
A shared Discard-changes confirm dialog is used by both the dock's row menu and the project tree's own Git submenu, rather than a second copy of the wording.

**E. E2E instrumentation.**
`changes_row` gains a `status` field carrying the letter.
A new `changes_toolbar_shown` marker reports the toolbar's Refresh/Pull/Push rects and the live ahead/behind counts.
New tests in `crates/app/tests/e2e_vcs.rs` cover a staged rename, a conflicted file, and the ahead count moving after a commit.

**F. Docs.**
This plan doc, ADR-0053, the `docs/README.md` index lines, and `.ide/changes-panel-mockup.html`.

## Progress

| # | Task | Status | Commit |
|---|---|---|---|
| G1 | `status.rs`: `parse_porcelain_v2` + extended `ChangeKind`/`FileStatus`/`RepoStatus` | done | 1dbe825 |
| G2 | `repo.rs::status` switched to `cli::run`; `gix` status helpers deleted; real-git tests for conflict, rename, delete, staged+unstaged, ignored-not-shown | done | 1dbe825 |
| G3 | `staging.rs`: `stage_all`/`unstage_all` | done | 3f69c18 |
| G4 | Shared repo-relative path helper; Windows-tolerant comparison in `to_repo_relative` + `file_status` | done | 6394eb7 |
| G5 | FFI: `FfiChangeKind` additions, `orig_path`, `FfiBranchStatus`, `branchStatus`, `stageAll`/`unstageAll` | done | 6394eb7 |
| G6 | `changes_toolbar.{h,cpp}` + wiring; `showBranchMenu` promoted to `vcs_menu.h` | done | 86d21c9 |
| G7 | Panel: letter column, name + dim directory, group counts, Merge Conflicts group, rename suffix | done | 674f0b2 |
| G8 | Row context menu; Discard confirm dialog extracted and shared with the project tree | done | 674f0b2 |
| G9 | E2E markers + three new tests in `e2e_vcs.rs` | done | 830ef01 |
| G10 | ADR-0053, this plan doc, `docs/README.md`, `.ide/changes-panel-mockup.html` | done | a939fa2 |
| G11 | Windows/WSL manual pass, recorded in this plan doc | **open — awaits a manual Windows/WSL session.** The implementing sessions ran in a Linux-only Docker container with no Windows machine or real `\\wsl.localhost\...` project available. Every backend and UI change was verified on Linux CI (unit tests against a real `git` binary, and the E2E suite under Xvfb); the manual check below needs a real Windows/WSL box, the same `remote-wsl-plan.md` W8-2 precedent. | |

G10's own commit hash could not be written into this table by that commit (it would have to name itself before it exists), so it was recorded by this small follow-up commit immediately after — `remote-wsl-plan.md`'s own W8-1 precedent for the same problem.

## Verification

```sh
cargo test --workspace
cargo tree -p vcs-core -e normal | grep -i qt   # must stay empty
make lint
make coverage      # patch coverage must be >= 80%
```

E2E (needs Xvfb, `make e2e`): `e2e_vcs.rs` — `e2e_stage_and_commit_through_the_changes_dock` and `e2e_an_external_change_reaches_the_changes_dock` pass with the new letter column in place, alongside the three new G9 tests.

Manual, on Linux: open this repo, make a conflict with a merge, confirm the Merge Conflicts group appears and Pull/Push counts move after a commit and a fetch.

Manual, on Windows (G11): open the project from `\\wsl.localhost\Ubuntu\...`, and compare the dock line-for-line against `git status` run in a WSL shell — modified, staged, renamed, untracked and conflicted each represented.
This is the check the whole backend change exists to pass.
