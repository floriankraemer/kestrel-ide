; Blade injections (ADR-0071), from tree-sitter-blade's own
; `queries/injections.scm` (MIT, commit b5291d1) minus the `@envoy` Bash
; pattern, whose `#has-ancestor?` predicate tree-sitter cannot evaluate.
; The HTML is parsed by the Blade grammar itself, so only PHP and the
; JavaScript in Livewire/Alpine attributes are injected.

((php_only) @injection.content
  (#set! injection.language "php_only"))

((parameter) @injection.content
  (#set! injection.include-children)
  (#set! injection.language "php_only"))

; Livewire attributes: <div wire:click="baz++">
(attribute
  (attribute_name) @_attr
  (#any-of? @_attr "wire:model" "wire:click" "wire:stream" "wire:text" "wire:show")
  (quoted_attribute_value
    (attribute_value) @injection.content)
  (#set! injection.language "javascript"))

; AlpineJS attributes: <div x-data="{ foo: 'bar' }" x-init="baz()">
(attribute
  (attribute_name) @_attr
  (#match? @_attr "^x-[a-z]+")
  (#not-any-of? @_attr "x-teleport" "x-ref" "x-transition")
  (quoted_attribute_value
    (attribute_value) @injection.content)
  (#set! injection.language "javascript"))

; <div :foo="bar" @click="baz()">
(attribute
  (attribute_name) @_attr
  (#match? @_attr "^[:@][a-z]+")
  (quoted_attribute_value
    (attribute_value) @injection.content)
  (#set! injection.language "javascript"))
