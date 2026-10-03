; Blade highlights.scm: tree-sitter-html 0.23.2's highlights (the grammar
; parses the markup around the directives itself, and EmranMR/tree-sitter-blade
; declares `; inherits: html`) followed by tree-sitter-blade's own
; `queries/highlights.scm` (MIT, commit b5291d1). Inlined because this
; catalog has no query inheritance between rows.

(tag_name) @tag
(erroneous_end_tag_name) @tag.error
(doctype) @constant
(attribute_name) @attribute
(attribute_value) @string
(comment) @comment

[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket


; Upstream colours the directives as `@tag`, the same as HTML elements;
; `@keyword` is what a reader expects of `@foreach` and `@endif`.
[
  (directive)
  (directive_start)
  (directive_end)
] @keyword

[
  (php_tag)
  (php_end_tag)
  "{{"
  "}}"
  "{!!"
  "!!}"
  "("
  ")"
] @punctuation.bracket
