# 0049. UI internationalization: locale setting, `tr()` sweep, hand-authored translations

## Status

Accepted

## Context

Date/number formatting was already locale-aware throughout `crates/ui-shell/cpp/` (`QLocale::toString(..., QLocale::ShortFormat/LongFormat)` in `history_list_view.cpp`/`commit_detail_view.cpp`, `QLocale`/`%L1` in `ai_chat_panel.cpp`/`editor_tabs.cpp`) — confirmed by grep before this work started, and needed no change.
What was missing was everything else a language picker needs: a persisted `ui_locale` setting, a Settings page to change it, a build pipeline that turns translation sources into something the running app can load, the install of a `QTranslator` at startup, and — the bulk of the work — every user-visible string in `crates/ui-shell/cpp/` wrapped in `tr()` and translated.

Four questions had to be answered, each with a wrong-by-default choice:

1. **Detect the OS locale, or default to English?**
   Auto-detecting looks friendlier on day one but is a silent, unreviewable choice at every install: a user whose OS is set to a locale this build ships no translation for gets English anyway (Qt's `tr()` fallback), and one whose OS locale *is* shipped gets a language they never picked, with no visible setting to explain why. An explicit default that is always English, plus a page that lists exactly what is available, means what the user sees on first launch and what the Language page shows always agree.
2. **Hand-author the `.ts` files now, or run `lupdate` to extract strings first?**
   `lupdate` extracts `tr()` call sites from source into a `.ts`, preserving (and flagging `type="unfinished"`) whatever translations an existing `.ts` already had. There was no existing `.ts` for it to merge against — the very first `.ts` for this codebase is what this change creates — so `lupdate` would have produced the same empty `<translation type="unfinished">` entries this pass had to fill in by hand regardless. Running it anyway would have added a build-time dependency on its exact output shape for zero benefit on a first pass. `lupdate` becomes worth wiring in once these files exist and a future change needs to extract *new* strings against them — documented as a manual step in `crates/ui-shell/translations/README.md`, not a decision to revisit lightly, since a repo this size will want it back in the loop the first time a PR adds ten `tr()` calls a translator has to be told about.
3. **Load `.qm` files from disk, or embed them via `rcc`?**
   Every other bundled asset in this repo — icons, fonts, the ADS docking library's own resources — is `rcc`-embedded into the binary rather than read from a runtime path. A disk-loaded `.qm` would be the one asset in the app whose absence degrades differently (silently falling back to English, like a missing font falls back to a system one, but through a different code path with its own failure mode to reason about) for no offsetting benefit — there is nothing a user or packager needs to swap a `.qm` file for at runtime that a rebuild does not already cover.
4. **Live retranslation (`retranslateUi()` on every widget), or restart-required?**
   Qt's live-retranslation path means every widget that sets `tr()`-wrapped text once, at construction, would need a second call site — a `LanguageChange` event handler, or an explicit re-run of whatever built it — wired individually across all 66 files that call `tr()`. That is real, ongoing plumbing every future `tr()`-wrapped string would also owe, for a switch a user makes rarely (if ever, past their first run) and which the app already asks a restart for on other persisted-at-startup choices (the theme's font install, the language registry). A disabled notice label on the Language page ("Restart the app for a language change to take effect") is the honest minimum, and it is what ships.

## Decision

### 1. `ui_locale` is a person-shaped, global-only setting (`app-config`)

`Settings::ui_locale: String` sits next to `theme`, same shape: empty means "never chosen," `Settings::ui_locale_or_default()` resolves it against `SUPPORTED_UI_LOCALES = &["en", "de", "es", "fr"]`, falling back to `"en"` for an empty or unrecognized value — so a hand-edited `settings.toml` with a typo or a locale this build does not ship a translation for degrades to English rather than to an error or a blank UI. Per ADR-0022, it carries no `ScopedField` variant and no `ProjectSettings` involvement: a project does not get its own language. The accessor and the constant live in `crates/app-config/src/ui_locale.rs` rather than `lib.rs`, purely to stay under that file's ADR-0025 line-count ceiling (matching the `window` module's precedent for the same reason), not because the setting is a bounded concept of its own.

