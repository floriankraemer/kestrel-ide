; Groovy highlights.scm — hand-written against tree-sitter-groovy 0.1.2,
; which (like kotlin-ng) ships no `queries/` directory of its own (its
; HIGHLIGHTS_QUERY etc. are commented out in the bindings crate). The
; grammar is a fork of tree-sitter-java's, so its node-types.json is close
; enough to Java's that most of queries/java/highlights.scm carries over
; node-for-node; this file adds Groovy's own `def`, closures, spread and
; GString interpolation on top.
;
; Same two rules as every other hand-written file in this catalog: no
; `#match?` predicates in the body, no catch-all `(identifier) @variable`.

(line_comment) @comment
(block_comment) @comment

(character_literal) @string
(string_literal) @string
(string_fragment) @string
(multiline_string_fragment) @string

[
  (hex_integer_literal)
  (decimal_integer_literal)
  (octal_integer_literal)
  (binary_integer_literal)
  (decimal_floating_point_literal)
  (hex_floating_point_literal)
] @number

(type_identifier) @type
(class_declaration name: (identifier) @type)
(interface_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(record_declaration name: (identifier) @type)
(annotation_type_declaration name: (identifier) @type)
(constructor_declaration name: (identifier) @type)
(compact_constructor_declaration name: (identifier) @type)

[
  (boolean_type)
  (integral_type)
  (floating_point_type)
  (void_type)
] @type

(method_declaration name: (identifier) @function)
(method_invocation name: (identifier) @function)
(juxt_function_call name: (identifier) @function)

[
  "class"
  "interface"
  "enum"
  "record"
  "extends"
  "implements"
  "permits"
  "return"
  "new"
  "if"
  "else"
  "for"
  "while"
  "do"
  "switch"
  "case"
  "default"
  "break"
  "continue"
  "try"
  "catch"
  "finally"
  "throw"
  "throws"
  "import"
  "package"
  "static"
  "public"
  "private"
  "protected"
  "final"
  "abstract"
  "synchronized"
  "instanceof"
  "assert"
  "yield"
  "as"
  "in"
  "def"
] @keyword

[
  (this)
  (super)
] @keyword
