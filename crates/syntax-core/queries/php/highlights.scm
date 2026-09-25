; PHP highlights.scm — targets the `php_only` grammar (body-only, no
; embedded-HTML/`<?php` tag nodes; see Cargo.toml/lib.rs comments for why).
; Same capture-name convention as rust/json (@keyword, @string, @comment,
; @number, @function, @type). Trimmed from tree-sitter-php's own shipped
; queries/highlights.scm down to the six TokenKind captures this crate
; understands.
;
; PHP's grammar names classes/functions/methods/variables all as `(name)`
; nodes in different wrapper positions (`variable_name` for `$foo`, plain
; `name` for a class/function/method identifier) rather than distinct
; `identifier`/`type_identifier` node kinds like Rust/C#/Java — captures
; below key off the wrapping field, not the leaf node kind.

(comment) @comment

[
  (string)
  (encapsed_string)
] @string

(integer) @number
(float) @number

(primitive_type) @type
(class_declaration name: (name) @type)
(interface_declaration name: (name) @type)
(trait_declaration name: (name) @type)
(enum_declaration name: (name) @type)
(object_creation_expression (name) @type)

(function_definition name: (name) @function)
(method_declaration name: (name) @function)
(function_call_expression function: (name) @function)
(member_call_expression name: (name) @function)

[
  (visibility_modifier)
  (static_modifier)
  (abstract_modifier)
  (final_modifier)
  (readonly_modifier)
] @keyword

[
  "class"
  "function"
  "fn"
  "interface"
  "trait"
  "extends"
  "implements"
  "if"
  "else"
  "elseif"
  "for"
  "foreach"
  "while"
  "do"
  "switch"
  "case"
  "default"
  "break"
  "continue"
  "return"
  "try"
  "catch"
  "finally"
  "throw"
  "new"
  "echo"
  "print"
  "use"
  "namespace"
  "const"
  "global"
  "instanceof"
  "match"
  "yield"
  "as"
  "and"
  "or"
  "xor"
] @keyword
