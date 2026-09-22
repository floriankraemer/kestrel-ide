# Project structure

Where things live in the repository and where to read further.
This page is orientation only; the crate table lives in [overview.md §3](overview.md#3-building-block-view) and the binding import rules in [layering.md](layering.md).

## Repository layout

| Path | Contents |
|------|----------|
| `crates/` | The Cargo workspace; one directory per crate — see [overview.md §3](overview.md#3-building-block-view) for the full, current list. |
| `crates/ui-shell/cpp/` | The Qt Widgets humble view (C++), built alongside `ui-shell`'s Rust adapter. |
| `docs/architecture/` | Overview, layering rules, this page, and the completed plan documents. |
| `docs/architecture/decisions/` | Architecture Decision Records (ADRs), numbered. |
| `docs/design/` | UX specifications. |
| `docs/product/` | Product proposals. |
| `docker/` | The multi-stage builder image (Linux Qt6 toolchain and MXE Windows cross-toolchain). |
| `Makefile` | Entry point for all builds, tests, and lint runs inside Docker. |

## Layers

Five layers, detailed in [overview.md §3](overview.md#3-building-block-view) (the per-crate table there is the source of truth — not duplicated here, so it cannot drift out of sync) and enforced by [layering.md](layering.md):

- Domain: `editor-core`, `project-model`.
- Application: `app-core`.
- Support: everything else that is Qt-free — `app-config`, `syntax-core`, `index-core`, `lsp-core`, `settings-model`, `edit-ops`, `pty-core`, `terminal-core`, `run-core`, `mcp-server`, `container-core`, `container-registry`, `jvm-build-core`, `secret-store`, `db-core`, `db-sql`, `db-exchange`, `db-drivers`, `db-driver-adbc`, `db-driver-odbc`, and more (overview.md's table has the full list).
- Adapter + view: `ui-shell`.
- Main: `app`.

## Where tests live

Unit tests live inside each Qt-free crate next to the rules they cover, and run under plain `cargo test --workspace` with no display.
The C++ under `crates/ui-shell/cpp/` is a humble view and untested by design ([ADR-0002](decisions/0002-application-layer-and-humble-view.md)); if a C++ test feels necessary, the logic sits in the wrong layer.

## Development workflow

All builds, tests, and app runs go through Docker via the [Makefile](../../Makefile): `make linux-image`, `make test`, `make lint`, `make shell`.
The `debugging` cargo profile (`cargo build --profile debugging -p app`) adds full DWARF for stepping through the cxx-qt seam.
Task status is tracked in the Progress tables of the plan docs under `docs/architecture/`.
See [CLAUDE.md](../../CLAUDE.md) for the full agent and workflow rules.
