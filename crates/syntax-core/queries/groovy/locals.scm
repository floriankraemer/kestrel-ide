; Identifier-occurrence query for A2 (`identifier_occurrences`), Groovy.
; Adapted from queries/java/locals.scm — the grammar is a Java fork with
; the same declaration-node shape.

(class_declaration name: (identifier) @definition)
(interface_declaration name: (identifier) @definition)
(enum_declaration name: (identifier) @definition)
(record_declaration name: (identifier) @definition)
(constructor_declaration name: (identifier) @definition)
(method_declaration name: (identifier) @definition)
(formal_parameter name: (identifier) @definition)
(variable_declarator name: (identifier) @definition)

(identifier) @reference
; Type usages (`extends Foo`, a field's type, `new Foo()`) are
; `type_identifier` nodes, not `identifier` — without this catch-all they
; are not occurrences at all.
(type_identifier) @reference