### 2. The bridge pairs `uiLocale`/`saveUiLocale` with `themeName`/`saveTheme`

Same load/mutate/save shape in `crates/ui-shell/src/bridge/settings.rs`, same qinvokable pair in `ffi.rs`. `AppSettings::uiLocale()` returns `ui_locale_or_default()`, never the raw stored value — the Language page never has to special-case an empty or unsupported string itself, the same reasoning `themeName()` already follows for the theme.

### 3. Hand-authored `.ts` → `lrelease` → `rcc`-embedded `.qm`

`crates/ui-shell/translations/ide_de.ts`, `ide_es.ts`, `ide_fr.ts` are hand-written Qt Linguist XML — one `<context>` per C++ class whose `tr()` calls it holds (a free function's `QObject::tr()` call lands under the `QObject` context, matching how Qt resolves a call site's translation context at runtime; getting this wrong means a translation silently never matches and falls back to English, not a build error, so every context in the generated `.ts` files was derived mechanically from where each call site actually sits, not guessed). There is no `ide_en.ts`: English is the `tr()` source text itself.

`crates/ui-shell/build.rs`'s `compile_translations_qrc` follows `compile_ads_qrc`'s exact staged pattern: for each locale, run `lrelease` on its `.ts` into a temp `.qm`, swap it in only if the bytes changed (`replace_if_changed`, the same anti-thrash mechanism `compile_ads_qrc` uses, since every generated file handed to `cpp_file()` becomes a `rerun-if-changed` input); write a generated `translations.qrc` listing all three `.qm` files under prefix `/i18n`; run `rcc --name translations` on it. `docker/Dockerfile`'s `linux-builder` stage gained the `qt6-l10n-tools` package (`lrelease`/`lupdate`; `rcc` already ships with `qt6-base-dev-tools`).

`main_window.cpp`'s `run_app()` calls `installUiTranslators(appSettings, app)` (new `i18n_startup.cpp`, split out for the same ADR-0025 size-ceiling reason `status_bar.cpp`/`navigate_menu.cpp`/`ai_menu.cpp` were) before any widget is built: it inits the `rcc`-embedded resource (`Q_INIT_RESOURCE(translations)` — called from a free function outside every namespace, because the macro's generated `extern` declaration is scoped to wherever it is written, and `run_app()` lives inside `namespace ui_shell`, which would otherwise silently declare and look up a nonexistent `ui_shell::qInitResources_translations()` instead of the real global one `rcc` emits — an undefined-symbol link error, not a compile error, and worth documenting in the code, not just here), then for any locale but `en` installs Qt's own shipped `qtbase_<locale>.qm` (so standard dialog buttons like OK/Cancel are translated too) followed by the app's own `ide_<locale>.qm`. Both loads degrade silently on failure — a missing Qt-shipped translation or app `.qm` leaves that half in English, never a crash.

### 4. Restart-required, no live retranslation

