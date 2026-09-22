# 0059. Generic `tool-windows` and `settings-pages` contribution points

## Status

Accepted.
Implemented by [the database-tools plan](../database-tools-plan.md)'s G1 phase, ahead of any database-specific code, so the `database`/`databaseResults` docks and the Database settings page are born on the generic path rather than migrated onto it later.
Amends [ADR-0026](0026-plugin-host.md) (two new contribution points) and [ADR-0055](0055-cli-driven-container-integration.md) by one sentence (the `containers` built-in gains a manifest, see Consequences).

**As delivered (F9 truth-up)**: `tool-windows`/`settings-pages` shipped exactly as designed and are now shared by three built-ins (`jvm-build-tools`, `containers`, `database-tools`), plus F8b's own `database-drivers`/`sql-dialects` points for driver/dialect manifests.
The one gap this ADR flagged up front — a settings page whose id has no matching `settings_model::ScopedField`, falling back to a global-only registration with a logged warning — remains unexercised: every built-in's settings page to date already has a matching `ScopedField`, so `settings_dialog.cpp` never had to build that fallback path.
The wasm limitation (no `render-tool-window` WIT export) is unchanged, deferred to a future `api_version` 2.

## Context

`plugin-api`/`plugin-host` (ADR-0026) already carry several data-shaped contribution points — `language-servers`, `analyzers`, `test-frameworks`, `previews`, `color-themes`, `build-tools` — each interpreted entirely by a Qt-free crate, with `ui-shell` only translating.
Two things every one of those built-ins still needs, though, are wired by literal id instead: which dock a feature gets, and which settings page it gets.

`main_window.cpp:383-384` constructs the `buildTools` and `containers` docks by name, in Rust-adjacent C++ that has no manifest row backing it at all; `settings_dialog.cpp:72-97` hardcodes the same two pages' labels, and `l.311-326` calls `scopedPage("buildTools")`/`scopedPage("containers")` directly rather than reading a contribution.
The practical consequence: disabling the `jvm-build-tools` or a future `containers` plugin through the Plugins page does not hide its dock or its settings page — nothing in the wiring ever asked the registry whether the plugin producing that dock was still enabled.
Database Tools needs two more docks (`database`, `databaseResults`) and one more settings page; adding a third and fourth literal-id case each would make the pattern worse rather than fix it, and building the database feature *first* on the literal path and only later migrating it, the way `jvm-build-tools` shipped before this gap was noticed, would mean writing the C++ twice.

## Decision

### 1. `tool-windows` and `settings-pages` are new, additive contribution points

`plugin-api::manifest` gains `ContributionPoint::ToolWindows(Vec<ToolWindowContribution>)` and `ContributionPoint::SettingsPages(Vec<SettingsPageContribution>)`, the same `Contributes` shape every existing point already has.

`ToolWindowContribution { id, title, area }` — `area` one of `left`/`right`/`bottom` (the ADS dock-area vocabulary the C++ layer already uses), validated as an enum in `plugin-api`, no filesystem access needed to validate it.
`SettingsPageContribution { id, title, scope }` — `scope` one of `global`/`project`, validated the same way; whether a manifest's declared scope is a scope `settings_model::scope::ScopedField` actually recognises is checked at `plugin-host` registry build, not in `plugin-api` (a manifest can be well-formed and still name a scope the host has no `ScopedField` variant for yet — a fail-soft `PluginLoadError` row, not a hard validation error, the same tolerance `plugin-host`'s existing cross-plugin checks already give a `family` that names an unknown dialect).

Both points are additive: `Contributes` flattens unknown keys (`plugin-api/src/manifest/mod.rs:322-341`), so `api_version` stays 1.

### 2. C++ reads a factory table, never a literal id

