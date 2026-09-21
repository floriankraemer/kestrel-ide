# 0058. Database driver seam: one blocking trait, three backends, our own value model

## Status

Proposed.
Implemented by [the database-tools plan](../database-tools-plan.md); this ADR covers the seam `db-core`/`db-drivers`/`db-driver-adbc`/`db-driver-odbc` establish, landed across F1 (native foundation), F7 (NoSQL) and F8 (ADBC/ODBC).

## Context

JetBrains-parity database tooling needs to talk to a wide and structurally different set of backends: relational engines with a driver ecosystem (PostgreSQL, MySQL, SQLite, SQL Server), a long tail of analytical/warehouse engines with no native Rust driver at all (Oracle, Snowflake, BigQuery, ClickHouse, Trino, DuckDB, …), anything reachable only through a system ODBC DSN, and document/key/wide-column NoSQL stores (MongoDB, Redis, Cassandra/Scylla) whose data model is not rows and columns.
One seam has to serve all of them, from one tree, one console, one result grid and one data editor, without every consumer crate learning eight different APIs.

Four questions had to be answered before any driver code could be written.

1. **Sync or async at the seam `ui-shell` sees?**
   Every native driver crate under consideration (`tokio-postgres`, `mysql_async`, `mongodb`, `scylla`) is async; ODBC and SQLite are inherently blocking C APIs.
   ADR-0007 already settled this question for the embedded terminal and every background-work crate since (`analysis-core`, `test-core`, `container-core`, `jvm-build-core`): long work runs on its own `std::thread` and reports back through `CxxQtThread::queue()`, never on an ambient runtime the adapter has to reason about.
2. **One value model, or Arrow's `RecordBatch` as the common currency?**
   Arrow is the closest thing to an industry-standard columnar row-batch format, and `adbc_core` already speaks it.
   But seven of the eight native drivers under consideration have nothing Arrow-shaped to hand over — they would all convert into it and back out of it for no reason — and MongoDB/Redis documents (nested, variably-shaped) fit a columnar batch badly enough that the conversion would be lossy or need its own escape hatch anyway.
