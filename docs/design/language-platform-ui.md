# Language platform UI specification

UX specification for tasks T4, G3, L6 and the Problems dock from L2 in `docs/architecture/language-platform-plan.md`.
Written so those pages get implemented against a spec instead of being invented at the keyboard.
This document specifies layout, states, wording, keyboard behaviour and colour; it deliberately contains no C++ and no Rust.

## 0. What is already established, and is not up for redesign

The Settings dialog is a `QDialog` with a `QListWidget` category rail on the left and a `QStackedWidget` on the right, built in `showSettingsDialog` in `crates/ui-shell/cpp/main_window.cpp`.
Three of the four new surfaces are new rows in that rail; the fourth is a new ADS `CDockWidget` next to Terminal, Search Results and Find Usages.

Two commit models already coexist in that dialog and both stay valid.
Appearance and Editor apply live and are reverted by the Cancel branch; Keymap and MCP edit a draft and commit on OK.
Syntax Colors follows Appearance (apply live, so the user sees the colour in the open editor while picking it), and Languages and Language Servers follow Keymap (draft, commit on OK), because starting and stopping a language server on every keystroke in a command field is not a preview, it is a fork bomb.

The Keymap page in `crates/ui-shell/cpp/keymap_page.{h,cpp}` is the visual and structural precedent for all three settings pages.
That shape is: a `QTreeWidget` with non-selectable category rows, a bottom control strip that acts on the currently selected row, bold text to mark "differs from default", a confirming `QMessageBox` for anything destructive, and every rule delegated across the bridge.
All three new pages reuse that shape rather than introducing a second table idiom.

Existing wording conventions that the new surfaces follow: sentence case for labels, a trailing colon on form labels, `...` on any button that opens a dialog, and a plain status sentence with a trailing period in a status label (`Index ready.`, `No matches selected.`).

## 1. Semantic colour tokens (prerequisite for every surface below)

`crates/ui-shell/cpp/theme.cpp` currently defines chrome only — it has no error, warning, info or success colour.
All four surfaces need one, so a small semantic set is added per theme, exposed the same way the stylesheets already are, and used by nothing else.

Colour themes are now a `color-themes` plugin contribution point ([ADR-0050](../architecture/decisions/0050-color-themes-as-a-contribution-point.md)): semantic, chrome, diff and terminal colours are per-theme *data* a contribution defines for itself, read through `ThemeProvider`, not a hardcoded branch keyed on one of a fixed set of theme names.
The table below still names the three original built-in themes because they are the ones this spec was written against and remain the ones every contrast number here was checked against; any other installed theme — including the nine vendored GitHub/Primer themes or a third-party VS Code theme JSON — supplies its own values through the same tokens rather than through a new branch in `theme.cpp`.

| Token | dark (on `#2b2b2b`) | light (on `#ffffff`) | vscode-dark (on `#252526`) |
|---|---|---|---|
| `severity.error` | `#ff6b68` (5.1:1) | `#c62828` (5.6:1) | `#ff6b68` (5.6:1) |
| `severity.warning` | `#d9a441` (6.3:1) | `#8a6100` (5.6:1) | `#d9a441` (6.9:1) |
| `severity.info` | `#74a7cc` (5.5:1) | `#1565c0` (5.8:1) | `#74a7cc` (6.0:1) |
| `status.ok` | `#6aab73` (5.2:1) | `#2e7d32` (5.2:1) | `#6aab73` (5.6:1) |
| `status.muted` | `#9a9a9a` (5.0:1) | `#5f5f5f` (6.5:1) | `#9a9a9a` (5.5:1) |

Every value clears WCAG AA 4.5:1 for body text against that theme's list background, which is the strictest place any of them is used.
Darcula's own `#6897bb` info blue was rejected at 4.50:1 against `#2b2b2b` — it passes by rounding, and it would fail the moment a row is drawn on the alternating `#313335` band.

**Meaning is never carried by hue alone.**
Every severity and status value is rendered as a short text label, and colour is applied to that label — `Error`, `Warning`, `Info`, `Running`, `Crashed`, `Disabled`.
A colourblind user, a greyscale screenshot in a bug report and a screen reader all get the same information from the text.
Where an icon is added later it is additive; it never replaces the word.

Selected rows already invert to a strong selection background (`#214283`, `#90caf9`, `#094771`).
Severity colouring is dropped on the selected row and the row uses the theme's selection foreground, because a red-on-blue row is worse than an uncoloured one and the severity word is still there.

