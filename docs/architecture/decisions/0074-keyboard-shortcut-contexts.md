# 0074. Keyboard shortcut contexts

## Status

Accepted.
Delivered with task E7 of the [PHPUnit editor runs plan](../phpunit-editor-runs-plan.md).
Refines the keymap of [ADR-0022](0022-per-project-settings.md)'s settings: the action catalogue in `app_config::keymap`.

## Context

Every action in `app_config::keymap::ACTIONS` had to own its shortcut alone: `Keymap::conflicts` compared every action with every other one, and a test kept the shipped defaults unique.
That rule matched how most actions are registered, as window-wide menu actions, but not how JetBrains IDEs bind keys.
In PhpStorm one key does different things depending on where focus is.
`Ctrl+Shift+T` is Go to Test in the editor and New Terminal Tab in the terminal; `F4` is Jump to Source in the Tests tree and Go to Referenced Row in a data grid.
Under the global rule, Go to Test could not have its JetBrains key, and the Tests tree's Jump to Source shipped unbound.

Qt allows one key on several shortcuts only when at most one of them is active at a time.
A `Qt::WindowShortcut` is active wherever focus is in the window; a `Qt::WidgetWithChildrenShortcut` only while focus is inside its widget.
When two active shortcuts share a key, Qt reports it as ambiguous and fires neither.
So a window-wide shortcut and a widget-scoped shortcut on the same key clash whenever focus is in that widget.
Two widget-scoped shortcuts on disjoint widget trees never clash.

The app already had widget-scoped actions: the terminal's Copy, Paste, New Terminal Tab and Select Shell, and the Tests tree's key filter.
The keymap did not know about them, so it could neither allow the sharing they make safe nor warn when a rebind breaks it.

## Decision

Each action has a shortcut context, `app_config::ShortcutContext`: `Global`, `Editor`, `Terminal`, `TestsTree` or `DataGrid`.
`Global` is the default; the few scoped actions are listed in one table, `SCOPED_ACTIONS`, beside the enum in `keymap/context.rs`.
Two contexts overlap when they are equal or either is `Global`.
`Keymap::conflicts`, the "default taken by an override" rule of `Keymap::shortcut_for`, and therefore the Keymap settings page's warning and `assign`'s steal, consider only actions in overlapping contexts.
The shipped-defaults test applies the same rule.

The view registers a scoped action on its owning widget and never window-wide, through one helper in `keymap_page.h`:
`scopeShortcut` gives a `QAction` `Qt::WidgetShortcut` or `Qt::WidgetWithChildrenShortcut` on its owner (the editor area, a terminal, the terminal dock, a result grid) and has no window-wide reach to pass, and `scopedShortcutKey` serves the Tests tree's key filter, which claims the key on `ShortcutOverride`.
The test `every_scoped_action_is_registered_widget_scoped` scans `ui-shell/cpp` for the two helpers' calls and holds the ids they name equal to `SCOPED_ACTIONS` (#423).
A scoped action may still appear in a menu; Qt only matches a menu-associated shortcut while focus is in the menu bar, so the widget association is what makes the key live.

With the rule in place, `navigate.goToTest` (Editor) and `terminal.newSession` (Terminal) share `Ctrl+Shift+T`, and `tests.jumpToSource` (TestsTree) and `database.goToReferencedRow` (DataGrid) share `F4`.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| Keep global uniqueness and pick non-JetBrains keys | The reason for the change: users coming from PhpStorm expect `Ctrl+Shift+T` and `F4` where they press them. |
| A context field on every `ActionDef` literal | 161 identical `Global` lines for seven scoped actions; the table keeps the catalogue readable and a test checks every listed id exists. |
| Let the view decide contexts without telling the keymap | The settings page would warn about harmless rebinds and stay silent about ambiguous ones, because the conflict rule lives in Rust. |
| A nesting model (a context inside another, such as a grid inside the editor) | No scoped widget is nested inside another today; `overlaps` is the one place to add it when one is. |
| Register scoped actions window-wide and enable them on focus change | Fragile: every focus path must keep the enabled state right, and Qt still sees two enabled shortcuts during the switch. |

## Consequences

- JetBrains default keys can be used where PhpStorm uses them, without Qt ambiguity.
- A user rebind only warns about, and only steals from, actions that can actually be active at the same time.
- The catalogue's context and the view's registration must agree; a new scoped action needs both the table entry and a `scopeShortcut` (or `scopedShortcutKey`) call, and the scanning test fails until it has both.
- A scoped action's shortcut does nothing when focus is elsewhere, so the menu bar entry is the way to reach it from another widget (Go to Test from the terminal, for instance).

## Related

- [PHPUnit editor runs plan](../phpunit-editor-runs-plan.md), task E7.
- [ADR-0022](0022-per-project-settings.md): where keymap overrides are persisted.
