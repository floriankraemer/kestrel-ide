; Twig folds. The grammar is flat — `{% if %}` and `{% endif %}` are sibling
; directives — so the block pairs are folded by `syntax_core::folds`
; (`twig_block_folds`), not here. What the tree does nest is data.

(hash) @fold
(array) @fold
(arguments) @fold
(parameters) @fold
