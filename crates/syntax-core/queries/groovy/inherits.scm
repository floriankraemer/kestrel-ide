; Supertype-edge query (`supertype_edges()`), Groovy. Adapted from
; queries/java/inherits.scm — the grammar is a Java fork with the same
; `superclass`/`super_interfaces`/`extends_interfaces` field shape.

(class_declaration
  name: (identifier) @type
  (superclass (type_identifier) @supertype))

(class_declaration
  name: (identifier) @type
  (super_interfaces (type_list (type_identifier) @supertype)))

(interface_declaration
  name: (identifier) @type
  (extends_interfaces (type_list (type_identifier) @supertype)))

(enum_declaration
  name: (identifier) @type
  (super_interfaces (type_list (type_identifier) @supertype)))
