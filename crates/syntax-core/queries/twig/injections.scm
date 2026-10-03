; Twig injections (ADR-0071): everything outside `{{ }}`, `{% %}` and `{# #}`
; is the template's output language, HTML here. `injection.combined` parses
; the separate `(content)` runs as one document, so a tag opened before a
; `{% if %}` and closed after it still pairs up.

((content) @injection.content
  (#set! injection.language "html")
  (#set! injection.combined))
