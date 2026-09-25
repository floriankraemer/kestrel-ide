# 0064. Project scope: explicit exclusions, not `.gitignore`

## Status

Accepted.
Supersedes [ADR-0063](0063-nested-repositories-are-walked-as-their-own-roots.md).
Amends [ADR-0008](0008-project-index.md) (what the index walks) and [ADR-0051](0051-ignore-aware-project-watcher.md) (what the watcher watches).
Delivered by the [project scope plan](../project-scope-plan.md).

## Context

The index, the watcher, the MCP tree walk and the AI chat folder expansion all walked the project with `ignore::WalkBuilder`'s defaults.
What a user could find therefore depended on `.gitignore` files at every level, `.ignore` files, `.git/info/exclude`, the global git excludes file, and a blanket "skip every dotfile" rule — none of which the IDE showed anywhere.
A workspace listing its nested repositories in its own `.gitignore` lost them entirely; ADR-0063 patched that one case with a depth-bounded search for nested `.git` directories, which is one more implicit rule.
`.github/workflows/ci.yml`, `.env.example` and every other dotfile were unsearchable.

JetBrains IDEs separate the two concerns.
`.gitignore` only affects version control: an ignored file is shown in an "ignored" colour and left out of commits, but it is indexed and searchable.
What the IDE skips is an explicit, visible list: folders marked *Excluded* per project, and a global *Ignored Files and Folders* name list (`.git;.hg;.svn;CVS;__pycache__;*.pyc;…`).
Since 2019.3 the IDE notices gitignored folders that are not excluded and offers to exclude them, but only on the user's click.

## Decision

One Qt-free rule, `project_model::ProjectScope`, decides whether a path is part of the project, and every project walk goes through it.

- **Excluded** (per project, `.ide/settings.toml` key `excluded`, gitignore syntax anchored at the project root): folders the user marked "Mark Directory as Excluded".
  The old per-project `index_exclude` key is read as an alias.
- **Ignored names** (global, `ignored_names`, gitignore syntax matched against the entry name anywhere): defaults to `.git .hg .svn CVS _svn .DS_Store __pycache__ *.pyc *.pyo *.rbc *.yarb *~ vssver.scc vssver2.scc .ide-index node_modules target`.
  The old global `index_excludes` key is read as an alias; a user who had set it keeps exactly their list, without the new defaults.
- A path is out of scope when it, or any of its ancestors below the root, matches either list.
- `.gitignore`, `.ignore`, git's exclude files and the hidden-file rule no longer affect what is indexed, watched or listed.
  Dotfiles are in scope. `vendor/` is in scope.
- The watcher watches exactly the in-scope directories, so an indexed file never goes stale; the default name list is what keeps `target/` and `node_modules/` off the OS watch budget ADR-0051 protects.
- The index re-checks the scope for every watcher-reported path, and a scope change rescopes the open project: a delta walk that indexes newly in-scope files and purges newly excluded ones, plus a watcher restart.
- On project open, gitignored folders (from `git status --ignored`, per repository) that are neither excluded nor already reviewed are offered in a "Found N ignored but not excluded folders" notification.
  The user's per-folder choice is persisted: checked folders join `excluded`, unchecked ones join `reviewed_not_excluded` so they are not offered again.
  Indexing does not wait for the answer.
- `.gitignore` stays visible: gitignored entries get a VCS "ignored" colour in the project tree, and excluded folders their own colour.
- The only remaining implicit rules are about content, not location, and are stated on the settings page: a file with a NUL byte in its first 8 KiB or that is not UTF-8 is not indexed, and a file over 2 MiB is found by name only.

## Consequences

- What is searchable is exactly what the two lists say; both are editable on the Project Scope settings page and from the project tree.
- ADR-0063's nested-repository search is deleted: a nested repository is simply part of the project unless excluded.
- First open of a project with a large gitignored build folder not covered by the default names (`dist/`, `build/`, `.gradle/`) indexes and watches it until the user accepts the notification; binary files are dropped by the content sniff and large ones by the size cap, which bounds the cost.
- `target` and `node_modules` as default names also skip a legitimately named source folder; the user removes the name from the list.