## 2. Settings > Syntax Colors

### 2.1 The problem

Roughly 30 tree-sitter capture scopes, each with a foreground colour and bold/italic/underline flags, in a base table plus an override table per language.
Any given cell of that matrix has one of three origins: the theme default, a base customisation the user made, or a per-language override the user made for the selected language.
Making that origin visible at a glance is the whole design problem; the colours themselves are a solved `QColorDialog` interaction.

### 2.2 Layout

```
Language: [ (Base — all languages)      v ]        [ Reset Language... ]
+----------------------------------------------------------------------+
| Scope                     Sample              Style     From         |
| v Comments                                                            |
|     comment               // comment          I         Theme        |
|     comment.doc           /** doc */          I         Base         |
| v Literals                                                            |
|     string                "text"                        Theme        |
|     string.escape         \n                            Theme        |
|     number                42                            Rust         |
| v Identifiers                                                         |
|     variable              name                          Theme        |
|     function              call()                B       Base         |
|     type                  TypeName                      Rust         |
| v Keywords                                                            |
|     keyword               return                B       Theme        |
| ...                                                                   |
+----------------------------------------------------------------------+
 Color: [ #cc7832 ][ Choose... ]  [x] Bold [ ] Italic [ ] Underline
                                            [ Reset Scope ]
```

The tree is a `QTreeWidget`, not a `QTableWidget`.
The rows are grouped, the group headers are inert, the row count is fixed and known at build time, and the selection drives a control strip below — that is the Keymap page's structure exactly, and reusing it costs nothing while a `QTableWidget` would need its own grouping fake with spanned rows.
The tree is created with `setRootIsDecorated(false)` and `setIndentation(12)` to match Keymap.

Group headers are the scope families of the tree-sitter capture vocabulary and are fixed: Comments, Literals, Identifiers, Keywords, Operators and punctuation, Types, Markup, Diagnostics.
A scope whose family has no members in the current taxonomy is not rendered as an empty group — an empty group header is chrome that carries no information.

Column resize modes follow Keymap: Scope stretches, Sample and Style and From are `ResizeToContents`.

### 2.3 The Sample column is the preview, and there is no preview pane

**Judgement call, flagged for the user.**
A live code-preview pane was considered and is not specified.

Each row's Sample cell renders a short, representative fragment for that scope *in that scope's own resolved style* — colour, bold, italic, underline — using the editor font from `AppSettings::editorFont()`.
That gives every one of the ~30 rows its own preview at zero vertical cost, updates the instant the user picks a colour, and is honest about the flags in a way a colour swatch is not.
A separate preview pane would show maybe 12 lines of code that between them exercise perhaps eight of the 30 scopes, so the user editing `string.escape` or `markup.link` would watch an unchanged pane and conclude the setting was broken.

The counter-argument, which is real: only a code pane shows scopes *in combination*, and syntax colour schemes are judged by how the whole thing reads, not by whether `keyword` is a nice orange.
The mitigation specified here is that the dialog applies live to the open editor, so the user's actual code behind the dialog is the preview — better than any synthetic sample, at the cost of the dialog possibly covering it.
If the user wants the pane anyway, the place for it is a collapsible splitter below the tree defaulting to collapsed, and it should be a real editor widget fed a per-language sample file, not a hand-written `QTextEdit` with fake highlighting.

### 2.4 Language selection and the override model

The language `QComboBox` at the top holds `(Base — all languages)` first, a separator, then every language in the registry in catalog order.
Nothing else lives on that row except the `Reset Language...` button, which is disabled while Base is selected.

With Base selected, edits write the base table and the From column reads `Theme` or `Base`.
With a language selected, edits write that language's override table, and the From column reads one of three values.

| From | Means | Rendering |
|---|---|---|
| `Theme` | Nothing customised; the value comes from the active theme's built-in table. | `status.muted`, regular weight. |
| `Base` | The user customised it for all languages; this language inherits that. | Default foreground, regular weight. |
| *language name* | This language overrides whatever base or theme says. | Default foreground, **bold**, exactly as Keymap bolds a non-default shortcut. |

Bold-means-overridden is the single mechanism the user already learned on the Keymap page, so it is reused rather than replaced by a coloured dot or a modified-marker gutter.
The From column is what makes "this language overrides the base" legible without hovering, clicking or comparing two screens.

