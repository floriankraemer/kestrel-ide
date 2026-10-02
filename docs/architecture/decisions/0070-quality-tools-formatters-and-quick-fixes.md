# 0070. Quality tools: Psalm, PHPMD, formatters and analyzer quick fixes

## Status

Accepted.
Delivered by phase Q of the [PHP parity plan](../php-parity-plan.md).
Extends [ADR-0033](0033-markdown-preview.md)'s additive contribution points and [ADR-0047](0047-analyzers-contribution-point.md); uses [ADR-0067](0067-container-exec-host.md)'s host-aware runs.

## Context

PHPStan and PHPCS ran live, but nothing else did.
Psalm and PHPMD were missing, and there was no way to format a file with php-cs-fixer, Pint or phpcbf.
A finding could only be read, never silenced or fixed from the editor.
Three facts shaped the design:
- A formatter returns the whole file, while an editor wants a few small edits that keep the caret and form one undo step.
- Tools disagree on how they take input (stdin, or a file they rewrite in place) and on what a non-zero exit means (phpcbf exits 1 when it fixed something).
- The three analyzers do not report a rule id the same way (`source` for PHPStan and PHPCS, a message prefix for Psalm).

## Decision

**More analyzers, same contribution point.**
Psalm and PHPMD are `analyzers` rows of the `php-tools` plugin.
PHPMD's argv is positional (`<path> <format> <ruleset>`), so an analyzer's `args` may now place the target with `{file}`; for a project run the placeholder is the project root, and without it the root is still appended.
PHPMD's ruleset is the `{ruleset}` placeholder: the project's `phpmd.xml` or `phpmd.xml.dist` (an analyzer's `config-file-candidates`) when one exists at the project root, else the built-in rule sets (`ruleset-default`).
It excludes `vendor` and `node_modules`.

**A `formatters` contribution point (`api_version` stays 1).**
Fields: `id`, `name`, `languages`, `program-candidates`, `args`, `buffer`, `success-exit-codes`, `config-file-candidates`, `composer-package`, `requires-interpreter`, `fix-args`.
- `buffer = "temp-copy"` (the default) names the file in `args` with `{file}`: the tool rewrites a dotfile copy beside the original and the copy is read back.
  The copy keeps the original's extension, because php-cs-fixer and Pint select files by extension.
- `buffer = "stdin"` sends the buffer on stdin and takes the formatted text from stdout.
  An empty stdout for a non-empty buffer is an error, never an empty file.
- `success-exit-codes` defaults to `[0]`; phpcbf lists `[0, 1, 2]` because all three print the resulting source.
- `analysis_core::format` runs one formatter through `process_exec::run_on` on the interpreter's host and returns the new text.
  `format_on_save` and Reformat Code both call it; errors are typed (`NotInstalled`, `TimedOut`, `Failed`, `BadOutput`).

**Choosing a formatter (`settings_model::formatting::plan`).**
The configured `[php].formatter` wins when it names a contribution that lists the language and the program resolves, otherwise the language server formats, as before.
A tool never formats a selection: Reformat with a selection keeps using the server's `rangeFormatting`, because a formatter rewrites lines the user did not select.
A configured tool that turns out not to be installed falls back to the server silently.

**Applying the result as edits.**
`lsp_core::edits_between(old, new)` diffs by lines (`editor_core::diff`) and returns one edit per changed run, last first.
The edits go through the pending-edit path every reformat already uses, so Ctrl+Z undoes the whole reformat.
A diff over `MAX_DIFF_BYTES` falls back to one whole-text edit.

**Format on save.**
`[editing] format_on_save` is a per-language tri-state (off by default).
With a tool formatter the tool runs; with none the language server formats instead (`textDocument/formatting`, after sending it the buffer's current text), under the same limit.
A language with no running server is not an error and the save goes ahead as it is.
The work runs with a 10 second limit, before the trim and final-newline rules, and both end up as one diff against the buffer.
A failing or timed-out formatter does not keep the file from being saved: the file is saved unformatted and the view shows a notice naming the tool and its message.

**Quick fixes for findings.**
- `Diagnostic` gains `code: Option<String>`, the rule id for sources with no raw LSP payload.
  PHPStan and PHPCS give it as the checkstyle `source`; an analyzer with `code-in-message = true` (Psalm) gets it from the `Id: text` prefix.
- An analyzer's `suppress-comment` template (`// @phpstan-ignore {code}`, `/** @psalm-suppress {code} */`, `// phpcs:ignore {code}`) becomes a "Suppress" quick fix.
  `analysis_core::suppress::suppress_insertion` is the pure rule: the comment goes on its own line above the finding, with the finding's indentation and line ending.
  Suppressing a second rule on the same line adds a second comment line rather than extending the first.
- An analyzer's `fixer` names a formatter whose `fix-args` narrow it to one rule (`--sniffs={sniff}`, the first three dot-separated parts of the PHPCS source).
  That makes "Fix with phpcbf": the fixer runs over the buffer and the result is applied as one undo step.
- Both appear in Alt+Enter and the hover card next to the language servers' code actions, as synthesised quick fixes the ordinary apply path handles.

## Consequences

- Reformat, format-on-save and the quick fixes work in a container or on WSL for free, because they use `run_on` with the interpreter's host.
- A formatter that only works on files (php-cs-fixer, Pint) costs a temp file per run; the copy is deleted when the run ends and matches the existing gitignore pattern.
- Format on save blocks the editor for up to 10 seconds in the worst case.
  Making it asynchronous would need the save itself to become asynchronous; it was not worth that for a tool that normally answers in well under a second.
- The save waits for a language server's answer at most as long as for a tool, and goes ahead unformatted when it is late.
- Checked against the real tools in the nightly `php_real` flows: PHPStan's checkstyle prints paths relative to its working directory, which the IDE sets to the project root and joins back; php-cs-fixer and Pint format the dotfile temp copy; `phpcbf --sniffs` accepts a three-part sniff code and refuses the four-part message code, which is why `fix-args` uses `{sniff}`; PHPStan honours stacked `@phpstan-ignore` comments.
  The formatter argv shapes are also tested against a stub that plays the same protocol (`stub_analyzer`'s formatter mode).
