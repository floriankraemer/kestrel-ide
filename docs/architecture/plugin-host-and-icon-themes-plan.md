# Plugin host and icon themes plan

A plugin host with two tiers — declarative contributions, and a sandboxed WebAssembly tier — and the first thing built on it: per-file-type icons, sourced from the MIT-licensed Material icon theme.

Architecture decisions: [ADR-0026](decisions/0026-plugin-host.md) (the host), [ADR-0027](decisions/0027-icon-themes.md) (icon themes), [ADR-0028](decisions/0028-wasm-plugin-tier.md) (the sandbox tier).

## Why

The IDE draws no icons.
`ProjectTreeModel::data` (`crates/ui-shell/src/bridge/tree.rs`) returns an invalid `QVariant` for `Qt::DecorationRole` on purpose, and there is no `QIcon`, no `.qrc` and no bundled asset anywhere under `crates/`.
File rows in the project tree, editor tabs and every search result list are indistinguishable at a glance.

The icons themselves are pure data, so they do not need a plugin host to arrive.
They get one anyway, because [ADR-0001](decisions/0001-core-tech-stack.md)'s hybrid plugin system has never been built and a hardcoded icon table has no incremental path to it — see ADR-0026's rejected alternatives.

## Scope decisions

- **SVG is rasterised in Rust with `resvg`, not by Qt.**
  Qt6Svg is not in either toolchain: `docker/Dockerfile` installs `qt6-base-dev` on Linux and cross-builds `qt6-qtbase` only for Windows.
  Taking the Qt route means two Dockerfile changes, a cross-built `qt6-qtsvg`, and shipping `Qt6Svg.dll` plus the `qsvg` imageformat plugin into `dist/windows/`, which today carries `platforms/qwindows.dll` and nothing else.
  Rasterising in a Qt-free crate costs one dependency tree and is unit-testable.
- **Icons appear in the project tree, editor tabs, and the Search Everywhere / Search Results / Problems lists.**
- **Upstream icons are vendored, not forked**, by a pinned and re-runnable import of the published extension package.
- **The wasm tier ships with `commands`**, a second contribution point, so it has a real consumer rather than landing as dead code.

## Progress

Living status table — update the relevant row **in the same commit** that finishes a task, so status and code never drift apart.
A fresh session should read this table (and `git log`) before picking up work, per `CLAUDE.md`.

| Task | Status | Commit |
|---|---|---|
| P0 — spike: cross-build `wasmtime` for `x86_64-pc-windows-gnu` | done | throwaway; result recorded below |
| P1 — `plugin-api`: manifest, contribution points, WIT world, ADR-0026, this plan | done | #91 |
| P2 — `plugin-host`: discovery, registry, built-ins, reload | done | #93 |
| P3 — `icon-theme`: pack model, resolver, `resvg` rasteriser, cache, ADR-0027 | done | #92 |
| P4 — Material import script + vendored pack under `third_party/` | done | #94 |
| P5 — FFI seam + project tree icons | done | #96 |
| P6 — icons in editor tabs and the search/result lists | done | #97 |
| P7 — settings: icon theme choice, disabled plugins, Plugins page | in review | #98 |
| P8 — wasm tier: runtime, capabilities, limits, `commands`, ADR-0028 | done | #95 |
| P9 — docs truth-up and the end-to-end pass | in review | #99 |

Lanes: `{P1→P2→P8}` and `{P1→P3→P4}` run in parallel and converge at P5.
P7 needs only the settings seam from P5, not P6.

### P0 result

`wasmtime` 38.0.4 cross-builds **and links** for `x86_64-pc-windows-gnu` through the MXE toolchain in `ide-windows-builder`, with the Cranelift backend, using:

```toml
wasmtime = { version = "38", default-features = false, features = ["runtime", "cranelift", "component-model", "std"] }
```

`wat` is added as a dev-only feature so tests can write modules inline.
The pure-Rust Pulley interpreter fallback is therefore **not needed**; it stays documented in ADR-0028 as the escape hatch if a future wasmtime release regresses the target, which is Tier 2 upstream and so not covered by their release gating.
Runtime behaviour was verified natively on Linux (compile, instantiate, call); the Windows binary was link-checked only, since the build host cannot execute it.

## Tasks

### P1 — `plugin-api`

The contract: `PluginManifest` (`plugin.toml`), `ContributionPoint` with one payload type per point, `LoadErrorKind`/`PluginLoadError`, and `wit/plugin.wit`.
A leaf crate — `serde` and `toml` only.
Every rule decidable without a filesystem is validated and tested here: id charset (an id is also a directory name), relative-path safety, contribution-id uniqueness, `${plugin_dir}`-scoped capabilities, `api_version` compatibility.