When a language is selected and a scope reads `Theme` or `Base`, the Sample cell still renders the effective inherited style, because that is what the editor will draw.

### 2.5 Resetting

Three scopes of reset, each named for what it actually clears.

`Reset Scope` in the control strip clears the selected row *at the current level only*.
With Base selected it removes the base entry and the row falls back to `Theme`.
With a language selected it removes that language's override and the row falls back to `Base` or `Theme` — it does not touch the base entry, and the button's tooltip says so: `Remove this language's override and inherit the base style.`
The button is disabled when the row is already at `Theme` while Base is selected, or already inheriting while a language is selected, so it never looks like a no-op the user has to test.

`Reset Language...` clears every override for the selected language after a `QMessageBox::question`: `Remove all Rust colour overrides and inherit the base styles?` with Yes/Cancel defaulting to Cancel, matching `Reset All` on the Keymap page.

There is no `Reset Everything` button.
Selecting Base and pressing `Reset Language...` — relabelled `Reset Base...` while Base is selected — already does it, and a third destructive button next to two others is how a user resets the wrong thing.

### 2.6 States

**Unmodified state.** Every row reads `Theme`, the From column is entirely muted, and no row is bold.
This is the state after a fresh install and it needs no banner, no "you have not customised anything yet" text, and no call to action.

**Empty state.** The tree is never empty; the scope taxonomy is static and closed by design decision 5 of the plan.
The language combo is never empty either, because the built-in catalog is compiled in.
The only genuinely empty case is a *language* with no overrides, which is the unmodified state above and needs nothing.

**Theme changed while the dialog is open.** Appearance and Syntax Colors are separate pages of the same dialog, so this is reachable.
The `Theme` rows repaint to the new theme's defaults, `Base` and per-language rows keep the user's colours, and nothing is lost — the user's customisations are stored independently of the theme, which is the behaviour the From column already promised.

### 2.7 Keyboard and focus

Tab order: language combo → tree → colour hex field → `Choose...` → Bold → Italic → Underline → `Reset Scope` → `Reset Language...` → the dialog's OK/Cancel box.

Inside the tree, Up/Down moves between scope rows and skips group headers, which are already non-selectable via `Qt::ItemIsEnabled` on the Keymap page.
Left/Right collapse and expand a group, which is `QTreeWidget` default behaviour and is not overridden.
Enter on a selected row opens the colour dialog, so a keyboard user never has to reach `Choose...` by tabbing.
`Ctrl+B`, `Ctrl+I`, `Ctrl+U` toggle the three flags for the selected row while the tree has focus; these are the conventional bindings and cost nothing.

The hex field is editable, not read-only, and accepts `#rrggbb`.
Typing a colour is faster than a colour wheel for a developer porting a scheme from another editor, and it is the only way to enter an exact value.
An invalid value leaves the previous colour applied and shows the field in the error colour with an inline message below the strip — never a modal.

### 2.8 Where this page says nothing

No per-row icons; the Sample cell already carries the visual.
No colour swatch column separate from Sample; one visual per row.
No count of customised scopes.
No "Changes apply immediately" hint — the user sees them apply immediately.

## 3. Settings > Languages

### 3.1 Layout

```
[x] Show only languages with problems              [ Add Language... ]

+----------------------------------------------------------------------+
| Language          Matches                Source        Status        |
| v Built-in                                                            |
|     Rust          *.rs                   Built-in                     |
|     Python        *.py, *.pyi            Built-in                     |
|     Zig           *.zig                  Built-in                     |
| v User config                                                         |
|     Nim           *.nim                  Overlay                      |
|     Odin          *.odin                 Overlay       Query error   |
| v Grammar libraries                                                   |
|     Vala          *.vala                 libvala.so    Disabled      |
+----------------------------------------------------------------------+
| Odin — highlights.scm                                                 |
| The highlighting query does not match this grammar.                   |
| Line 14: no node type named "proc_declaration".                       |
| /home/you/.config/ide/languages/odin/highlights.scm                   |
+----------------------------------------------------------------------+
[ Disable Language ]                        [ Open File ]  [ Reload ]

Languages are read from /home/you/.config/ide/languages
```

A `QTreeWidget` again, grouped by source, with a details pane below that is populated only when the selected language has something to say.

