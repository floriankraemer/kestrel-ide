# 0022. Per-project settings: a sparse project layer, resolved by `settings-model`

## Status

Accepted

## Context

Every setting this IDE has is global.
`settings.toml` in the platform config directory holds the theme, the fonts, the keymap, the language-server overrides and the AI providers, and it holds them once for the person rather than once per project.

That was right while the settings were all preferences.
It stops being right the moment a setting describes *the project* rather than the person reading it.
Run configurations are the immediate forcing case — a run configuration is the definition of a project, it belongs in version control beside the code, and a global list of them would be wrong on the first day a second project is opened.
Editing behaviour (tab width, spaces versus tabs) has the same shape: a Go project and a Python project disagree, and the disagreement belongs to the projects, not to the developer.

## Decision

### 1. A second file, `<project_root>/.ide/settings.toml`, layered over the global one

The project layer is loaded from the project root and takes precedence over the global file, per key.

It is meant to be **committed**.
That is the point: a project configures itself the same way for everyone who checks it out.
Machine-local state — window geometry, dock layout, open tabs, recent files — stays global and out of the project, because it belongs to a person at a desk, not to the code.

### 2. A separate, **sparse** type, not a second `Settings`

Every field on `Settings` is `#[serde(default)]`.
That makes a missing key and a key explicitly set to the default indistinguishable once parsed, which is fine for a single file and fatal for a layered one: *"tab width is not set here, ask the global layer"* and *"tab width is set to 0"* have to be different answers.

So `ProjectSettings` is its own type, every field an `Option`, and `None` means **absent**.

It is also sparse in a second sense: it covers only the settings that may be overridden, not a mirror of all 22 fields.
A full mirror would have to be hand-synchronised forever and would leave room for fields to drift that must never be project-scoped at all.

### 3. Precedence is a rule, so it lives in `settings-model`

`app-config` reads and writes two files and knows nothing about which one wins (ADR-0017).
`settings_model::scope` resolves the layers and answers where each effective value came from, so the settings dialog can label a value's origin rather than re-deriving it.

### 4. What may be overridden, and what may not

Project scope covers project-shaped settings: editing behaviour, language servers, run configurations, index excludes, and the terminal's shell and start directory.
Global scope keeps person-shaped ones: theme, fonts, keymap, AI providers.

The terminal was added to that list after the shell picker was built.
A repository whose tooling only runs under WSL, or under `bash` on a machine whose owner uses `fish`, is describing the checkout rather than the person — the same test every other project-scoped area passes.
This is the "widening it later is additive" case the next paragraph anticipates, and it cost exactly one variant in `settings_model::scope::ScopedField`.

The line is defensible in one sentence — **a project may configure the project, not you** — and it matters that it is drawn deliberately.
Widening it later is additive; narrowing it is a breaking change to a file people have already committed.

### 5. A version, and unknown keys kept verbatim

The project file carries a `version`, refused when it is from the future.
`Settings` has none, which leaves it unable to tell a file written by a newer build apart from a corrupt one; the project layer starts with one while that costs nothing.

Unknown keys round-trip untouched.
Without that, opening a project in an older build and changing one setting silently deletes every key the older build had never heard of — from a file that is shared with the whole team.

### 6. The same durability guarantees as the global file, by construction

Both layers are persisted through one path-keyed core, so a missing file defaults, a **malformed file is an error rather than silently becoming defaults**, the write is atomic, and read-modify-write aborts rather than saving defaults over a file it could not read.

This is not hypothetical caution: `settings.toml` was once wiped to defaults by precisely that failure mode.
Sharing the implementation means the project layer inherits the fix instead of having to remember it.

### 7. `.ide` is confined to the project, on read **and** write

A `.ide` that resolves outside the project root — most obviously a symlink — is refused.
Reading it would disclose a file the project has no claim to, and writing it would let a checked-out repository scribble outside its own directory.
It is a trust boundary, so it is checked on both sides rather than only where it seems likely.

### 8. `.ide/.gitignore` is seeded with `local/`

`.ide/settings.toml` is meant to be committed, but it sits beside `.ide-index/`, which is not.
The obvious reflex — `echo .ide >> .gitignore` — would silently un-commit the settings the project is trying to share.
Seeding the directory's own `.gitignore` makes the intent self-documenting.
An existing file is never touched; it is the user's.

## Consequences

- Run configurations (ADR-0029) have somewhere to live that travels with the project.
- Editing behaviour can differ per project without a second mechanism.
- There are now two files to reason about when a setting looks wrong, which is why `scope::origin` exists and why the dialog shows where a value came from rather than leaving the user to guess.
- The project layer starts small on purpose. Fields are added as their features land; a key that parses and then does nothing reads as a working feature and is worse than no key at all.

## Alternatives rejected

**Reuse `Settings` for both layers.** Its `#[serde(default)]` scalars cannot distinguish unset from default, so the project layer would silently zero the global one.

**Merge in `app-config`.** It is dumb persistence by ADR-0017. Precedence is a rule, and rules that need a vocabulary — which fields are project-shaped — belong in `settings-model`.

**One flat file with `[project.*]` sections in the global settings.** Unshareable, which defeats the purpose: the point is that the file is committed with the project.

**Everything overridable per project.** A project that forces your theme and your keymap is hostile. The split *is* the decision.

**`.editorconfig` as the project layer.** It covers a fraction of what needs scoping and is out of scope for the editing-behaviour work. Worth revisiting as an *importer* into this layer, not as a replacement for it.