The Language page (`crates/ui-shell/cpp/language_page.h`/`.cpp`, modeled on `appearance_page.h`/`.cpp`'s `{widget, commit}` shape but with no `revert`, since nothing on it previews live) lists English plus each shipped locale's native name (`QLocale::nativeLanguageName()`, capitalized to match every other combo's label casing in Settings), and a disabled notice label saying a restart is needed. `commit()` calls `saveUiLocale`; there is no relaunch button, because no process-relaunch mechanism exists anywhere in this app — a label is the honest minimum rather than a half-built promise.

### 5. Full `tr()` sweep, all strings translated

Every `.cpp` file in `crates/ui-shell/cpp/` with a user-visible string literal now wraps it in `tr()` (`QObject::tr()` in a free function, bare `tr()` inside a `QObject`-derived class's own member function — the existing convention, unchanged). The files with no `tr()` calls at all were individually checked, not assumed compliant: each one's remaining unwrapped string literals are either the brand name ("Kestrel", on the splash screen and the main window title — a proper noun, not translated), single-glyph button icons (✕, ‹, ›), JSON e2e-marker keys, or CSS/QSS selector text — none of them user-readable prose. Every `tr()`-wrapped source string across all 66 files that call `tr()` has a hand-authored, idiomatic (not word-for-word) `de`/`es`/`fr` entry in the `.ts` files; the same English phrase repeated across files reuses the same translation for consistency.

**Standing rule, going forward**: every new user-visible string added to `crates/ui-shell/cpp/` must be `tr()`-wrapped. Noted in `CLAUDE.md`'s Qt/`cpp/` guidance so it is discoverable without reading this ADR.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Detect and default to the OS locale | See Context question 1 — an invisible, unreviewable choice that can silently diverge from what the Language page shows is available. |
| Run `lupdate` to seed the `.ts` files, then hand-fill the extracted entries | Produces the identical `type="unfinished"` entries this pass filled by hand anyway, since there was no prior `.ts` to merge against — extra build-time tooling dependency for no different result on a first pass. Revisit once these files exist and a later change needs true extract-and-merge. |
| Load `.qm` files from a runtime path instead of embedding them | The one bundled asset that would behave differently from every other one (icons, fonts, ADS resources) for no offsetting benefit — see Context question 3. |
| Live retranslation via `retranslateUi()`/`LanguageChange` handling | Ongoing per-widget plumbing across all 66 `tr()`-calling files for a switch a restart already handles honestly elsewhere in this app — see Context question 4. |
| A relaunch button that restarts the process | No process-relaunch mechanism exists anywhere in this codebase; building one just for this page would be scope creep beyond what the plan asked for. A disabled notice label says the same thing without pretending to do more. |

## Consequences

- Positive: `en`/`de`/`es`/`fr` are available from Settings > Language today, with `en` needing no translator at all and every other locale falling back to English per-string automatically (Qt's own `tr()` fallback) for anything a future change adds without a translation yet.
- Positive: the `rcc`-embedded `.qm` files add no runtime file-path dependency and follow the same asset-bundling shape as every icon, font, and the ADS docking library already in this binary.
- Positive: the standing `tr()`-wrapping rule is now written down in both this ADR and `CLAUDE.md`, so the next contributor adding a dialog or a menu item sees it without having to notice the pattern in 66 other files first.
- Negative / accepted: a language switch needs a restart — no mid-session preview, unlike every other setting on the Appearance page. Acceptable per Context question 4; revisit only if user feedback makes this the app's most-complained-about setting.
- Negative / accepted: `lupdate` is not wired into `build.rs` — a `tr()`-wrapped string added after this PR needs someone to run it manually (documented in `translations/README.md`) and then hand-fill the new entry in all three `.ts` files, rather than the build catching a missing translation on its own. Acceptable for the same reason as question 2: automating a merge step this repo has never exercised once would be premature, and the standing rule plus the manual `lupdate` step keep the burden visible rather than silently accumulating.

## Related

- [ADR-0022: per-project settings](0022-per-project-settings.md) — the global-only, person-shaped setting bucket `ui_locale` joins alongside `theme`/`keymap`.
- [ADR-0025: seam split and file-size ceiling](0025-seam-split-and-file-size-ceiling.md) — the precedent `i18n_startup.cpp`'s split-out and `app-config`'s `ui_locale` module both follow.
- [ADR-0003: FFI conventions](0003-ffi-conventions.md) — the typed-error/no-`QString`-sentinel rules `uiLocale`/`saveUiLocale` follow, same as every other bridge accessor.
- `crates/ui-shell/translations/README.md` — the `lupdate` workflow for future string additions.