Grouping by source rather than sorting a flat list by a Source column is deliberate: "which of these did I add, and which came with the app" is the question a user opens this page with, and grouping answers it before they read a single row.
Empty groups are not rendered — a user with no overlays sees no `User config` header.

### 3.2 The Status column, and the healthy majority

A language that loaded correctly has an **empty** Status cell.
Not `OK`, not a green check, not `Loaded`.
Thirty rows of green checks train the eye to skip the column, which is precisely the column that has to catch the eye on the one row that failed.
This is the strongest "say nothing" call in the document and it should not be softened during implementation.

It also fixes the column widths.
An empty cell only catches the eye while it is on screen, so Status must never be reachable only by scrolling sideways.
Matches is the single column that absorbs the leftover width and therefore the only one that elides — Bash and Dockerfile each enumerate a dozen filenames, and sizing that column to its longest content is what pushed Status off the right edge.
Language, Source and Status size to their content, the last section does not stretch, and the sections therefore always sum to the viewport width.
The settings dialog opens at 960x640 for the same reason: the pages' own minimums add up to about 740x510, which lays a page out but leaves Matches nothing to elide.

| Status text | Colour | Meaning |
|---|---|---|
| *(empty)* | — | Loaded and available. |
| `Grammar error` | `severity.error` | The grammar failed to load: missing symbol, bad ABI, unreadable file. |
| `Query error` | `severity.error` | The grammar loaded but a `.scm` query would not compile. |
| `Version mismatch` | `severity.error` | Grammar ABI outside the supported range. |
| `Disabled` | `status.muted` | Turned off by the user. Its details pane says so, and the strip's toggle reads `Enable Language`. |
| `Disabled after crash` | `severity.warning` | Auto-quarantined by the crash marker (plan G1b). |

`Not loaded` — "compiled in but its grammar is unavailable in this build" — was specified here and is **not implemented, because it cannot happen**.
Every grammar in `syntax_core::BUILTIN_LANGUAGES` is a non-optional dependency reached through a plain `fn() -> tree_sitter::Language`, with no Cargo feature and no `cfg` anywhere in the catalog: a language that is compiled in always has its grammar.
Adding the status would mean adding a state the code can never produce, so the row was removed rather than faked.
If a build ever gates a grammar behind a feature, that is when the status earns its place.

The filter checkbox `Show only languages with problems` is unchecked by default and filters to the non-empty Status rows.
It exists because the failure case is a needle in ~25 rows and the user arriving here already knows something is broken.

### 3.3 Making errors legible instead of pasting a Rust error

The details pane is the whole point of this page and it has a fixed four-part shape.

1. **Title line**: the language and the artefact that failed — `Odin — highlights.scm`, `Vala — libvala.so`.
2. **One plain sentence saying what is wrong**, written for a user who has never read the tree-sitter source.
3. **The specific detail**, including a line number when the underlying error has one.
4. **The path**, selectable, so it can be copied.

The raw error string from the Rust side is never rendered on its own.
It is mapped to one of a small fixed set of causes at the seam, and each cause gets a sentence written here.

| Cause | Sentence | Actions offered |
|---|---|---|
| Query will not compile | `The highlighting query does not match this grammar.` plus `Line N: <detail>.` | Open File, Reload |
| Missing entry symbol | `This library does not export a tree-sitter grammar. Expected a function named tree_sitter_odin.` | Reload, Open Folder |
| ABI mismatch | `This grammar was built for tree-sitter ABI 12; this build supports 13 to 15. Rebuild it against a newer tree-sitter.` | Reload, Open Folder |
| Malformed manifest | `language.toml could not be read.` plus `Line N: <detail>.` | Open File, Reload |
| File unreadable or missing | `The file could not be opened.` plus the OS message. | Reload |
| Crash quarantine | `This grammar crashed the editor on <date>, so it was disabled automatically. Re-enable it if you have since rebuilt or replaced it.` | Open Folder |

Every sentence names the file, the expected thing, or the version — never `error: parse failed`.
The exact underlying string is not thrown away; it goes to the log, and the details pane is what the user reads.

`Open File` opens the offending file in the editor behind the dialog, which is the only genuinely actionable button for a query or manifest error, and it is the reason this page is worth building rather than printing a startup warning.

Turning a language on or off is *not* one of these per-cause actions, and no longer appears in this pane; it lives in the strip described in 3.4, which reaches every row rather than only the two error causes that used to carry it.

