# 0072. Live templates, file templates and code generation

## Status

Accepted.
Delivered by phase G of the [PHP parity plan](../php-parity-plan.md).
Extends [ADR-0033](0033-markdown-preview.md)'s additive contribution points; reuses the snippet grammar and session of the LSP snippet completions.

## Context

The editor could neither expand an abbreviation, wrap a selection in a construct, create a file from a template nor generate boilerplate.
For PHP these are daily work: `fore` and `$xs.foreach`, New > PHP Class in the right namespace, a constructor and accessors for a class's properties.
Phpactor can generate some of this, but it does not run on Windows, so the basics must not depend on a server.

## Decision

**A `live-templates` contribution point (`api_version` stays 1).**
Fields: `language`, `abbreviation`, `description`, `body`, `postfix`, `context`.
- `body` is an LSP snippet (`$1`, `${1:default}`, `$0`) plus two variables: `$SELECTION$` for surround templates and `$EXPR$` for postfix ones.
  A literal dollar is written `\$`, which is how a PHP variable appears in a body.
- A postfix body must contain `$EXPR$`.
- `context` is `any`, `statement`, `expression` or `class`; the shipped PHP templates use `statement` and `class`.
- A plugin's templates are unique per `(language, abbreviation, postfix)`.
- The user's `[[live_template]]` rows in `settings.toml` replace a plugin template with the same key or add a new one (`settings_model::live_templates::resolve`).
  A row with no language, abbreviation or body is ignored.

**`edit_ops::templates` turns a template into an edit.**
Each entry point is a pure function over the text and returns one edit plus the tab stops, as absolute byte offsets of the edited text.
- `expand` replaces the abbreviation before the caret, `surround` wraps a selection (widened to whole lines when it spans several), `postfix` replaces `expr.abbr`.
- The postfix expression is the longest chain of member accesses, calls and subscripts that ends right before the dot, found on the tree-sitter parse of the text before the dot.
  `$a->b()->c.foreach` takes the whole chain, `$x = $items.foreach` takes `$items`.
  The chain is recognised by substrings of the grammar's node names, so a grammar that names them differently only loses postfix templates.
- Substituted text is escaped before the snippet is parsed, so code containing `$` or `}` survives.
- Leading tabs in a body become the language's indent unit, and every later line takes the indentation of the line the template starts on.
- When the body has tab stops and no `$0`, one is appended, so the caret ends after the construct.
- The place a template stands at (`Site`: statement, expression, class body, other) is judged on the parse with the abbreviation swapped for a plain identifier, so a half-typed word cannot distort the tree.

**One snippet mechanism.**
The editor already starts a snippet session when an LSP snippet completion is accepted.
Template expansion starts the same session over its stops (`EditorOps::expandTemplate`), and a postfix template is offered in the completion popup as an ordinary snippet item whose range covers `expr.abbr`, so accepting it runs the existing accept path.
Rust owns every decision; the view applies edits and selects the first stop.

**Where a template is offered.**
- Tab expands the postfix template named after a dot, else the plain template named by the word before the caret, when it fits the place.
  A word that is a variable (`$if`) or a member (`->if`, `::if`) is never an abbreviation.
  Otherwise Tab indents as before.
- Ctrl+J lists the plain templates that fit the caret's place; Ctrl+Alt+T lists the templates that contain `$SELECTION$`.
- The completion popup lists the postfix templates after `expr.`, and the plain template whose abbreviation is exactly the typed word, first.
  Plain templates are not listed by prefix, because they would sit above the language's own keywords (`if`, `fn`) in every popup.
- Ctrl+Alt+T was `view.projectTree`'s default; the Project window moves to Alt+1.
- A shortcut a release adds as a default can collide with a key the user already bound.
  `Keymap` resolves it at load time: the persisted override keeps its key and the clashing default is left unbound, so no key triggers two actions (the new defaults Ctrl+N, Ctrl+J, Ctrl+Shift+B, Alt+1, Ctrl+Alt+Insert, Alt+Insert and Ctrl+Alt+T are covered by a test).

**A `file-templates` contribution point.**
Fields: `id`, `name`, `language`, `extension`, `name-suffix`, `body`.
- `body` may use `${NAME}`, `${NAMESPACE}`, `${DATE}` and `${YEAR}`.
  With an empty namespace the lines that mention `${NAMESPACE}` and the blank line after them are dropped, so a file outside every autoload root has no `namespace ;`.
- `name-suffix` is appended to the entered name unless it already ends with it (`Foo` becomes `FooTest` for the PHPUnit template).
- A name that goes into code (`${NAME}`) must be an identifier.
- `php_core::psr4::namespace_for` picks the PSR-4 root (from `autoload` and `autoload-dev`, several roots per prefix allowed) whose directory is the longest prefix of the target directory, then appends the sub-directories.
  No root means an empty namespace.
- `php-tools` ships PHP Class, Interface, Trait, Enum, PHPUnit Test and PHP File.
- The New menu lists a template only for a language the project uses (`settings_model::file_templates::is_offered`): the language's marker file exists (`composer.json` for PHP) or the project holds a file with the template's extension (`project_model::contains_extension`, a depth-limited probe that honours `.gitignore`, so `vendor/` does not count).
  A language with no known marker is always offered.

**New in the tree and the File menu.**
New > File, Directory and one entry per file template, in the project tree's context menu and at the top of the File menu; the target is the selected folder, or the project root.
`project_model` now validates every entry name (not empty, not `.` or `..`, no path separator, no control character) for create and rename, and creates a file with `create_new`, so an existing file is never overwritten.
`app_core::AppSession::create_file_with` is the one way a rendered template reaches the disk; the new file opens in a tab.

**Generate (Alt+Insert) without a server.**
`php_core::generate` reads the class around the caret from the tree-sitter parse and produces one insertion.
- Constructor: the instance properties that are neither initialized nor promoted; refused when the class already has one.
- Getters, Setters, Getters and Setters: static properties are skipped, readonly ones get no setter, a method that already exists (case-insensitively) is never generated again.
  Names are camel-cased (`user_id` gives `getUserId`); a `bool` property gets `isFoo`, or keeps an existing `isFoo` name, and its setter drops the `is`.
  Types, including nullable and union types, are copied to parameters and return types.
- The menu also lists the language servers' `source.*` and `refactor.*` code actions, taken from the intentions that `LanguageService` already merges across servers, so a server's own "implement" or "generate" actions appear without new routing.
  With no server the local entries show after 300 ms.
- A picker lists the properties as checkboxes, all checked.
- Alt+Insert was `database.addRow`'s default; the grid's Add Row moves to Ctrl+Alt+Insert.

## Consequences

- Templates, file templates and generators are Qt-free and unit-tested (`edit-ops`, `settings-model`, `php-core`, `plugin-api`); the C++ only builds menus and dialogs.
- Other languages get live templates and New entries by contributing rows; only the namespace rule is PHP's.
- No crate gains a dependency: `settings-model` already depended on `edit-ops` and `plugin-host`, and `ui-shell` already on `php-core`.
- A template cannot yet name a `$VAR` other than the two above; `${…}` variables for file templates are fixed.
- The postfix expression heuristic is purposely small: operators and assignments end the chain, `new Foo().x` forms are not special-cased.
- Not built: promoted-property constructors, generating `__toString`, `equals` or `implement methods` locally (a server's actions cover the last), live-template editing UI (rows are edited in `settings.toml`).