### P2 — `plugin-host`

Scan `<config_dir>/plugins`, skipping dot-directories.
Built-in plugins embedded in the binary and loaded through the same path, distinguished only by `PluginSource::Builtin`.
The user's `disabled_plugins` list is a filter, not a load failure.
`RwLock<Arc<PluginRegistry>>` with scan-outside-the-lock and pointer-swap reload, as `syntax_core::registry` does.
Contribution lookup by point returns payloads; the host never interprets one.
Declarative only — the wasm runtime is P8.

### P3 — `icon-theme`

`IconPack` and its `pack.toml`; `IconResolver` with the full resolution order — exact filename, then longest multi-part extension, then extension, then language id, then the default — and separate open/closed folder tables.
The resolver takes an already-resolved `language_id: Option<&str>` rather than depending on `syntax-core`, so [ADR-0018](decisions/0018-single-source-language-detection.md)'s single detection table stays single.
`resvg` rasterisation to premultiplied RGBA8, cached by `(pack, icon, px)`.
Light-theme variants are a substitution table applied after resolution.

### P4 — Material import

`scripts/import-material-icons.py`, standard library only: download the pinned `PKief.material-icon-theme` package from open-vsx, verify its SHA-256, convert the generated `material-icons.json` into `pack.toml`, and copy the SVGs.
Output is committed under `third_party/material-icon-theme/` with the upstream `LICENSE`, a `VERSION` file, and attribution in `README.md`.
Re-running the script with a new version is how the pack is updated.

### P5 — FFI seam and the project tree

`Roles::IconKey` on the tree model, and an `IconProvider` QObject exposing `iconKeyForPath(path, isDir, expanded) -> QString` and `iconPixels(key, px) -> QByteArray`.

C++ gets two humble pieces: `cpp/icon_cache.*`, memoising `QIcon`s by key and size, and `cpp/icon_decoration_proxy.*`, a `QIdentityProxyModel` that answers `Qt::DecorationRole` from the source model's `IconKey` and returns an invalid `QVariant` for an empty key.
The proxy is why the Rust model keeps its Qt-role-free `data()`: no Qt-defined role is ever added on the Rust side, and the regression test at `tree.rs` (`tree_roles_stay_out_of_the_range_qt_reserves`) stays valid.

Pixels cross as premultiplied RGBA8 and are wrapped with `QImage::Format_RGBA8888_Premultiplied`, which matches tiny-skia's byte order exactly — `Format_ARGB32_Premultiplied` is BGRA on little-endian and would need a swizzle.

### P6 — tabs and result lists

Editor tab icons on open and rename; icons in Search Everywhere, the Search Results dock and the Problems dock.
All four call `iconKeyForPath` plus `IconCache::iconFor` at the point they build a row — the proxy model is for the tree only.

Both calls are wrapped as `ui_shell::fileIcon(path, px)` over a process-wide `sharedIconCache()`, which is also what the tree's proxy now reads through.
Five call sites over one cache rather than five caches: the pixels behind a key are already memoised once on the Rust side, so a second cache would only decode the same image again, and P7's theme switch has one cache to clear rather than five.
The `IconProvider` a window used to own therefore has no callers left and is gone; the shared cache builds its own, which is equivalent because the Rust struct behind it holds nothing but handles on `bridge::registry::shared_icons`.

A row that names no file — Search Everywhere lists actions beside files — resolves to no icon at all rather than to the pack's default, which would dress an action up as a file.
That is a rule, so it is `IconService::icon_key`'s and has a unit test; the view only ever sees an empty key.

### P7 — settings

`Settings::icon_theme` and `Settings::disabled_plugins`, both global: per-project settings deliberately exclude theme-like choices (see `crates/app-config/src/project_settings.rs`).
An icon-theme combo on the Appearance page with live preview and revert-on-Cancel, the same shape as the existing theme combo.
`settings-model/src/plugins.rs` produces the rows; `PluginCatalog` and `cpp/plugins_page.*` mirror `LanguageCatalog` and `cpp/languages_page.*`.

Two things the task turned out to need that this section did not say.
The Plugins page scans with **nothing disabled** rather than reading the live registry, because `plugin_host::load` filters a disabled plugin out before it opens its manifest — a page reading the live registry could list every plugin except the ones it exists to switch back on.
`WasmTier::disabled()` also had no owner: P8 built the tier and nothing in the application ever started one, so the trap rows the page is for would always have been empty.
The tier now lives beside the registry in `plugin-host` (`start_tier`/`tier`, same `RwLock<Arc<_>>` shape), is started from the same scan the icon theme comes from, and is restarted whenever a plugin is switched off or back on.
Choosing its `HostServices` stays in `ui-shell`, because the implementation that eventually routes a plugin's `notify` to the editor is a Qt object.

