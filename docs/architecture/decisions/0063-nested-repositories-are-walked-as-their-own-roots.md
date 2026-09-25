# 0063. Nested repositories are walked as their own roots

## Status

Accepted.
Amends [ADR-0008](0008-project-index.md) and [ADR-0051](0051-ignore-aware-project-watcher.md): both walks stay gitignore-aware, but an outer `.gitignore` no longer hides a nested repository.

## Context

A common workspace layout checks several repositories out side by side inside one outer repository and lists them in the outer `.gitignore` (`/projects/`).
That line only keeps them out of the *outer* repository — git itself treats a nested `.git` as a boundary.
Every project walk here (`index-core`'s index build, `project-model`'s watcher, and `walk_all_entries` behind MCP's tree tool) used a plain `ignore::WalkBuilder`, which read the line as "not part of the project" and never descended.
Search Everywhere then could not find a single file of any nested repository, not even by its exact name, and edits there never reached the index because the watcher did not watch them either.

## Decision

All three walks go through `project_model::walk_project`.
It walks the root as before, then looks for directories holding a `.git` up to three levels below the root with gitignore rules switched off (they are what hides these directories), and walks each one the first walk did not reach as a root of its own, with the same configuration.
Inside a nested repository its own `.gitignore` applies as usual, so its `vendor/` or `node_modules/` stay out.
The caller's overrides (the index's exclude settings) and `filter_entry` also bound the search, so a directory a user explicitly excludes keeps its repositories out; `node_modules` is never searched.
`index-core` gains a dependency on `project-model` (support on domain, the same direction as its existing `editor-core` edge) so the rule exists once.

## Consequences

- Files and symbols of nested repositories are indexed, watched, and listed; a real four-repository workspace went from its ~40 outer files to ~9,200 indexed in under two seconds.
- A repository nested deeper than three levels inside an ignored directory stays unindexed; the depth is a constant, to become a setting only if someone needs it.
- An ignored directory that contains no repository is still skipped, so `target/` and friends keep the watch budget ADR-0051 protects.
