# Implementation plan: project scope (explicit exclusions, not `.gitignore`)

## Status

In delivery.
Design recorded in [ADR-0064](decisions/0064-project-scope-explicit-exclusions.md); supersedes ADR-0063.

## Progress

| Task | Status | Commit |
|---|---|---|
| T1 | done | (this PR) |
| T2 | open | |
| T3 | open | |
| T4 | open | |
| T5 | open | |
| T6 | open | |
| T7 | open | |

## Context

Search Everywhere could not find files in a workspace's nested repositories because every project walk honoured `.gitignore`, `.ignore`, git's exclude files and a blanket dotfile rule, none of which the IDE showed.
PR #333 patched the nested-repository case; this plan replaces the implicit rules with the JetBrains model: `.gitignore` only affects version control, and what the IDE skips is a per-project **Excluded** folder list plus a global **Ignored names** list, both visible and editable.
See ADR-0064 for the decision and its consequences.

## Task breakdown

| Task | Scope | Verification |
|---|---|---|
| T1 | This plan, ADR-0064, `docs/README.md` index | Docs review |
| T2 | `project-model`: `ProjectScope` (one `ignore::gitignore::Gitignore` matcher: root-anchored excluded entries + basename ignored names; `is_excluded` checks ancestors; `walk` = `WalkBuilder` with `standard_filters(false)` + pruning `filter_entry`). `app-config`: project `excluded` (alias `index_exclude`) + `reviewed_not_excluded`, global `ignored_names` (alias `index_excludes`) with the ADR's defaults; `toggle_excluded`. `settings-model`: `ScopedField::Excluded`/`IgnoredNames` replace `IndexExcludes` | `cargo test -p project-model -p app-config -p settings-model`: anchored/basename/ancestor matching, `.gitignore` has no effect, dotfiles in scope, alias migration, toggle add/remove idempotent |
| T3 | Every walk through the scope: `index-core` build (`IndexOptions` carries the scope; `excludes.rs` folded in), `sync_paths` re-checks the scope, purge of newly excluded files on reopen; watcher initial set and new-directory check (`root_gitignore_matcher`/`is_ignored` removed); `walk_all_entries`; `ai-chat-core` `expand_folder`. ADR-0063's `walk_project` nested search deleted. `layering.md`/`overview.md` updated | `cargo test -p index-core -p project-model -p ai-chat-core`: gitignored file indexed, dotfile indexed, excluded folder and ignored name skipped, `sync_paths` refuses an excluded path, rescope purges; existing E2E `e2e_search_everywhere_*` still pass |
| T4 | Rescope on settings change (index delta reopen + watcher restart); project tree "Mark Directory as Excluded" / "Cancel Exclusion" for folders; excluded folders styled with a themed colour via an `IsExcluded` role | E2E: marking a folder excluded drops its file from Search Everywhere, cancelling brings it back |
| T5 | "Project Scope" settings page: editable Excluded folders (project) and Ignored names (global) lists, reset to defaults, a note stating the content rules; all strings `tr()` | E2E: editing the list through the page rescopes; screenshot review |
| T6 | "Found N ignored but not excluded folders" notification on project open: candidates from `vcs-core` `Repo::ignored_paths()` per repository, filtered by a Qt-free rule (not excluded, not reviewed, folders only; a folder holding a nested `.git` starts unchecked); review dialog with a checkbox per folder persisting `excluded`/`reviewed_not_excluded` | Unit tests for the candidate rule; E2E: accepting the dialog excludes the checked folder and is not offered again |
| T7 | VCS "ignored" colour for gitignored entries in the project tree (`ChangeKind`/theme colour, `VcsStatusColorProxy`) | Unit test for the status mapping; E2E screenshot review |