### 3.4 Turning a language off, and back on

A single toggle sits at the left of the page's bottom control strip, the same shape the Keymap page's strip uses, and it acts on the selected row.

It is one button, not two, and its caption follows the selection: `Disable Language` for a row that is currently on, `Enable Language` for a row showing `Disabled` or `Disabled after crash`.
A control that says `Disable Language` while pointing at a language that is already off would be lying about what pressing it does, so which caption a row gets is decided in `settings-model` alongside the row's status rather than in the widget.
With no row selected the button stays in place, greyed: the strip is part of the page, not something that appears once you have earned it.

The strip, rather than a per-row control, is the deliberate choice here.
An `Enabled` column or a per-row checkbox is exactly what 3.2 forbids — it would put a mark on all thirty healthy rows and train the eye off the one column that has to catch it.
A selection-driven strip adds no per-row chrome at all, and it reaches the healthy majority, which the details pane never could: before this, a language that had simply loaded correctly offered no way to turn it off short of hand-editing `settings.toml`.

Disabling asks nothing.
It is reversible from the same button, it takes effect immediately — files of that language already open drop to plain text without the dialog closing — and a language that is off says so once, in its Status cell, with no checkmark added anywhere else.
Re-enabling asks nothing either, with one exception, which is 3.5.

A disabled healthy language gets the same muted `Disabled` status and the same details pane as a disabled broken one: `This language is turned off. Files it would claim open as plain text.`
The pane says what being off means for the user's files; the strip is what changes it back.

### 3.5 The crash quarantine

A grammar that crashed on a previous launch appears with `Disabled after crash` in `severity.warning`, and it is the one status that is a warning rather than an error, because the current session is fine.
Its details pane states the date, and the strip's toggle reads `Enable Language`, which clears the marker as well as the user's disable — one button for both causes, because a user looking at `Disabled after crash` and a user looking at `Disabled` are pressing the same thing for the same reason.
Re-enabling *this* row, and only this row, shows a confirming `QMessageBox::warning`: `Vala crashed the editor on 2026-08-14. Enable it again?` with Yes/Cancel defaulting to Cancel.
That confirmation is warranted — this is the one setting in the dialog that can take the app down.

The status bar also carries a compact indicator when any language is in an error or quarantine state (`2 language problems`), clickable, opening Settings on this page, per plan task G3.
When nothing is wrong the status bar shows nothing at all.

### 3.6 Adding a language

`Add Language...` opens a small modal with two paths, because the plan ships two mechanisms (G1a data overlay, G1b foreign dylib) and pretending they are one thing would lie about what the user is choosing.

```
Add a language

 (o) From a folder of tree-sitter queries
     A folder containing language.toml and one or more .scm files.
     [ /home/you/downloads/nim-queries         ] [ Browse... ]

 ( ) From a compiled grammar library
     A shared library exporting tree_sitter_<name>.
     [                                          ] [ Browse... ]
     Loading a grammar library runs code inside the editor.
     A faulty grammar can crash it.

                                          [ Add ]  [ Cancel ]
```

`Add` copies or links the artefact into the config directory, attempts the load immediately, and closes.
Whatever happened is then visible in the list and the details pane — a successful add lands as a new row under `User config` or `Grammar libraries` with an empty status and the row selected, a failed add lands as a row with its error already explained.
The add dialog never reports the outcome itself; a modal that says "added successfully" and then a list that says "query error" is two sources of truth.

The security note under the library option is plain text in the default foreground, not a red warning box.
It states a fact the user needs before choosing, and shouting it would just get it tuned out.

### 3.7 Restart to apply

Plan task G2 delivers live reload with `Arc`-held compiled languages, so the normal case requires no restart and this page must not say otherwise.

A restart notice appears only when a specific action genuinely cannot take effect live, and the known case is disabling or replacing a foreign dylib, which is never unloaded by design (plan decision 10).
It renders as an inline strip at the bottom of the page above the button box, in the default foreground with the `severity.info` colour on the leading word:

`Restart required — Vala stays loaded until the editor restarts.  [ Restart Now ]`

It is not a modal, it does not block OK, and it disappears when the condition no longer holds.
`Restart Now` prompts to save modified documents through the existing path before restarting.

### 3.8 Keyboard and focus

