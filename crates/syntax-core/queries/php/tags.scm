; Outline extraction query for Task D (`outline()`), PHP. Same
; `tree-sitter-tags` convention as rust/tags.scm: whole definition node as
; `@definition.<kind>`, its identifier as `@name`, nesting by byte-range
; containment in lib.rs.
;
; PHP's `name` nodes for a `property_element`'s `$foo` are `(variable_name)`
; itself (not an inner `name`, which — as in locals.scm — would double-
; capture), matching the convention already established in php/locals.scm.
; `trait_declaration` maps onto `SymbolKind::Interface`: PHP traits are a
; distinct concept from interfaces, but `SymbolKind` has no `Trait` variant
; and "a named container of methods with no state of its own" is closer to
; an interface than to a class.

(class_declaration name: (name) @name) @definition.class
(interface_declaration name: (name) @name) @definition.interface
(trait_declaration name: (name) @name) @definition.interface
(enum_declaration name: (name) @name) @definition.enum
(method_declaration name: (name) @name) @definition.method
(function_definition name: (name) @name) @definition.function
(property_declaration
  (property_element name: (variable_name) @name)) @definition.property
(const_element (name) @name) @definition.constant
(enum_case name: (name) @name) @definition.enum_member