### P8 — the wasm tier

`wasmtime` with fuel accounting, epoch interruption plus a watchdog, and a `StoreLimits` memory cap.
A capability-gated `Linker`: `log` always, `notify` and `workspace-root` by declaration, `read-file` restricted to prefixes granted under `${plugin_dir}`.
`contributes.commands` is a fully working mechanism — `WasmTier::invoke` calls a component's `on-command` by id — but it has no palette consumer: the palette's action list is `app_config::keymap::ACTIONS`, a static table, and nothing merges a plugin's contributed commands into it. Found while writing [the markdown preview plan](../markdown-preview-plan.md); wiring the palette up is not part of that plan's scope and remains open.
Two more points, `tool-windows` and `settings-pages`, do now have a native consumer path — the database-tools plan's G1 (amends ADR-0026 as ADR-0059): `ui-shell/cpp/tool_window_factories.cpp`'s id-keyed dock-factory table and `settings_dialog.cpp`'s equivalent for pages, both reading `AppSettings::contributedToolWindows`/`contributedSettingsPages`.
A wasm plugin may still only *declare* a tool window; it is logged and skipped until a `render-tool-window` WIT export exists, which is the same open palette-consumer question `commands` raises above, not a new one.
A worked example plugin lives under `crates/plugin-host/examples/` (`hello-plugin`, exercising `on-command`; `preview-plugin`, exercising the preview world's `render` export added in ADR-0033).
A trap disables the plugin with a typed error on the Plugins page; it never takes the process down, which is the whole reason for choosing a sandbox over the `dlopen` tier.

### P9 — docs and the end-to-end pass

`overview.md` and `layering.md` brought back to truth, and the end-to-end run below.

The pass found one seam neither P6 nor P7 owned: a tab keeps the `QIcon` it was given when it opened, so an icon-theme or colour-theme switch left every open tab on the old art.
The tree repaints because clearing the shared cache invalidates the proxy, and the result lists repaint because they rebuild their rows; tabs do neither.
`EditorTabs::refreshTabIcons` closes it through the same `renderTabText` a rename already uses, so there is one place that renders a tab rather than two.

## Verification

```sh
make test
make lint
cargo tree -p plugin-api  -e normal | grep -i qt   # must be empty
cargo tree -p plugin-host -e normal | grep -i qt   # must be empty
cargo tree -p icon-theme  -e normal | grep -i qt   # must be empty
```

Unit coverage lives in the Qt-free crates:

- `plugin-api` — a newer `api_version` is refused whole; an id or path that could climb out of the plugins directory is rejected; a capability path outside `${plugin_dir}` is rejected; commands without a component are rejected; an unknown key is a parse error rather than a silent drop.
- `plugin-host` — a good manifest loads; a bad one is skipped with the rest unaffected; a disabled id is filtered; reload swaps the registry while an `Arc` taken before the swap stays usable.
- `icon-theme` — resolution order (filename beats `spec.ts` beats `ts` beats language id beats default), folder open/closed variants, light-variant substitution, unknown icon falls back to the default, rasteriser output dimensions and non-empty alpha.
- `plugin-host` (wasm) — fuel exhaustion traps, the epoch deadline traps a spin loop, a `read-file` outside the grant is denied, and a trapping plugin is disabled rather than fatal.
- `settings-model` — plugin rows merge built-in, installed, failed and disabled, with an installed plugin shadowing a built-in of the same id.

End to end, through the headless harness (Xvfb + xdotool in `linux-builder`, [ADR-0024](decisions/0024-verification-foundation.md)):

1. Launch the real binary and open this repository as the project.
2. Screenshot the project tree: `Cargo.toml`, `*.rs`, `docs/` and `crates/` must show distinct icons, and an expanded folder must show its open variant.
3. Open a `.rs` and a `.md` file; the tab icons differ.
4. Open Search Everywhere, type `tree`; result rows carry icons.
5. Settings → Appearance: switching the icon theme repaints the tree live, and Cancel restores the previous one.
6. Settings → Plugins: the Material plugin is listed as built-in, and disabling it falls back to no icons without an error dialog.
7. Switch to a light theme: the light icon variants swap in.

Cross-platform gate: the Windows bundle still builds and runs from `dist/windows/`, with no DLL added beyond what P0 concluded (none).