3. **One crate, or several?**
   `container-core` already established that a background-work crate whose HTTP client carries a private tokio runtime cannot sit beneath `run-core`/`build-core`/`dap-core`, which is why `container-registry` split off (`docs/architecture/layering.md`'s `container-registry` row).
   The same shape applies here, doubled: `db-driver-adbc`'s `reqwest` client and `db-drivers`' six-driver private runtime are two different reasons two different leaf crates need their own tokio, and neither reason implies the other's dependents need tokio at all.
4. **Which SQL Server driver?**
   `tiberius` is the only Rust-native TDS client with any adoption, but its last crates.io release is 2024-07 — this repository's "no abandoned libs" rule (the same one that ruled out `sqls` for the LSP catalog) rules it out too.

## Decision

### 1. One blocking `Connection` trait in a tokio-free `db-core`; three backend leaf crates each own their runtime

`db-core::driver::{Driver, Connection, RowStream, CancelHandle, Execution, Capabilities, ExecOptions}` is the contract every backend implements and every consumer (`ui-shell`, `db-sql`, `db-exchange`) programs against.
It is entirely synchronous: `Connection::execute` blocks the calling thread and returns once the first batch (or the whole result, for a non-`Rows` execution) is ready; `RowStream::next_batch` blocks for the next page.
`db-core` itself takes no tokio dependency — it does not run anything, it only describes the shape of a connection.

Three backend crates implement it, each on the runtime its own drivers actually need:

- `db-drivers` (native: PostgreSQL, MySQL, SQLite, MongoDB, Redis, Cassandra/Scylla) owns a private `tokio::Runtime` (`OnceLock`, 2 workers named `"db-io"`) that every async driver's calls are `block_on`'d against from the calling `std::thread`; SQLite and Redis bypass the runtime entirely, since both have synchronous APIs and forcing them through `block_on` would be pure overhead (R5 in the plan's risk table).
- `db-driver-adbc` wraps `adbc_core`/`adbc_driver_manager` and is the one place Arrow (`arrow-array`/`arrow-schema`) appears in the workspace — `arrow.rs` converts an ADBC `RecordBatch` into `db_core::RowBatch` at the crate boundary and nowhere else. Its `reqwest::blocking` client (driver downloads) carries a private tokio runtime the same way `ai-chat-core`/`container-registry` already do.
- `db-driver-odbc` wraps `odbc-api`, entirely synchronous (the ODBC C API already is), no tokio at all.

`ui-shell`'s bridge sees one `Session` (`db-core::session::Session`) per console or per source-tree connection, running on its own worker thread behind `SessionWorker` — the same "one blocking call per background operation, reported through `qt_thread().queue()`" shape `ContainerService`/`BuildToolsService` already use.

### 2. `db-core` defines its own `Value`/`RowBatch`, not Arrow

`value::Value` is a flat enum (`Null, Bool, Int, Float, Decimal(String), Text, Bytes, Date, Time, DateTime, DateTimeTz, Uuid, Json, Array, Document(Vec<(String,Value)>), Other{type_name,display}`) wide enough to represent every native driver's row *and* a MongoDB document *and* a Redis value, with `Document` and `Array` as the two recursive cases the NoSQL backends actually need and the SQL backends never produce.
Every backend converts its own wire format into this once, at the driver boundary; the result grid, the data editor, export and the ER-diagram code all program against one type regardless of which of the eight backends produced a row.
`Decimal` is stored as a `String` rather than a fixed-point type, deliberately: a driver's own arbitrary-precision decimal representation is preserved exactly as the server sent it, rather than rounded through a Rust decimal crate's own precision limits before the user ever sees the value — the same "delegate and never model" instinct `build-core` (ADR-0040) already applies to a build tool's own numbers.

Arrow stays confined to `db-driver-adbc::arrow`, converted immediately into the same `RowBatch` every other backend produces — a consumer never learns that one particular connection happened to arrive over ADBC.

### 3. Three backend crates, matching the tokio/Qt boundary each actually needs

`db-drivers` and `db-driver-adbc` carry `Qt: No` but tokio-in-some-form (a private runtime, or `reqwest::blocking`'s); `db-driver-odbc`, `db-core`, `db-sql` and `db-exchange` carry neither.
This mirrors `container-core`/`container-registry`'s split exactly, and for the identical reason: nothing beneath `run-core` may carry tokio in its tree (the `cargo tree -p run-core -e normal | grep -i tokio` gate), and none of `db-drivers`/`db-driver-adbc`/`db-driver-odbc` sits beneath `run-core` at all — `run-core` gains no `db-*` edge (a `sql-script` run configuration is a kind tag `RunService` reads, not a dependency) — so, unlike `container-core`, no crate here needed a second split purely to keep tokio out of a lower layer's tree; the split here is instead about which crate owns which downloads/runtime, not about a layering violation to avoid.

### 4. SQL Server: ADBC/ODBC, not `tiberius`

The foundry `mssql` ADBC driver (1.6.2, actively released) and `msodbcsql18` via ODBC both work today with no Rust-native SQL Server driver at all; `tiberius`'s abandonment (last release 2024-07) is the same signal that already ruled out `sqls` for the LSP catalog.
A `native` SQL Server row is additive later if `tiberius` revives (R10 in the plan's risk table) — nothing in this decision forecloses it, it is simply not today's default.

### 5. `secret-store` extraction, done in the same commit as the first consumer

`container-registry::secrets::SecretStore` is the only existing OS-keychain wrapper in the tree.
Rather than a second copy for database credentials, F1 extracts it to a new leaf crate `secret-store` (`keyring` 3, `linux-native`/`windows-native`/`apple-native`, same feature set `container-registry` already uses and the same D-Bus-avoidance reasoning ADR-0021/ADR-0055 already established for `keyring`'s Linux backend), with the service name becoming a constructor argument instead of a hardcoded string, so `container-registry` and `db-core` each pass their own (`ide.containers.registries`-shaped vs. `ide.database`) rather than sharing one keychain namespace.
`container-registry` migrates onto the extracted crate in the same commit — extract-don't-copy, the same rule ADR-0047 §4 states for `process-exec`'s extraction from `vcs-core`.

## Non-functional requirements

| Attribute | Target | Verified by |
|---|---|---|
| First row | `SELECT 1` localhost PG/MySQL/SQLite: first `rowsAppended` ≤ 100 ms p95 after Run (warm SQLite ≤ 30 ms) | nightly bench; E2E timestamp on SQLite |
| 1M-row paging | UI thread never blocked > 16 ms per FFI call; scroll to end at page 500 keeps RSS ≤ cap + 64 MiB | `e2e_database` generated fixture, `IDE_E2E_EVENTS` timings, `/proc` RSS |
| Memory cap | fetch stops within one batch of `memory_cap_mib`; "Fetch more" resumes | unit test `ResultSet::append_batch` |
| Introspection | 5 000 tables × 20 cols, level Columns: roots+names ≤ 1.5 s, full ≤ 10 s; lazy schema ≤ 300 ms | nightly `schema-5k` fixture |
| Cancel | server cancel ≤ 500 ms p95; disconnect-cancel ≤ 1.5 s | nightly `pg_sleep(30)`; E2E recursive CTE on SQLite |
| Connect timeout | 10 s default, tunnel readiness 10 s, typed error never a hang | unit test with black-hole address |
| Download integrity | sha256 mismatch → deleted, `DriverChecksum` error, never loaded | unit test corrupted archive |
| DML safety | all values bound; identifiers via `quote_ident`; adversarial fixtures produce 0 unquoted occurrences | property tests; `security-audit` before F4 merges |
| Startup | 0 connections, 0 driver loads at app start | E2E asserts no `db-io` thread before first connect |
| Binary size | Linux release +≤ 20 MB all native drivers; ADBC/ODBC +≤ 3 MB; hard-fail +30 MB | CI artifact size |
| Cross-build | every new crate cross-builds **and links** for `x86_64-pc-windows-gnu` before its phase merges | per-phase spike in `ide-windows-builder` |
| Coverage | ≥ 80% patch on Qt-free crates with no real DB (SQLite + fake `Connection`) | existing gate |

## Alternatives considered

| Option | Why rejected |
|---|---|
| Arrow `RecordBatch` as the one common row-batch type | Seven of eight native drivers would convert into it and back out for no reason; MongoDB/Redis documents fit a columnar batch badly; `adbc_core` 0.24 pins `arrow >=58,<60` while Arrow ships a major monthly, so anything beyond `db-driver-adbc` taking the dependency directly would mean bumping Arrow in lock-step across every driver crate whenever `adbc_core` moves its pin. |
| One `db` crate for every backend | Every backend crate's own dependency footprint (a private tokio runtime, `reqwest`, Arrow, ODBC's link-time `libodbc.so.2`) would leak into `cargo check -p db`'s build graph even for a consumer that only ever opens SQLite files — the same "separate driver crates keep `cargo check -p db-core` fast" reasoning the plan states as R9. |
| `tiberius` for SQL Server | Abandoned upstream (last release 2024-07); this repo's existing rule against abandoned libraries already excluded `sqls` for the same reason. |
| Bundled `duckdb` crate (the multi-GB C++ build) | Documented mingw cross-build failures, and a multi-gigabyte bundled build for one of many supported engines; the prebuilt `libduckdb` shared library through ADBC needs no C++ toolchain in this repo's own build at all. |
| An async trait at the seam (`ui-shell` `.await`s a driver call) | ADR-0007 already settled this for every long-running operation in this codebase: background work runs on its own thread and reports through `CxxQtThread::queue()`, so the adapter never owns or reasons about an async runtime. Reopening it here for one feature would be a second concurrency model living beside the one every other background-work crate already uses. |

## Consequences

- `db-core` stays the single place every backend-agnostic rule lives (the value model, the dialect table, the read-only classifier's client side, the DML-plan builder) — a consumer never re-derives a rule a driver already encodes, the same "one source of truth" shape `syntax-core`'s language registry and `run-core::toolchain` already establish for their own domains.
- The three backend crates' combined footprint (a private runtime, `reqwest`, Arrow, `libodbc.so.2`) is real but contained: none of it reaches `run-core`/`build-core`/`dap-core`'s tree, and `cargo tree -p db-core -e normal | grep -iE 'qt|tokio'` stays empty forever, by construction.
- `secret-store`'s extraction gives every future OS-keychain consumer (a third one would otherwise have copied `container-registry`'s wrapper a second time) one already-tested crate rather than a growing pile of near-identical `keyring` wrappers.
- The P0 cross-link spike (`docs/architecture/database-tools-plan.md` §13) found two things this decision's original draft did not anticipate: `aws-lc-rs` enters the tree transitively through `russh`/`redis`/`scylla`/`tokio-postgres-rustls`'s own default features despite a workspace-level `rustls` override, and `mongodb` 3.9.1 does not currently compile under this repo's pinned rustc at all. Both are now tracked as risks (R2 corrected, R11 added) rather than settled facts, and F1/F7/F8 each carry the concrete follow-up task.

## Related

- [ADR-0007: embedded terminal](0007-embedded-terminal.md) — the `std::thread` + `CxxQtThread::queue()` shape this ADR's seam reuses rather than an async trait.
- [ADR-0021: AI chat](0021-ai-chat.md) — `reqwest::blocking`'s private tokio runtime and the `keyring` Linux-backend reasoning `db-driver-adbc` and `secret-store` reuse.
- [ADR-0040: `build-core`](0040-build-core.md) — the delegate-and-never-model rule `Value::Decimal(String)` restates for a driver's own numeric representation.
- [ADR-0047: the `analyzers` contribution point](0047-analyzers-contribution-point.md) — the extract-don't-copy rule `secret-store`'s extraction from `container-registry` follows.
- [ADR-0055: CLI-driven container integration](0055-cli-driven-container-integration.md) — `container-core`/`container-registry`'s split, the direct precedent for `db-drivers`/`db-driver-adbc`'s own tokio-carrying leaf crates; amended by one sentence for the `containers` manifest split, see [ADR-0059](0059-tool-window-and-settings-page-contribution-points.md).
- `docs/architecture/database-tools-plan.md` — the plan this ADR's driver-seam decisions belong to; §13 carries the P0 cross-link spike's findings this ADR's consequences section summarises.
