# Markdown and Mermaid preview plan

A Markdown preview with inline Mermaid diagrams, delivered as the plugin host's third contribution point — and its first executable-tier consumer that returns content.

Architecture decision: [ADR-0033](decisions/0033-markdown-preview.md).
Builds on [ADR-0026](decisions/0026-plugin-host.md) (the host), [ADR-0027](decisions/0027-icon-themes.md) (the `app-core` join and the `resvg` rasteriser), [ADR-0028](decisions/0028-wasm-plugin-tier.md) (the sandbox tier), [ADR-0021](decisions/0021-ai-chat.md) (untrusted text in a `QTextBrowser`, and the off-thread `CxxQtThread::queue()` pattern).

## Why

Markdown is a first-class file type in this repository — every ADR, plan and design doc is one, several carry ```mermaid fences, and `CLAUDE.md` asks for one sentence per line, which makes the raw source deliberately hard to read as prose.
`syntax-core` registers `md`/`markdown`/`mdown`/`mkd` for highlighting and that is the end of it: there was no preview, no renderer crate anywhere in the dependency tree, and a `.png` still opens as hex.

Separately, the plugin host had a credibility gap.
`icon-themes` had a real consumer; `commands` had none — the palette's action list is a static table nothing merges a plugin's contributions into — and the executable tier's WIT world could not return content at all, only `result<_, string>`.
This plan closes both: Markdown preview ships as the third contribution point, with a built-in native provider and a sandboxed one a third party can also offer, proving the executable tier end to end rather than by assertion.

## Scope decisions

See [ADR-0033](decisions/0033-markdown-preview.md) for the full reasoning behind each of these; summarised here for the Progress table's context.

- **comrak** parses and renders; `render.r#unsafe` stays `false`, always.
- **merman**, pinned exactly to `=0.7.0-alpha.1` across its whole crate family, lays a diagram out to SVG — the `0.8` line needs a newer `rustc` than this repository carries.
- **resvg** rasterises, reached only through its own `usvg`/`tiny_skia` re-exports, never a direct dependency on either.
- **Liberation Sans** (OFL-1.1) is bundled, not resolved from the host's fonts — byte-identical output everywhere, testable by an E2E harness.
- The preview is an ADS dock, not a new `TabKind`; rendering runs off the Qt thread with a per-tab revision that drops a stale result.
- `api_version` stays `1`: a real bug was found and fixed to make that hold (`Contributes` had `deny_unknown_fields`, contradicting ADR-0026's own claim about an unrecognised point), and the wasm world gained a second, additive `world` rather than a fourth export on the existing one.

## Progress

Living status table — update the relevant row **in the same commit** that finishes a task, so status and code never drift apart.

| Task | Status | Commit |
|---|---|---|
| M0 — spike: `merman` + `comrak` + `resvg/text`, native and through MXE; render budget | done | throwaway, results in this doc's history |
| M1 — `plugin-api`: the `previews` point, the `preview-plugin` world, ADR-0033, this plan | done | 652209c |
| M2 — `plugin-host`: `previews()` accessor, the built-in, the preview world in `WasmTier` | done | 45f485a |
| M3+M4 — `markdown-preview`: comrak → Qt-subset HTML, highlighted fences, anchors, links, `merman` → SVG → RGBA, bundled font, diagram cache | done | 0c2efaa |
| M5 — `app-core`: the `previews` join, provider resolution | done | 1f85e71 |
| M6+M7 — FFI seam, Preview dock, debounce, scroll sync, click-to-jump, link policy | done | 84c0a83 |
| M8 — settings: the Plugins page row, the example wasm preview plugin | done | d175b17 |
| M9 — docs truth-up and the end-to-end pass | done | this commit |

Lanes actually run: `{M1→M2}` and `{M3→M5}` converged at M6; M8 needed only M2.
M3 and M4 landed as one commit rather than two — the crate did not exist until M3, and every M4 file depends on types M3 defines in the same crate, so splitting the commit would have meant temporarily stripping declared-but-unused dependencies out of `Cargo.toml` for no reader's benefit. M6 and M7 landed together for the matching reason: both live in the same two view files (`bridge/preview.rs`, `markdown_preview_panel.{h,cpp}`), and the scroll-sync/link-policy code in M7 was written and verified alongside the dock it has no meaning without.

### M0 result

Run inside `linux-builder` against the shared cargo registry volume, throwaway crate, nothing merged.

**merman is pinned to `=0.7.0-alpha.1`, not `0.8.0-alpha.x`.** Every crate in the `0.8` line (`merman`, `merman-core`, `merman-render`, `dugong`, `dugong-graphlib`, `manatee`) declares `rust-version = 1.95`; this repository's toolchain is `1.90.0`, and a bump is a separate decision this plan does not make. `0.7.0-alpha.1` of the same five crates declares `rust-version = 1.87` and builds clean on `1.90.0` — but only when **all five are pinned to that exact version**; leaving any one unpinned lets the resolver pull its newer, `1.95`-only sibling in. The public API used is `merman::render::HeadlessRenderer::new().with_diagram_id(id).render_svg_with_pipeline_sync(source, &SvgPipeline::resvg_safe())`.

**`resvg`'s `text` feature must be the only way `usvg`/`tiny-skia` enter the tree — no direct dependency on either.** `icon-theme/Cargo.toml`'s comment warns about exactly this, and the spike reproduced the trap by hand: adding `usvg` as a direct dependency pulled its default features back in, and `memmap2` + `fontconfig-parser` reappeared despite `resvg`'s own `default-features = false`. Reaching both only through `resvg::usvg` / `resvg::tiny_skia` made both disappear.

**Render speed is not a concern.** This repository's own `overview.md` mermaid fence (`graph TB`/`subgraph`/`<br/>` inside node labels — a genuinely demanding fixture): ~7–18 ms per render. Three back-to-back simple diagrams: ~17 ms total.

**Text does not render without work.** merman's SVG sets `font-family` per diagram type in different places — inline per `<text style="...">` for a flowchart, once in a `<style>#id{font-family:"...";}` block for a sequence diagram, quoted. A `fontdb` loaded with only the bundled font renders nothing for either kind until the SVG's `font-family` value is rewritten: strip a quoted segment first, then replace whatever remains up to the next `;` or the attribute's closing quote. Both passes are needed; either alone leaves one diagram type blank.

**Text overflows its box on the flowchart fixture** — visible but expected: merman lays out box widths with its own font-agnostic measurer, so a substituted font's real metrics do not always fit. Legible, not fixed for v1; the escape hatch is a `TextMeasurer` implementation over `ttf-parser` glyph advances against the already-loaded face, ~40 lines, not needed yet.

**The MXE cross-link was retired for real in M2**, not just judged low-risk in the spike: `cargo test -p markdown-preview --target x86_64-pc-windows-gnu --no-run` compiled and linked two `.exe` test binaries through `ide-windows-builder`.

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
make e2e                                               # crates/app/tests/e2e/core.rs::e2e_markdown_preview_dock
cargo tree -p markdown-preview -e normal | grep -i qt  # must be empty
cargo tree -p app-core         -e normal | grep -i qt  # must be empty
cargo tree -p plugin-api       -e normal | grep -i qt  # must be empty
```

`e2e_markdown_preview_dock` (`crates/app/tests/fixtures/markdown/`) opens a Markdown file, toggles the Preview dock, waits for the first render, edits the buffer, and waits for a strictly later revision — every assertion against the marker stream, never a screenshot, per this repository's own E2E rule. It caught two real bugs before landing: `Ctrl+Alt+M`'s default shortcut collided with Extract Method's, and the test itself left a dirty tab across `Ctrl+Q`, hanging on the unhandled "save before closing?" dialog.

## Risks, and how each was retired

| Risk | Retired by |
|---|---|
| `merman` is alpha and may change API or regress on a bump | Pinned exactly, not a range; the renderer is behind a plugin, so a failure costs one dock. |
| `resvg/text` may not cross-build/link through MXE | M2's real `--target x86_64-pc-windows-gnu --no-run` link, not just `cargo check`. |
| `usvg`/`tiny-skia` as direct dependencies silently reintroduce `memmap2`/`fontconfig-parser` | Confirmed to happen in M0 — reached only through `resvg`'s re-export from M3 onward; the standing check is `cargo tree -p markdown-preview -e normal \| grep -iE "memmap\|fontconfig"` empty. |
| merman's font-agnostic measurer vs. the bundled font ⇒ text overflowing its boxes | Confirmed present but legible; `TextMeasurer` is the documented escape hatch if it worsens. |
| `Contributes`'s `deny_unknown_fields` made ADR-0026's "unknown point is ignored" claim false | Found while implementing M1, fixed with a flattened unknown-keys field and a test. |
| A new WIT export breaking every existing component | Two worlds + `include`, confirmed by `hello-plugin` still compiling unchanged after the WIT file changed, and by a real compiled `preview-plugin` component instantiating and running through `WasmTier` in M8. |
| A wasm preview provider hanging or blowing memory | The same fuel/epoch/memory limits ADR-0028 already enforces for `on-command`; no new limit needed for `render`. |
| Remote resource loading / script injection from an untrusted `.md` | `render.r#unsafe` stays `false` (M3); every external link scheme is refused, never opened (M7). |
