# UI translations

`ide_de.ts`, `ide_es.ts`, `ide_fr.ts` are hand-authored Qt Linguist source
files: one `<context>` per C++ class whose `tr()` calls it holds (free
functions use `QObject::tr()`, so their strings live under the `QObject`
context — this must match how `tr()`/`QObject::tr()` resolve their context
at the call site, or `QTranslator` silently fails to find the string and
falls back to the English source text).

There is no `ide_en.ts`: English is the `tr()` source text itself, so it
needs no translation file — an unmatched id falls back to it automatically.

## Build

`build.rs`'s `compile_translations_qrc` runs `lrelease` on each `.ts` to
produce a `.qm`, then `rcc` to embed all three into the binary under the
`/i18n` prefix. `main_window.cpp` loads the active locale's `.qm` (plus
Qt's own shipped `qtbase_<locale>.qm` for standard dialog buttons) at
startup, before any window is shown. Changing the language setting takes
effect after a restart — there is no live retranslation.

## Keeping these current

This pass hand-wrote every entry directly (no `lupdate` extraction step,
since there was no existing `.ts` to merge against). Going forward, once
these files exist, the normal workflow is:

```sh
lupdate crates/ui-shell/cpp/*.cpp -ts crates/ui-shell/translations/ide_de.ts \
                                       crates/ui-shell/translations/ide_es.ts \
                                       crates/ui-shell/translations/ide_fr.ts
```

`lupdate` adds new/changed `<source>` entries (flagged `type="unfinished"`)
and marks removed ones as obsolete, without touching translations that are
still current — run it after adding or changing any `tr()`-wrapped string,
then fill in the new entries by hand. It is not wired into `build.rs`:
running it is a manual step, left for whoever adds the next batch of
strings.

## Standing rule

Every new user-visible string added to `crates/ui-shell/cpp/` must be
wrapped in `tr()` (or `QObject::tr()` in a free function). See
[ADR-0049](../../../docs/architecture/decisions/0049-ui-internationalization.md).
