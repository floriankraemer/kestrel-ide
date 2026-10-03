; Foldable regions (Task C): class/interface/trait/enum bodies
; (`declaration_list`), and function/method/if/for/... bodies
; (`compound_statement`).
(compound_statement) @fold
(declaration_list) @fold
; Y5: enum bodies, `switch` and `match` bodies, array literals, `use A\{B, C}`
; groups, heredoc/nowdoc strings, attribute lists, and parameter and argument
; lists that span lines. Multi-line doc comments fold through the generic
; comment rule in `syntax_core::folds`.
(enum_declaration_list) @fold
(switch_block) @fold
(match_block) @fold
(array_creation_expression) @fold
(namespace_use_group) @fold
(heredoc) @fold
(nowdoc) @fold
(attribute_list) @fold
(formal_parameters) @fold
(arguments) @fold
