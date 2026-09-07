# Window state restore gaps and named layouts

The window comes back maximized when it was left maximized, and a user can keep several named workspace arrangements and switch between them.

Architecture decision: [ADR-0045](decisions/0045-named-layouts.md).
Builds on [ADR-0022](decisions/0022-per-project-settings.md), whose global/project layering the layouts store reuses, and [ADR-0005](decisions/0005-ads-build-integration.md), whose dock manager supplies the state blob.

## Why

Session restore was already there and already worked.
`IdeMainWindow::closeEvent` persisted the window geometry, the Qt-ADS dock blob and the editor splitter tree with the files open in each pane; startup restored all three.

Two gaps sat inside that.

The maximized state was dropped.
`closeEvent` saves `normalGeometry()` on purpose — a maximized window's current rect is the screen and a minimised one's is `0x0`, so the normal rect is the only one worth keeping — but nothing recorded that the window had been maximized, so it reopened at the size it would have un-maximized to.

And there was exactly one layout, the implicit one.
Arranging the window for debugging and again for writing meant rebuilding one arrangement by hand every time the other was wanted.

## Scope decisions

See [ADR-0045](decisions/0045-named-layouts.md) for the reasoning behind each.

- A layout is the dock state plus the editor split grid and never the open files: applying one rearranges the window around what is open and closes nothing.
- Layouts live in both settings layers and merge **by name**, a project entry shadowing a global one — a union, not a `ScopedField` override, because each entry is atomic.
- `saveGrid`/`applyGrid` are flags on the serializer `EditorTabs` already had, not a second tree walk.
- `DockRegistry::restoreState()` folds the ADS restore and the homeless-dock reseat into one operation, because they are never correct apart.
- The menu is two stock Qt dialogs; the layer question is skipped when no project is open.
- No new crate and no new dependency: `app-config` still imports no workspace crate and no Qt.

## Progress

Living status table — update the relevant row **in the same commit** that finishes a task, so status and code never drift apart.

| Task | Status | Commit |
|---|---|---|
| W1 — `Settings::window_maximized`, the seam pair, `showRestored()` beside the `closeEvent` that writes it | done | fdd8064 |
| W2 — `app_config::Layout`, the two `layouts` maps, `app_config::window` split out | done | fdd8064 |
| W3 — `settings_model::scope::resolve_layouts`: merge by name, project shadows global | done | fdd8064 |
| W4 — `EditorTabs::saveGrid`/`applyGrid`, `DockRegistry::restoreState`, the reseat fold | done | fdd8064 |
| W5 — the four `AppSettings` layout slots, `bridge::layouts`, the View > Layouts menu | done | fdd8064 |
| W6 — E2E: maximized and dock-open state survive a restart; ADR, plan and docs index | done | (this commit) |
