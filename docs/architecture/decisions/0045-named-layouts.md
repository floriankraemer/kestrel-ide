# 0045. Named workspace layouts

## Status

Accepted.

## Context

The IDE already reopens the way it was left.
`IdeMainWindow::closeEvent` persists the window geometry, the Qt-ADS dock blob and the editor splitter tree with the files open in each pane, and startup restores all three.

Two things were missing from that picture.

The maximized state was dropped.
`closeEvent` deliberately saves `normalGeometry()` rather than `geometry()` — a maximized window's current rect is the screen, and a minimised one's is `0x0`, so the normal rect is the only one worth restoring.
But nothing recorded that the window *was* maximized, so a maximized window reopened at the size it would have un-maximized to.

And there was exactly one layout, the implicit one.
A user who arranges the window for debugging (Debug and Run docks open, the editor unsplit) and differently for writing (Preview docked right, two panes) had to rebuild one arrangement by hand every time they wanted the other.
Every mainstream IDE offers named layouts for this: IntelliJ's "Store Current Layout as…", VS Code's profiles, Qt Creator's modes.

## Decision

### 1. A layout holds the arrangement, never the documents

`app_config::Layout` is two opaque blobs: the base64 ADS dock state, and the editor splitter tree as JSON with every group's `files` omitted.

Applying a layout rearranges the window around whatever is already open and closes nothing.
`EditorTabs::applyGrid` detaches every open tab before the panes are torn down and reattaches them into the grid's focused pane; extra panes the grid adds come up empty, which is a place to drag a file to rather than a bug.

Rejected: a layout that also captures the open file set.
It reads as the more powerful option and is in practice a destructive one — picking a layout from a menu would close the documents the user is working in, which is not what any of the products above do with the word "layout", and there is no undo for it.
The session's own file set is still persisted, in `Settings::editor_layout`, which is a different thing with a different lifetime: the last session versus an arrangement the user chose to keep.

### 2. Both layers, merged by name — a union, not an override

Layouts live in the global `settings.toml` and in a project's committed `.ide/settings.toml`, and `settings_model::scope::resolve_layouts` merges the two by name, a project entry shadowing a global one.

This is deliberately *not* a `ScopedField`.
That enum's contract is that a project overrides whole areas, because "a half-overridden section is a merge rule nobody can predict from looking at the file" — and the file is meant to be read in review.
Layouts are not a section but a named collection whose entries are each atomic, so a union keyed by name is exactly the rule a reader of the two files expects.
Folding them into `ScopedField` would instead mean a project that ships one layout replaces every layout the user has.

The consequence to accept: an empty `[layouts]` table in a project file cannot mean "this project clears your layouts", the way `Some(vec![])` does for the sparse sections.
A merge by name has no way to spell a removal, and inventing one (a tombstone entry) would buy a capability nobody asked for.

`scope.rs` also records that theme, fonts, keymap and AI providers stay out of the project layer because "a project that forces your colour scheme on you is hostile".
Layouts do not cross that line: a project *offers* an arrangement under a name, and nothing applies it until the user picks it from the menu.

### 3. The blobs stay opaque, and the view owns their format

Nothing in `app-core` models a dock area or a splitter tree, so there is no domain type to translate a layout into.
ADR-0003 permits an opaque view-owned blob for exactly this, and `Settings::window_state`/`editor_layout` already are two.
`Layout` reuses both encodings unchanged — base64 because `CDockManager::saveState()` returns bytes and the Rust field must be UTF-8, JSON because `EditorTabs` already writes it.

`saveGrid`/`applyGrid` are flags on the serializer `EditorTabs` already had (`includeFiles` going out, `allowEmptyGroups` coming back), not a second copy of the tree walk.

### 4. Restoring a dock layout and reseating homeless docks are one operation

`CDockManager::restoreState()` leaves any dock the saved state predates un-parented — closed, with no dock area.
`DockRegistry::show()` has recovered such a dock since the File History crash, but only at the moment something asks for it, which is enough at startup where nothing looks at a dock before then.

Applying a named layout restores a state over docks the user is already looking at, and there is no later call site to do the rescuing: an un-reseated dock simply disappears.
So `DockRegistry::restoreState()` now does both, and every restore goes through it.
The two steps are never correct apart, and making that structural is cheaper than a comment at each call site telling the next reader to remember.

## Consequences

- `app-config` gains `Layout` and two `layouts` maps; it still depends on no workspace crate and no Qt.
- `BTreeMap`, not `HashMap`: the project-scoped map is serialized into a committed file, and a stable key order is the difference between a readable diff and a spurious one.
- Three files crossed their ADR-0025 ceilings and were split along seams they already had: `app_config::window` (geometry and layouts), `bridge::window_state` (the session-state slots) and `bridge::layouts`.
- The menu is two stock Qt dialogs (`QInputDialog::getText` for the name, `getItem` for the layer and for deletion) rather than a management dialog of its own. A layout has a name and a layer and nothing else to edit; a custom dialog would be a window for two fields.
- The scope question is skipped when no project is open, because then the person's own layer is the only answer there is.