`cpp/tool_window_factories.{h,cpp}` holds one `QHash<QString, DockFactory>` populated by every built-in and installed plugin that has ever registered a factory for the ids it *may* contribute (`buildTools`, `containers`, `database`, `databaseResults` — a factory function exists whether or not the owning plugin is currently enabled).
`main_window.cpp:383-384`'s two literal calls become one loop: for each row FFI's `contributedToolWindows()` returns (which already excludes a disabled plugin's rows, since the registry never included them), look up the row's id in the factory table, call it, register the resulting dock with `DockRegistry`, and add a `view.<id>` action to the View menu.
An id with no matching factory (a plugin declaring a tool window `render-tool-window` cannot yet back, see §4) logs and is skipped — nothing crashes, nothing renders blank.

`settings_dialog.cpp` gets the identical shape: a `PageFactory` table replaces the hardcoded labels (`l.72-97`) and the two `scopedPage(...)` calls (`l.311-326`); a row absent from the FFI accessor (disabled plugin) means the page never appears in the dialog's list at all — not "there but greyed out," genuinely absent, the same as every other now-hideable feature.

Because both loops read from the registry rather than a compiled-in list, disabling `containers` or `jvm-build-tools` on the Plugins page now removes its dock, its View-menu entry, and its settings page in one place — the property ADR-0026 promised generically and these two features never actually had until this ADR.

### 3. Migration: existing built-ins gain manifest rows, no code moves

`jvm-build-tools/plugin.toml` gains a `[[contributes.tool-windows]]` row (`buildTools`) and a `[[contributes.settings-pages]]` row, matching what `main_window.cpp`/`settings_dialog.cpp` already construct for it today — the C++ factory functions themselves are unchanged, only *how they get called* changes.

`containers` did not previously have a `plugin.toml` at all (ADR-0055's C1 phase built it as a fully hard-wired feature, since the plugin host did not yet have a data shape it could contribute through).
A new `builtin/containers/plugin.toml` is added carrying **only** the `tool-windows`/`settings-pages` rows — no code moves, no existing `container-core`/`container-registry` behaviour changes, this is purely giving the feature a manifest identity so the generic loop above can see it. This is the one-sentence amendment to ADR-0055: *the `containers` built-in gains a `plugin.toml` manifest (tool-window and settings-page rows only) so it participates in the generic contribution-point wiring this ADR introduces, with no change to its CLI-driven architecture.*

Host docks that are not plugin-owned at all (`searchResults`, `problems`, `terminal`, …) stay literal — they have no plugin to disable, so a contribution row would describe a fact that can never become false.

### 4. Two more points ride the same manifest-is-data shape: `database-drivers`, `sql-dialects`

Database Tools also needs the plugin host to carry two data-only points with no dock/settings-page shape at all: `database-drivers` (`DatabaseDriverContribution { id, name, family, backend, native-id?, default-port?, url-template?, dump-tool?, icon?, adbc: {...}? }`, `backend` one of `native`/`adbc`/`odbc`) and `sql-dialects` (`SqlDialectContribution { id, name, parser, identifier-quote, param-style, keywords }`).
Both validate exactly like every existing point (id charset + per-point uniqueness, `backend`/quote/param-style enums, a url-template's placeholders restricted to `{user,host,port,database,options}`, an ADBC row's sha256/`https://`/platform fields, `native` requiring `native-id` and `adbc` requiring `manifest-name` or at least one artifact) — no filesystem access, all decidable from the manifest text alone, the same "validate here, never re-check at the call site" rule `plugin-api` already applies everywhere.
Cross-plugin consistency (a driver row's `family` naming a dialect row that actually exists) is a `plugin-host` registry-build check, not a `plugin-api` one, for the same reason `build-tools`' `requires-toolchain` join is: it needs to see every loaded manifest at once, not just its own.
These two points are grouped with `tool-windows`/`settings-pages` in this ADR because all four land in the same G1 phase and the same `plugin-api`/`plugin-host` commits, not because they share a shape with each other — `database-drivers`/`sql-dialects` are ordinary data contributions exactly like `language-servers`/`analyzers`, consumed directly by `db-core`/`db-drivers` (mapped to plain strings at the `ui-shell` seam, the same `toolchain`-string precedent `jvm-build-core` already set) rather than needing any C++ factory table at all.

### 5. Wasm limitation stated, not solved

A wasm plugin's manifest may declare a `tool-windows` row today — validation accepts it, the row appears in the registry — but nothing renders its content: no `render-tool-window` WIT export exists in the sandboxed plugin world (ADR-0028), and adding one is a new host-function surface a wasm guest could call into the Qt view through, which is a decision this ADR does not make.
A wasm-declared tool window is therefore skipped at the factory-lookup step above (no factory registered for a wasm plugin's own id) with a Plugins-page row explaining why, the same "typed error, not a crash" pattern `plugin_host::wasm`'s trap handling already uses.
That capability, when it lands, is `api_version` 2 — stated here as the known gap, deferred deliberately rather than rushed to unblock this feature.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Keep literal ids, add `database`/`databaseResults`/`Database` as a third and fourth hardcoded case | Grows the exact pattern already causing the "disabling a plugin hides nothing" bug for two more features instead of fixing it once; every future built-in tool window repeats the same C++ diff. |
| A factory *reference* inside the manifest itself (e.g. a Rust type-name string the host resolves via reflection) | Rust has no runtime reflection to resolve a string into a constructor safely; the factory table has to be compiled code somewhere regardless, so a manifest field claiming to select one would be decorative at best and a footgun at worst (a typo resolves to nothing, silently, at runtime rather than failing manifest validation). |
| Bump `api_version` to 2 for these two points | Both points are purely additive — `Contributes` already tolerates unknown keys, and no existing manifest or wasm world contract changes shape. A version bump is reserved for an actual breaking change to what a plugin may assume, which this is not. |

## Consequences

- Disabling any plugin that owns a dock or a settings page — built-in or installed — now genuinely removes it: the dock, its View-menu entry, and its settings page, in one factory-table lookup rather than three hardcoded call sites.
- `jvm-build-tools` and the new `containers` manifest both gain `tool-windows`/`settings-pages` rows in the same PR that lands this ADR's machinery, so the fix is proven against two already-shipped features before Database Tools' own rows (`database`, `databaseResults`, the Database settings page) are the third and fourth consumer rather than the first.
- `settings_model::scope::ScopedField` stays an enum, not a manifest-declared open set — a `settings-pages` row's `scope` must still name a variant that enum already has, so widening what a project may override remains the deliberate edit ADR-0022 requires, never a side effect of a plugin manifest.
- A wasm plugin may declare a tool window today with no way to render one; this is recorded as a known, deliberate gap (not a bug) until a `render-tool-window` WIT export exists, at which point it is an `api_version` 2 change, not a revision of this ADR.

## Related

- [ADR-0026: plugin host](0026-plugin-host.md) — the contribution-point and built-in-plugin shape this ADR's two new points extend; the same "manifest is data, native code interprets it" split.
- [ADR-0028: wasm plugin tier](0028-wasm-plugin-tier.md) — the typed-error-not-a-crash pattern a wasm-declared, unrenderable tool window reuses.
- [ADR-0022: per-project settings](0022-per-project-settings.md) — `ScopedField`'s enum-not-convention rule, which a `settings-pages` contribution's `scope` field must still resolve against.
- [ADR-0055: CLI-driven container integration](0055-cli-driven-container-integration.md) — amended by one sentence here: the `containers` built-in gains a manifest for its tool-window and settings-page rows, no architecture change.
- [ADR-0057: Gradle & Maven support](0057-jvm-build-tools.md) — `jvm-build-tools`'s existing hardcoded dock/settings-page wiring, migrated onto this ADR's factory tables without a behaviour change.
- `docs/architecture/database-tools-plan.md` — the plan whose G1 phase implements this ADR ahead of any database-specific contribution.