Tab order: filter checkbox → `Add Language...` → tree → the strip's enable/disable toggle → details pane action buttons in the order they are shown → restart strip's button if present → dialog button box.

Up/Down moves between languages and skips group headers.
Enter on a selected row with an `Open File` action performs it, since that is the row's primary action; on a healthy row Enter does nothing.
`Delete` on a row from `User config` or `Grammar libraries` offers to remove it, with a confirming question naming the folder or file to be removed; `Delete` on a built-in row does nothing.

The details pane is a focusable, read-only, selectable text area so a keyboard user can select and copy the message and the path.

### 3.9 Where this page says nothing

No status text on healthy languages.
No `Enabled` column and no per-row checkbox: whether a language is on is said once, by the Status cell of the rows that are off.
No group header for a source with no languages.
No details pane when the selected language has nothing to report — the pane collapses rather than showing `No problems.`
No version, grammar hash or file-size columns; nobody opens this page to learn those.

## 4. Settings > Language Servers

### 4.1 Layout

```
+----------------------------------------------------------------------+
| On   Language      Command                       Status              |
| [x]  Rust          rust-analyzer                 Running             |
| [x]  Python        pylsp                         Starting            |
| [x]  Go            gopls                         Crashed, retrying   |
| [ ]  TypeScript    typescript-language-server    Disabled            |
|      C++                                         Not configured      |
| [x]  Zig           zls                           Command not found   |
+----------------------------------------------------------------------+
 Language:  C++
 Command:   [ clangd                                              ]
 Arguments: [ --background-index --clang-tidy                     ]
 [x] Enabled                                        [ Restart Server ]

 clangd: exited immediately with status 127.
 Check that the command is on PATH.
```

A flat `QTreeWidget` with `setRootIsDecorated(false)` — no grouping, because the natural grouping key would be status and status changes while the user watches, which would make rows jump between groups.
Sorted by language name, stable, always.

The `On` column is a checkbox, editable in place, because enable/disable is the single most frequent action on this page and making it a two-step select-then-toggle would be worse for no gain.

Every language in the registry gets a row, including those with no server, so "does this editor support a language server for C++" is answered by looking rather than by an `Add` dialog with a language dropdown.
That removes the need for an `Add Server` button entirely: configuring a server for a language with no default is selecting its row and typing a command.
The row's `Not configured` status is muted, its checkbox is absent rather than unchecked, and it turns into a normal row the moment a command is entered.

### 4.2 Status column

| Status | Colour | Meaning |
|---|---|---|
| `Not configured` | `status.muted` | No command; the row is a placeholder. |
| `Disabled` | `status.muted` | Configured but switched off. |
| `Starting` | default foreground | Process spawned, `initialize` outstanding. |
| `Running` | `status.ok` | Initialised and answering. |
| `Crashed, retrying` | `severity.warning` | Died; `LspManager` backoff is in effect. |
| `Command not found` | `severity.error` | Spawn failed at the OS level. |
| `Stopped` | `severity.error` | Died and retries are exhausted. |

`Running` is the one healthy status that *is* shown, unlike the Languages page's empty cell, because it is live state rather than a static property — the user came here to find out whether the thing is up right now, and an empty cell would read as "no answer yet".

Status updates arrive on the manager's signals and repaint the affected row only.
`Starting` and `Crashed, retrying` are transient by nature; no spinner, no animation, no progress bar — the word changes and that is the feedback.
The status column is `ResizeToContents` and sized to the longest string so live changes never reflow the table.

### 4.3 A failing command without a modal

A bad command must never open a dialog, because the LSP manager retries on a backoff and a modal per retry would make the editor unusable.

The failure surfaces in exactly three places.
The row's Status cell changes.
The detail strip under the form shows two lines while that row is selected: what happened, and what to do about it.
The status bar shows nothing, because a language server that is down is not an editor-wide condition.

The detail strip's wording follows `error-message-design`: what failed, then the one thing the user can check.

| Failure | Detail lines |
|---|---|
| Spawn failed | `clangd: exited immediately with status 127.` / `Check that the command is on PATH.` |
| Binary missing | `clangd: no such file or directory.` / `Enter an absolute path, or install it and reopen this page.` |
| Died after initialising | `gopls exited with status 1 after 4 seconds. Retrying in 8 seconds.` / `Its output is in the log.` |
| Protocol error | `pylsp did not answer initialize within 10 seconds.` / `Try running it in a terminal to see its startup output.` |
| Retries exhausted | `zls stopped after 5 failed starts.` / `Fix the command, then press Restart Server.` |

