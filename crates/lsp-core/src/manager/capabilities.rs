//! What this client advertises in `initialize`.

use serde_json::{json, Value};

/// What this client can do. Kept deliberately small — capabilities are added
/// by the feature tasks that implement them (L2-L5), not speculatively.
pub(super) fn client_capabilities() -> Value {
    json!({
        "textDocument": {
            "synchronization": {"dynamicRegistration": false},
            "publishDiagnostics": {"relatedInformation": true},
            // L3/L4: advertised because they are implemented — `contentFormat`
            // lists markdown first because that is what the tooltip renders,
            // and `linkSupport` opts into the richer `LocationLink` reply.
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "definition": {"linkSupport": true},
            // N1/N2: the same reply shapes as `definition`, parsed by the same
            // `parse_definition`.
            "implementation": {"linkSupport": true},
            "typeDefinition": {"linkSupport": true},
            "declaration": {"linkSupport": true},
            // R2: `snippetSupport: true` — `edit_ops::snippet` parses the
            // placeholder grammar into inserted text plus tab stops, and
            // `EditorOps`'s snippet session (ui-shell) walks them with
            // Tab/Shift+Tab, so a server offering `${1:name}`-style items is
            // no longer asked to hold back.
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["plaintext", "markdown"],
                    // C7: which fields are worth a `completionItem/resolve`
                    // round trip for — `additionalTextEdits` is the `using`
                    // csharp-ls adds for an unimported type; `documentation`
                    // and `detail` are the two fields most servers only
                    // fill in on resolve, to keep the initial list cheap.
                    "resolveSupport": {
                        "properties": ["documentation", "detail", "additionalTextEdits"],
                    },
                },
                "contextSupport": false,
            },
            // RF6: code actions as literals rather than bare commands, so an
            // action can carry its own edit; `resolveSupport` names `edit`
            // only, because that is the one field we ask a server to fill in
            // later. The kind list is the families the UI offers — servers
            // may answer with any kind, and `code_action::kind_matches`
            // classifies what arrives, so this list narrows requests without
            // limiting what can come back.
            "codeAction": {
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                    "", "quickfix", "refactor", "refactor.extract",
                    "refactor.inline", "refactor.rewrite", "source",
                    // F2: Organize Imports is offered in its own right and
                    // as a quick fix for an unresolved symbol, so the kind
                    // is named rather than left to the `source` family.
                    "source.organizeImports",
                ]}},
                "resolveSupport": {"properties": ["edit"]},
                "dataSupport": true,
                "isPreferredSupport": true,
                "disabledSupport": true,
            },
            "rename": {"prepareSupport": true},
            // F1: advertised because reformat is implemented. `dynamicRegistration`
            // is false throughout this client — a server that wants to register
            // capabilities later has nowhere to send them.
            "formatting": {"dynamicRegistration": false},
            "rangeFormatting": {"dynamicRegistration": false},
            // N5: typing a trigger character may reformat the line.
            "onTypeFormatting": {"dynamicRegistration": false},
            // F2: parameter hints. `labelOffsetSupport` says we prefer the
            // unambiguous `[start, end]` parameter label — a substring has
            // to be searched for in the signature and can match the wrong
            // occurrence — but both shapes are handled either way
            // (`signature_help::parse_signature_help`).
            // `activeParameterSupport` opts into the per-signature index,
            // which is the only way an overload set can say that *this*
            // overload takes fewer arguments.
            "signatureHelp": {
                "signatureInformation": {
                    "documentationFormat": ["plaintext", "markdown"],
                    "parameterInformation": {"labelOffsetSupport": true},
                    "activeParameterSupport": true,
                },
                "contextSupport": false,
            },
            "documentHighlight": {"dynamicRegistration": false},
            // No `resolveSupport`: hints are requested for a viewport and
            // painted whole, so there is no second round trip to opt into.
            // The `InlayHintLabelPart[]` label form needs no capability and
            // is parsed regardless.
            "inlayHint": {"dynamicRegistration": false},
            // C9: `dynamicRegistration: true` — unlike every other entry in
            // this block — because csharp-ls is believed to declare this
            // one dynamically rather than statically (see
            // `semantic_tokens` module docs); `formats: ["relative"]` is
            // the only encoding LSP 3.17 defines, so it is the only value
            // that could go here. `tokenTypes`/`tokenModifiers` are the
            // full LSP standard vocabulary this client's mapping
            // understands (`semantic_tokens::base_scope_name`); a server is
            // free to define fewer, and any it defines that this list omits
            // still decodes correctly; `requests.full: true` and no `range`
            // entry is what makes only the whole-document request offered.
            "semanticTokens": {
                "dynamicRegistration": true,
                "requests": {"full": true},
                "tokenTypes": crate::semantic_tokens::STANDARD_TOKEN_TYPES,
                "tokenModifiers": crate::semantic_tokens::STANDARD_TOKEN_MODIFIERS,
                "formats": ["relative"],
            },
            // C10: dynamic, because csharp-ls is believed to register this
            // one dynamically too, same reasoning as `semanticTokens` above.
            // No `resolveSupport`-shaped field exists for code lens in the
            // spec — a lens without a `command` always needs
            // `codeLens/resolve`, decided per item
            // (`code_lens::CodeLensItem::needs_resolve`), not by a
            // capability this client would advertise.
            "codeLens": {"dynamicRegistration": true},
            // C11: dynamic, on the same suspicion as `semanticTokens` and
            // `codeLens` above — csharp-ls is not confirmed to declare
            // either hierarchy capability statically. Neither carries a
            // resolve-style sub-capability worth advertising: an item's
            // `data` always round-trips through `incomingCalls`/
            // `outgoingCalls`/`supertypes`/`subtypes` verbatim, with no
            // separate resolve request in the spec.
            "callHierarchy": {"dynamicRegistration": true},
            "typeHierarchy": {"dynamicRegistration": true},
        },
        "workspace": {
            // RF5: we answer `workspace/applyEdit`, which is how the
            // command-driven refactorings reach us at all.
            "applyEdit": true,
            "executeCommand": {"dynamicRegistration": false},
            // N3: Go to Symbol / Go to Class ask `workspace/symbol`.
            "symbol": {"dynamicRegistration": false},
            "workspaceEdit": {
                // Versions let a stale edit be caught before it is applied.
                "documentChanges": true,
                // F2: create, rename and delete are performed by
                // `app_core::AppSession::apply_file_ops` (F2). Without
                // these advertised, rust-analyzer's "move to submodule" and
                // every extract-to-new-file refactoring is refused whole —
                // the user sees "unsupported" for a correct edit.
                "resourceOperations": ["create", "rename", "delete"],
                // We apply all of an edit or none of it.
                "failureHandling": "abort",
                "normalizesLineEndings": false,
            },
            // C4: the one capability this client dynamically registers for
            // — csharp-ls and others declare their watched-file globs this
            // way rather than up front. `relativePatternSupport: false`
            // because `Registrations::watchers` hands `globPattern` on
            // untouched to C5, which does not yet resolve a `RelativePattern`
            // against a base URI.
            "didChangeWatchedFiles": {
                "dynamicRegistration": true,
                "relativePatternSupport": false,
            },
            // C6: we answer `workspace/configuration`, which is how
            // csharp-ls (and any server that pulls rather than takes pushed
            // settings) gets its config at all.
            "configuration": true,
        },
        // F0-16: without this a server has no permission to open a progress
        // token, and rust-analyzer stays silent while it indexes — which is
        // exactly the window in which it answers every request with nothing.
        "window": {"workDoneProgress": true},
        "general": {"positionEncodings": ["utf-16"]},
    })
}
