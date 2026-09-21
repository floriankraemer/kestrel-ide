# 0026. A plugin host: declarative contributions, with a sandboxed executable tier

## Status

Proposed.
Implemented by [the plugin host and icon themes plan](../plugin-host-and-icon-themes-plan.md); this ADR covers tasks P1 and P2 (`plugin-api`, `plugin-host`).
Makes concrete the "hybrid plugin system" direction of [ADR-0001](0001-core-tech-stack.md), whose open questions about the host API surface this decision answers for the first revision.
The sandbox tier's own limits are [ADR-0028](0028-wasm-plugin-tier.md); the first contribution point to use this host is [ADR-0027](0027-icon-themes.md).
Amended by ADR-0059: two more contribution points, `tool-windows` and `settings-pages` (the database-tools plan's G1), so a plugin can declare its own dock and settings page instead of the host wiring them by literal id — additive, so `api_version` stays 1.

## Context

ADR-0001 named a hybrid plugin system — native dylibs over a stable C ABI for trusted, performance-critical extensions, and sandboxed WebAssembly for third-party ones — and left the host API surface open.
Nothing was built.
The only thing resembling it in the tree is `syntax_core::runtime`, which loads user-supplied language packs from `<config_dir>/languages/<dir>/language.toml` and, for a foreign grammar, `dlopen`s a shared library.
That mechanism works and its shape is well-argued, but it is specific to grammars: nothing else can use it, and a second kind of user-installable content would have copied it.

The immediate need is icon themes.
The IDE draws no icons at all today — `ProjectTreeModel::data` explicitly returns an invalid `QVariant` for `Qt::DecorationRole` — and the intended source is the Material icon theme: a mapping table plus 904 SVGs.
That is pure data and needs no code execution whatsoever.

Two ways to get it in:

1. A hardcoded table plus embedded assets, with the extension question deferred.
2. A general host, with icon themes as its first contribution point.

The second was chosen deliberately, because the first has no path to the second that does not throw the first away.
It does mean the executable tier lands without an icon-theme consumer; the answer to that is a second contribution point, `commands`, so the tier ships with something that actually exercises it (ADR-0028).

The important observation is that these are two *different* extension mechanisms wearing one name.
A plugin that contributes data needs discovery, a manifest, validation, versioning and a settings page.
A plugin that contributes behaviour needs all of that **plus** a runtime, a capability model and resource limits.
Conflating them would force every icon theme through a WebAssembly runtime it has no use for.

## Decision

Two Qt-free crates, split along the line between the contract and the machinery.

**`plugin-api`** — the contract, and a leaf: `serde` and `toml`, nothing else.
It names neither `plugin-host` nor any consumer of a contribution, because a contract that depends on one of its parties is not a contract.
It holds `plugin.toml` (`PluginManifest`), the contribution-point vocabulary (`ContributionPoint` and one payload type per point), the typed rejection reasons (`LoadErrorKind`, `PluginLoadError`), and `wit/plugin.wit`, the WebAssembly component world.
Every rule decidable without a filesystem is validated here and unit-tested here: the id charset, path safety, contribution-id uniqueness, capability scoping, and API-version compatibility.
`PluginManifest::from_toml_str` parses and validates in one step and the two are not separable from outside, so a `PluginManifest` that exists has been validated.

**`plugin-host`** — discovery and lifecycle: scanning `<config_dir>/plugins`, the built-in plugins embedded in the binary, the user's disabled list, the live registry, capability grants, and (per ADR-0028) the wasm runtime.
It keeps a `RwLock<Arc<PluginRegistry>>` and reloads by building the next registry outside the lock and swapping the pointer, exactly as `syntax_core::registry` does — live consumers keep the `Arc` they already hold.

Three rules carry most of the weight:

- **A contribution is data.** The registry stores payloads by point; it does not know what an icon theme *is*. `icon-theme` reads `IconThemeContribution` and does not depend on `plugin-host`; the two are joined in `app-core`.
- **Fail-soft, always.** One bad plugin is skipped, its reason recorded as a typed `PluginLoadError`, and the rest load. The Settings page renders that list, the same way the Languages page renders `syntax_core::runtime`'s.
- **A built-in is a plugin.** The bundled Material icon theme ships as an embedded plugin directory with a real `plugin.toml`, loaded through the same code path as an installed one and distinguished only by `PluginSource::Builtin`. There is no privileged path for first-party content, so the path third parties use is the one that is exercised on every launch.

`api_version` is the single compatibility lever.
An older manifest keeps working, because a revision may only add optional fields.
A newer one is refused whole rather than understood in part — the alternative is silently dropping the fields that carry the meaning.
A contribution point an older host does not recognise is likewise not an error: it is ignored, which is what lets a new point ship without a version bump.

Path safety is enforced at the contract, not at each use.
A plugin id must match `[a-z0-9][a-z0-9._-]*` and be at most 64 characters, because an id is also a directory name; every path a manifest names must be relative and free of `..`; and a capability path must begin with `${plugin_dir}`, which is the whole grammar in revision 1 — reads outside a plugin's own directory cannot be expressed.

## Consequences

- The layering table gains two rows; `plugin-api` depends on nothing but `serde`/`toml`, and `plugin-host` on `plugin-api` plus its runtime.
- Icon themes, and every later kind of user-installable content, get discovery, validation, versioning, enable/disable and a settings page for free.
- The Plugins page is `settings-model`'s Languages page with the nouns changed — the same `Source`/`Status`/`Action`/`Problem` vocabulary, which is a deliberate reuse and not a coincidence.
- The manifest is a public contract from its first release. Widening it is cheap; narrowing it costs an `api_version` bump.
- `syntax_core::runtime` keeps its own manifest format. Folding language packs into plugins is possible later and is not attempted here: it would make an unrelated, already-shipped feature part of this change's blast radius.

### Amendment (P5): `app-core` gains three dependencies

This decision says a contribution is data, that the host never interprets one, and that the host and its consumers are joined in `app-core` rather than wired to each other.
P5 made that join real, and it cost `app-core` three new dependencies: `plugin-host` (the registry and `LoadedPlugin::read_asset`), `icon-theme` (the pack and the renderer), and `syntax-core` (the language id `IconPack::file_icon` is handed, per [ADR-0018](0018-single-source-language-detection.md)).
`app-core`'s row in [the layering table](../layering.md) previously listed only `editor-core` and `project-model`, so this is a real widening of the application layer and is recorded rather than assumed.

It is the price of the decision above, not a departure from it: the alternative is `icon-theme` depending on `plugin-host` and on `syntax-core`, which is exactly what this ADR and ADR-0027 refuse.
All three are Qt-free, so `app-core`'s no-Qt rule is untouched, and CI's layering gate (`cargo tree -p app-core -e normal | grep -i qt`) is what enforces that rather than the claim.
The join lives in one module, `app_core::icons`, and nothing outside it knows an `icon-themes` contribution names a pack file.

### Amendment (P7): `settings-model` gains the two plugin crates, and the wasm tier gains an owner

The Plugins page promised above is `settings_model::plugins`, and building it cost that crate `plugin-api` and `plugin-host` in its dependency row.
That is the same shape `settings_model::languages` already has with `syntax-core`'s runtime and rests on the same argument as [ADR-0017](0017-settings-model-crate.md): the page's whole value is that it never prints a Rust error, so the mapping from a typed failure to a sentence is a rule with tests, not view code.
Both crates are Qt-free, so nothing about the hard layering rule changes.

Two things this decision implied without stating, and which the implementation had to settle:

- **The page scans with nothing disabled.**
  `plugin_host::load` filters a disabled plugin out before it opens the manifest, which is right for the running editor and wrong for the page: reading the live registry would list every plugin except the ones the page exists to switch back on.
  The Languages page reads its overlay directly for the same reason, so this is the existing idiom rather than a new one.
- **The wasm tier needed a home.**
  [ADR-0028](0028-wasm-plugin-tier.md) built `WasmTier` and left it un-owned; nothing in the application started one, so the trap rows this page exists for could never have appeared.
  The tier now sits beside the registry in `plugin-host` as a `RwLock<Arc<WasmTier>>` — same shape, same reasoning — started over the registry after each scan and restarted when a plugin is switched off or on.
  Which `HostServices` it runs with is chosen in `ui-shell`, because an implementation that routes a plugin's `notify` to the editor is a Qt object; until such a surface exists, the host's stderr default keeps a plugin's diagnostics reaching the log.

### Rejected alternatives

**A hardcoded icon table.**
Smaller by a wide margin, and the honest answer if icon themes were the only goal.
It was rejected because the user asked for the extension mechanism explicitly, and because a table has no incremental path to a host — the host would replace it rather than grow from it.

**One crate instead of two.**
The contract would then depend on the runtime, so anything reading a manifest — `settings-model`, `icon-theme`, a future contribution consumer — would pull in wasmtime to parse TOML.

**Native dylibs as the third-party tier (ADR-0001's other half).**
Cheaper than WebAssembly, and the `dlopen` and quarantine machinery already exists in `syntax_core::runtime`.
Rejected because a native plugin can take the process down, and ADR-0001 wanted the untrusted tier sandboxed.
The trade-off was accepted for foreign tree-sitter grammars, where there is no alternative; for plugins there is one.
Nothing here forbids adding the tier later — a manifest declaring a native component would be a new optional section and an `api_version` bump.

**Reuse `syntax_core::runtime` directly.**
Its manifest is a language description; every field would have to become optional to carry anything else, and the crate that owns grammars would become the crate that owns plugins.
Its *shape* is reused instead — directory per item, manifest, dot-directories skipped, fail-soft, pointer-swap reload — which is the part worth keeping.