The strip occupies no vertical space when the selected row is healthy.

### 4.4 Commit model and Restart Server

The page edits a draft and commits on OK, matching Keymap and MCP.
On OK, the manager reconciles: newly enabled or changed servers start, disabled ones stop, untouched ones are left alone.
Cancel discards the draft and nothing is started or stopped.

The Status column, however, is *live even while the draft is dirty*, because it reports the running world, not the draft.
A row whose command was edited but not yet committed shows its current status plus a muted suffix `(pending)`, so the user is never misled into thinking their new command is what is running.

`Restart Server` is enabled only for a row that is currently running or stopped and has no uncommitted edits, and it restarts immediately rather than waiting for OK — it is an action, not a setting.

### 4.5 Keyboard and focus

Tab order: tree → Command → Arguments → Enabled → `Restart Server` → dialog button box.
The detail strip is not in the tab order; it is static text.

Space toggles the `On` checkbox of the selected row without leaving the tree.
Up/Down moves between rows; the form below repopulates and any uncommitted edit in the form is written to the draft first, so navigating away never silently discards typing.
Enter in the Command or Arguments field commits that field to the draft and returns focus to the tree, which is the fast path for configuring several servers in a row.

Command and Arguments are one field each, and Arguments is a single space-separated line rather than a list editor with add/remove buttons.
Users of this page know how to type a command line; a list editor is four widgets and two behaviours to learn in exchange for correctly handling arguments containing spaces, which is rare enough to document rather than build for.
**Judgement call**: if quoted arguments turn out to matter, the upgrade is standard shell-style quoting in the same field, not a list editor.

### 4.6 Where this page says nothing

No process ID, no port, no uptime, no memory column.
No log pane; the log already exists and duplicating a tail of it here would be a second, worse log viewer.
No `Add Server` button, since every language already has a row.
No detail strip on a healthy row.

## 5. Problems dock panel

### 5.1 Placement and layout

A new ADS `CDockWidget` titled `Problems`, added with `CenterDockWidgetArea` against the existing bottom dock area, so it tabs alongside Terminal, Search Results and Find Usages exactly as those do to each other.
It is not visible on first run; it is opened by the View menu action, by clicking the status-bar counter, and automatically the first time a diagnostic arrives in a session — once per session, never again, because a panel that reopens itself every time a file fails to compile is a panel the user learns to fight.

```
[ Filter                    ] [x] 3 Errors [x] 12 Warnings [ ] 40 Infos
+----------------------------------------------------------------------+
| v src/main.rs (2)                                                     |
|     Error    14:9   cannot find value `conifg` in this scope   rustc  |
|     Warning  31:1   unused import: `std::fmt`                  rustc  |
| v src/theme.rs (1)                                                    |
|     Error     7:22  expected `;`, found `}`                    rustc  |
+----------------------------------------------------------------------+
 3 errors, 12 warnings in 2 files.
```

A `QTreeWidget` grouped by file, which is `SearchResultsPanel`'s exact structure — same widget, same grouping, same bottom status label, same double-click-to-open behaviour.
A user who has used Search Results already knows how to use this panel, which is Jakob's Law applied inside a single application.

Columns: Severity, Line:Column, Message, Source.
Severity stretches to nothing, Line:Column and Source are `ResizeToContents`, Message stretches.
The file path lives in the group header, not in a column, so the message column gets the width — the messages are the content and they are long.

### 5.2 Grouping and sorting

Grouped by file, one group per file with a diagnostic, the group header showing the project-relative path and the count in that file.
Groups are sorted by path; within a group, rows are sorted by line, then column, then severity.

Grouping is fixed, not user-selectable.
A grouping dropdown is a preference a user sets once and never changes, and the plausible alternative — group by severity — is already served by the severity filters.

The currently open file's group sorts to the top and stays expanded, because the diagnostics the user is acting on are almost always in the file they are looking at.
All other groups are expanded by default too; collapsing state is remembered per file while the panel stays open and is not persisted across sessions.

### 5.3 Filtering

Three toggle buttons, one per severity, each showing its live count and defaulting to Errors and Warnings on, Info off.
Info-level diagnostics from a chatty server would otherwise bury the errors on first open.
A severity with zero diagnostics renders its button disabled with a `0` count rather than hiding it, because a button that appears and disappears as you type is worse than a dim one — this is the one place counting to zero earns its space, since the button has to exist for the toggle state to be meaningful.

