; Blade folds: the grammar nests every directive pair and every element, so
; each of them folds as one node. Multi-line comments fold through the
; generic comment rule in `syntax_core::folds`.

(section) @fold
(loop) @fold
(conditional) @fold
(switch) @fold
(once) @fold
(stack) @fold
(verbatim) @fold
(livewire) @fold
(php_statement (directive_start)) @fold
(element) @fold
(script_element) @fold
(style_element) @fold
