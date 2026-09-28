# LSP conformance: checking the client against a real server

Every other test of `lsp-core` runs against `stub_server`, which is deterministic and can be told to misbehave on cue.
That is the right tool for testing the client's own failure paths — framing, version counters, out-of-order responses, a server that dies mid-session — and the wrong one for testing our *assumptions* about the protocol.
A stub answers the way we think a server answers, so a shared misunderstanding between the stub and the client stays invisible forever.

This suite closes that gap.

## Running it

```sh
make lsp-conformance
```

It builds the `lsp-conformance` Docker stage (`linux-builder` plus pinned `rust-analyzer` and `csharp-ls` binaries) and runs `crates/lsp-core/tests/conformance/real_server_conformance.rs` and `crates/lsp-core/tests/conformance/csharp_conformance.rs`.
The tests are `#[ignore]`d, so `cargo test --workspace` and every per-PR CI run are unaffected.

## The report is executable

`crates/lsp-core/tests/data/conformance-expectations.toml` records what a real server actually does, and the suite asserts against it.

This is deliberate. A prose document listing which server supports what rots within a fortnight and nobody notices, because nothing checks it.
A file the suite diffs against cannot silently disagree with reality: when it drifts, the run fails and names the difference.

When it does fail, one of two things happened — our client changed what it asks for, or the server changed what it answers.
Both deserve a human decision, which is why re-recording is explicit:

```sh
CONFORMANCE_BLESS=1 make lsp-conformance
```

The regenerated diff is what gets reviewed in the pull request.

## Why it is not a per-PR gate

It needs a separate image, takes minutes, and can go red because upstream changed rather than because we did.
A red CI that is nobody's fault is exactly how a suite like this gets ignored, then disabled, then deleted.
Nightly and on demand.

## Why rust-analyzer only, for now

One real server is enough to exercise the two things the client had never had checked: UTF-16 position encoding against non-ASCII text, and the shape of a genuine `textDocument/codeAction` reply.

pyright and clangd disagree with rust-analyzer in useful ways — a Node runtime, a different position-encoding history, `compile_commands.json`, heavier use of `codeAction/resolve` — and each should be added when a feature actually depends on that divergence, rather than up front.

The version is **pinned**.
An unpinned language server turns a conformance suite into a random number generator, and "upstream changed its `codeAction` shape" arriving as a red build on an unrelated pull request is how the suite stops being trusted.
Bumping the pin is a deliberate commit.

## Why csharp-ls crossed the bar

"For now" above meant: add a second server only when a feature actually depends on the divergence, not up front.
C# is the first feature that did.

The C# language-servers work taught this client two protocol behaviors no server had ever exercised for real: dynamic capability registration (`client/registerCapability`, answered by the stub since it was built) and pulled configuration (`workspace/configuration`, likewise stub-only).
Both were built and tested entirely against `stub_server`, which answers the way the client's author assumed a server would.
csharp-ls is the first real server this client talks to that actually uses both paths — it declares most of its capabilities dynamically after `initialized` rather than statically in the `initialize` result, and it pulls its settings rather than accepting a push.
That is exactly the shared-misunderstanding risk this suite exists to catch, so it earned its own conformance target: `crates/lsp-core/tests/conformance/csharp_conformance.rs`, in the same `lsp-conformance` Docker stage and the same `make lsp-conformance` run as rust-analyzer's suite.

It covers only what depended on that divergence — the `initialize`/`ServerReady` handshake, that csharp-ls registers at least one method dynamically, `textDocument/completion` and, where advertised, `completionItem/resolve` (the `using`-insertion round trip), and `textDocument/hover` at a position after the fixture's multi-byte characters.
It does not attempt everything csharp-ls can do; semantic tokens, code lens, and call/type hierarchy are separate features with their own conformance work when they land.

Both the .NET SDK and csharp-ls itself are **pinned**, for the same reason rust-analyzer's version is.
The pinned csharp-ls release ships a `net10.0`-targeted global-tool payload, which is why the stage installs the .NET 10 SDK rather than 8 — an older SDK cannot select that tool's `DotnetToolSettings.xml` and fails with a misleading error rather than a clear "wrong TFM".

## What the first run found — and how it was fixed

`ServerReady` fires as soon as `initialize` returns.
rust-analyzer accepts requests at that point but cannot answer any of them until it has run `cargo metadata` and indexed the crate — about 2–3 seconds for a one-file fixture, and far longer for a real project on a cold cache.
Until then every request returns an empty result, which is indistinguishable from "no answer exists".

So for the first seconds of a Rust project the IDE reported the server as ready and silently answered nothing: hover showed no tooltip, Go to Declaration did nothing, completion offered an empty list.
There was no `$/progress` handling in `lsp-core`, so there was nothing better to wait on.

**Fixed in F0-16.**
The client now advertises `window.workDoneProgress`, answers the `window/workDoneProgress/create` request a server sends to open a token, and tracks that server's open work in `crates/lsp-core/src/progress.rs` (`ProgressTracker`, applied on the reader thread in `manager.rs`'s `dispatch`).
Every visible change becomes an `LspEvent::ServerBusy`, carrying the server's own words for the work and its percentage when it reports one.
`crates/ui-shell/src/bridge/language/mod.rs` translates that into `LanguageService::serverBusyChanged`, and `crates/ui-shell/cpp/status_bar.cpp` shows it as a label plus a progress bar beside the project index's own — "rust-analyzer: Indexing..." with a determinate bar when there is a percentage and an indeterminate one when there is not.

The state is **advisory**: nothing waits on it, and no request is gated behind it.
A server that never sends `$/progress` — the stub, and most small servers — reads as idle from `ServerReady` onwards and behaves exactly as it did before, which is the only safe default when "no progress yet" and "no progress ever" look identical from outside.
A server that dies mid-index has the work it left open closed on its behalf by its supervisor, so the status bar never outlives the server it describes.

The regressions live with the stub, per the rule below: `stub/indexingRun` in `crates/lsp-core/src/bin/stub_server.rs` performs the full create-plus-begin/report/end sequence on demand, and `crates/lsp-core/tests/stub_server/progress.rs` drives it through indexing → idle, checks a silent server stays usable, and checks a dead one stops being busy.
The conformance suite asserts the other half — that a real rust-analyzer's progress actually reaches the client — and prints the work it named.

The suite still retries until an answer arrives, and still reports how long that took.
That is not a workaround for the defect any more: a test needs an answer whether or not the server reports progress, and progress being advisory is exactly what it has to keep tolerating.

## The division of labour

| | `stub_server/*.rs` | `conformance/real_server_conformance.rs` |
|---|---|---|
| Tests | **our client** | **our assumptions about the protocol** |
| Runs | every `cargo test --workspace` | nightly and on demand |
| Covers | framing, version counters, request/response correlation, out-of-order replies, cancellation, a server that dies mid-session, respawn and backoff, re-entrant `workspace/applyEdit` | capability shapes, position encoding, real payloads, indexing latency |
| Can do | deterministic misbehaviour on demand | nothing on demand |

A real server will not die on cue, will not answer out of order to order, and will not send a malformed response.
The stub is the only way to test the failure paths, and the failure paths are where clients break.

**The rule that ties them together:** every bug the conformance suite finds gets a stub regression test in the same change as the fix.
The nightly run finds it once; the stub catches it forever, in the two-second loop.
Without that rule the stub decays into a legacy fixture and the nightly becomes a 24-hour feedback loop on a client bug.