The `Filter` line edit does a case-insensitive substring match over message, file path and source, filtering rows live.
Empty groups disappear while filtering; the status label switches to `Showing 4 of 15.`
There is no regex toggle, no case toggle, no whole-word toggle — Search Results has those because it searches the project, and this filters a list of at most a few hundred rows.

### 5.4 Clicking a row

Single click selects and does nothing else.
Double click, and Enter, open the file and place the caret at the diagnostic's line and column, through the same `openAt(path, line, column)` callback `SearchResultsPanel` already uses.
The editor scrolls the line into view and selects the diagnostic's range where the server gave one, so the user sees exactly what the server is complaining about rather than a caret in the vicinity.
Focus moves to the editor, because the user's next action is to fix the code.

Double-clicking a group header expands or collapses it and opens nothing.

The panel does not follow the editor's cursor and does not highlight the row for the line the caret is on.
A list that reorders or scrolls itself while the user types is a list the user cannot use.

### 5.5 States

**Empty — no server.** `No language server is running for this file. Configure one in Settings > Language Servers.`
The second sentence is the whole reason for this state to exist; without it the user concludes diagnostics are broken.

**Empty — server running, nothing wrong.** `No problems.`
Two words, centred, muted, no icon, no illustration, no "Great job!".

**Waiting.** While a server is `Starting` and has not yet reported, `Waiting for rust-analyzer...` in the status label, list area empty.
Diagnostics for a large project can take tens of seconds to first arrive, and an empty list with no explanation reads as a bug.

**Stale.** When the server that produced the current rows has crashed, the rows stay visible, dimmed to `status.muted`, and the status label reads `rust-analyzer stopped; these results are from <time>.`
Clearing the list on crash throws away information the user was mid-way through acting on; keeping it silently would be a lie about its freshness.

### 5.6 Status bar

The status bar shows a compact counter — `3 errors, 12 warnings` — coloured by the highest severity present, and shows nothing when there are none.
Clicking it opens and focuses the Problems dock.

### 5.7 Keyboard and focus

Tab order: filter field → the three severity buttons → tree.
The dock's View-menu action toggles visibility and focuses the tree when showing.

Inside the tree: Up/Down through rows including group headers (unlike the settings pages — here the headers are meaningful navigation targets and are collapsible), Left/Right collapse and expand, Enter opens, `Ctrl+C` copies the selected row as `path:line:column: severity: message`, which is the form that pastes usefully into a terminal or an issue.
`Escape` in the filter field clears it; `Escape` in the tree returns focus to the editor.
`F8` and `Shift+F8` step to the next and previous diagnostic *in the editor*, wrapping within the current file, and are registered in the keymap catalog like every other binding rather than hardcoded.

### 5.8 Where this panel says nothing

No toolbar button to clear the list; diagnostics are owned by the server and a user-cleared list would refill on the next keystroke.
No severity icons in addition to the severity word.
No timestamp column.
No group header for a file with zero diagnostics after filtering.
No count badge on the dock tab title — the status bar already carries the count and two counters that can disagree is one too many.

## 6. Open questions for the user to settle

1. **The syntax-colour preview pane.** This spec says the per-row Sample column is enough and the live editor behind the dialog is the real preview. A collapsible code pane is the alternative and it costs vertical space plus a per-language sample corpus. Section 2.3 has the argument both ways.
2. **Language-server arguments as a single line.** Section 4.5. A list editor handles quoted arguments correctly and is worse in every other respect.
3. **Whether Problems opens itself on the first diagnostic of a session.** Specified as yes, once per session. Some users consider any self-opening panel an intrusion; the alternative is that the status-bar counter is the only discovery path.
4. **Severity colour values.** The table in section 1 is picked for contrast, not for beauty, and the vscode-dark set deliberately does not match VS Code's own `#f14c4c`, which measures 4.34:1 against `#252526` and fails AA. If matching VS Code exactly matters more than the last 0.16 of contrast ratio, that is a decision to make explicitly rather than by accident.
5. **Where the Languages page's `Add Language...` copies to.** This spec assumes a per-user config directory managed by the app. If languages should also be addable per project, that is a different feature with its own precedence rules and it is not specified here.
