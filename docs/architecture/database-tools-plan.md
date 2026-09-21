# Database Tools — IntelliJ-parity plan (built-in plugin `database-tools`)

Product side: product-owner judgement folded into §1–§2 and §12.
Tech side: software-architect judgement folded into §3–§8 (marked **[J]** where it is a judgement call rather than a constraint).
Nothing has been written to the repo before P0; P0 lands this plan doc plus ADR-0058..0061, the `docs/architecture/database-tools.md` skeleton, `docs/architecture/db-integration.md`, the `docs/README.md`/`layering.md` rows, and a throwaway Windows cross-link spike (§13).

## 1. Context

IntelliJ IDEA ships Database Tools (DataGrip inside the IDE): data sources, a Database tool window, query consoles, a result grid/data editor, import/export, diagrams, schema/data compare, object dialogs, schema-aware SQL assistance.
This IDE has none of it.
`syntax-core` has a `sql` tree-sitter grammar (`tree-sitter-sequel` 0.3.11, `crates/syntax-core/src/catalog.rs:284`), `lsp-core`'s catalog names `sqls` (abandoned upstream), and no DB driver crate is in `Cargo.lock`.
The plugin host (ADR-0026) carries data-shaped contributions only; docks, settings pages and tab kinds are wired by literal id in `main_window.cpp:383-384` and `settings_dialog.cpp:311-340`.
That is why `jvm-build-tools` is roughly 10% manifest and 90% hard-wired Rust/C++, and why disabling a plugin today hides nothing.

Goal: JetBrains-parity database tooling, delivered as a built-in plugin, over maintained Rust-native + ADBC + ODBC drivers, NoSQL (MongoDB, Redis, Cassandra/Scylla) first-class, and a plugin host that lets a plugin declare its own tool windows and settings pages.

User decisions taken (2026-09-16): drivers = native Rust + ADBC driver manager + ODBC bridge.
NoSQL is in core scope.
The plugin host gains `database-drivers`, `sql-dialects`, `tool-windows`, `settings-pages` contribution points, and the existing built-ins migrate to them.

## 2. Parity matrix (JetBrains help pages, read 2026-09-16)

| Area | JetBrains | This plan | Phase |
|---|---|---|---|
| Data sources | 30+ complete / 20+ basic DBMS; SSH/SSL/proxy; password/Kerberos/IAM/pgpass; read-only; groups + colour; project vs global; driver auto-download; `dataSources.xml` without passwords | Native: PostgreSQL, MySQL/MariaDB, SQLite, MongoDB, Redis, Cassandra/Scylla. ADBC (download on demand): SQL Server, DuckDB, Oracle, Snowflake, BigQuery, ClickHouse, Databricks, Trino, Presto, HANA, Teradata, Redshift, Spark, SingleStore, Exasol, Druid, Flight SQL. ODBC: anything with a system DSN. SSH tunnel (agent/key/config; password later), TLS modes, password/pgpass/env auth; read-only; groups + colour; global/project TOML; secrets in keychain. Kerberos/IAM/proxy deferred. | F1, F7, F8 |
| Database tool window | tree db/schema/table/view/mat view/routine/sequence/trigger/index/constraint/column/role/user; pattern + type filters; grouping; speed search; refresh/force; introspection levels; Go to DDL; Jump to console; Edit data; SQL Generator; rename/drop/truncate/comment; diagrams; import/export entries | all except the quick-doc popup and "scroll from editor" (deferred) | F2 |
| Query console | per source, many; schema switcher; run statement/selection/file; Output + Result tabs; tx auto/manual; error policy stop/ignore/ask; history; cancel; run SQL file config | all; consoles stored as `.sql` files; `sql-script` run-configuration kind | F3 |
| Result grid / data editor | table/tree/text/transpose; paging; WHERE/ORDER BY; inline + value editor; NULL/DEFAULT; add/delete/clone; submit/revert; DML preview; FK navigation; record view; aggregates; column visibility | all except a geo viewer and charts (deferred) | F3, F4 |
| Import/export | CSV/TSV/JSON/SQL insert+update/HTML/Markdown/XLSX; clipboard/file/console; CSV import mapping; pg_dump/mysqldump; copy table between sources | all; dumps via `process-exec` to the DBMS' own tool; XLSX via `rust_xlsxwriter`/`calamine` | F5 |
| Diagrams | ER diagram of schema/table | Mermaid `erDiagram` generated from the schema model, rendered by the existing preview dock (ADR-0033) | F6 |
| Schema compare + migration (Ultimate) | | object-level diff → migration script + per-object DDL pairs on `DiffView` (ADR-0030) | F6 |
| Data compare (Ultimate) | | key-aligned row diff as canonical TSV per side on `DiffView` (grid diff = recorded debt) | F6 |
| Create/modify object dialogs (Ultimate) | | table/column/index/FK/user editors emitting a DDL preview before running | F4 |
| SQL assistance | dialect-aware completion, inspections, formatting, navigation | native Rust provider over the introspected schema (`sqlparser` 0.63 + `sqlformat` 0.5), surfaced through the existing completion/diagnostics seams — not LSP | F3 |
| NoSQL | MongoDB, Redis, Cassandra complete | collections/keys/keyspaces in the same tree; JSON/tree grid mode; Mongo `runCommand` JSON + `db.coll.find(...)` sugar (no JS engine), RESP console, CQL console | F7 |
| PL/SQL debugger (Ultimate) | | out of scope | — |

## 3. Decisions

1. **Built-in plugin `database-tools` + Qt-free crates**, the same split as `jvm-build-tools`/`jvm-build-core` (ADR-0057): the manifest is data (driver catalogue, dialects, tool windows, settings pages); the code is native crates.
2. **One blocking `Connection` trait in tokio-free `db-core`; three backend leaf crates** (`db-drivers` native, `db-driver-adbc`, `db-driver-odbc`), each owning its own runtime. The seam `ui-shell` sees is synchronous on a `std::thread` + `qt_thread().queue` (the `containers/service.rs` shape). Rejected: an async trait at the seam (ADR-0007).
3. **Own `Value`/`RowBatch` model, not Arrow [J]**. Seven of eight native drivers would convert anyway; Mongo/Redis documents fit columnar batches badly; `adbc_core` 0.24 pins `arrow >=58,<60` while Arrow ships a major monthly — Arrow stays confined to `db-driver-adbc`.
4. **SQL consoles are real files**, `<config_dir>/consoles/<source-id>/*.sql`, opened as an ordinary `TabKind::Text` with language `sql`. No new `TabKind` (ADR-0020); virtual documents stay read-only (ADR-0036) and serve Go-to-DDL and the DML preview. Results/data editor is one bottom dock with per-console tabs over a windowed `QAbstractTableModel`.
5. **SSH tunnel = spawn `ssh -L` via `process-exec`** (ADR-0055's argument: the user's `~/.ssh/config`, agent, FIDO and ProxyJump come free). `russh` 0.63 is the in-process fallback when the CLI is missing or password/passphrase auth is needed (F7, see §12).
6. **TLS = rustls with the `ring` provider on every driver**; `aws-lc-rs` (needs cmake + NASM, a recurring MXE hazard) must never enter the tree — `cargo tree -i aws-lc-rs` empty joins `make lint`. **The P0 spike found this is not automatic** — see §13; it is a per-driver-crate feature-selection task for F1/F7/F8, not a fact this decision could assert as already true.
7. **SQL Server via ADBC (foundry `mssql` 1.6.2, 2026-09-10) / ODBC (`msodbcsql18`), not `tiberius` [J]**: tiberius' last crates.io release is 2024-07 — fails the "no abandoned libs" rule. Revisit trigger: `tiberius-rs` publishes ≥ 0.13.
8. **DuckDB via prebuilt `libduckdb` through ADBC** (entrypoint `duckdb_adbc_init`, pinned sha256), not the `duckdb` crate's multi-GB bundled C++ build with documented mingw failures.
9. **Sessions, not pools [J]**: one connection per console and one per source for the tree, lazy open, idle close (30 min default). Manual tx and open cursors pin a connection anyway; IntelliJ's default is the same.
10. **Generic contribution points land first (G1) [J]** so the `database`/`databaseResults` docks are born on the generic path instead of migrated later.
11. **Additive contribution points ⇒ `api_version` stays 1** (`Contributes` flattens unknown keys, `plugin-api/src/manifest/mod.rs:322-341`). Wasm plugins may *declare* a tool window; it is skipped with a Plugins-page row until a `render-tool-window` WIT export exists (that will be `api_version` 2 — stated, deferred).

Driver catalogue verdicts (all verified on crates.io 2026-09-16/17, confirmed to build and link by the P0 spike, §13):

| Target | Backend | Crate | Status |
|---|---|---|---|
| PostgreSQL | native | `tokio-postgres` 0.7.18 + `tokio-postgres-rustls` 0.14 (`ring`) | OK |
| MySQL/MariaDB | native | `mysql_async` 0.37.1 (`minimal-rust`, `ring`) | OK |
| SQLite | native | `rusqlite` 0.40.2 `bundled` (plain C) | OK; also the E2E backend |
| MongoDB | native | `mongodb` 3.9.1 (rustls) | **Blocked** — see §13; a spike finding, not a plan assumption |
| Redis | native | `redis` 1.7.0 (`tls-rustls`) | OK, sync API |
| Cassandra/Scylla | native | `scylla` 1.9.0 (`rustls-023`) | OK |
| SQL Server | ADBC / ODBC | foundry `mssql` / `msodbcsql18` | see decision 7 |
| DuckDB | ADBC | `libduckdb` release asset | see decision 8 |
| Oracle, Snowflake, BigQuery, ClickHouse, Trino, … | ADBC | `adbc_core` + `adbc_driver_manager` 0.24.0 (pure Rust, `libloading`), `arrow-array`/`-schema` 59 | OK; `rust-oracle` rejected (needs Instant Client) |
| Anything else | ODBC | `odbc-api` 29.0.0 / `odbc-sys` 0.31, `default-features=false` | OK on Linux with `unixodbc-dev` (R3, confirmed); OK on Windows against the system driver manager, no extra package needed |
| SQL parse/format | — | `sqlparser` 0.63.0 (16 dialects, no CQL), `sqlformat` 0.5.0 | OK |
| Export/import | — | `csv` 1.4, `rust_xlsxwriter` 0.99.1, `calamine` 0.36.1 | OK |
| Download integrity | — | `sha2` 0.11 | OK |

## 4. Architecture

### Crates (all Qt-free)

| Crate | Layer | Depends on | tokio |
|---|---|---|---|
| `secret-store` | support, leaf | `keyring` 3 (`linux-native`,`windows-native`,`apple-native`) — **extracted** from `crates/container-registry/src/secrets.rs`; the service name becomes a constructor argument; `container-registry` migrates in the same commit (extract-don't-copy, ADR-0047 §4) | no |
| `db-core` | support | `app-config` (reads `DataSourceSetting`), `process-exec` (ssh child), serde, serde_json, chrono, uuid | **no** |
| `db-sql` | support | `db-core`, `diagnostics-core`, `sqlparser`, `sqlformat` | no |
| `db-exchange` | support | `db-core`, `db-sql`, `editor-core` (`diff`), `process-exec`, csv, rust_xlsxwriter, calamine | no |
| `db-drivers` | support, leaf | `db-core`, tokio (`rt-multi-thread`), the six native drivers, rustls/`ring`; a cargo feature per driver, default on; `sqlite` required by `e2e` | private runtime |
| `db-driver-adbc` | support, leaf | `db-core`, adbc_core, adbc_driver_manager, arrow-array/-schema (`default-features=false`), reqwest blocking rustls, sha2, tar/flate2/zip | reqwest's, tolerated |
| `db-driver-odbc` | support, leaf | `db-core`, odbc-api; behind cargo feature `odbc` (default on) | no |
| `plugin-api` / `plugin-host` | existing | + 4 contribution points, accessors, builtin `database-tools`, builtin `containers` (manifest only) | — |
| `app-config` / `settings-model` | existing | + `[database]` settings, `ScopedField::Database` | — |
| `ui-shell` | adapter+view | + the seven crates above | — |

`app-core` gains nothing: source-of-tab is a pure path rule in `db_core::console`; ER diagrams ride `app_core::preview::PreviewService::render` with a synthetic `.mmd` path (`preview.rs:138` never reads the file).

Layering gate lines: `cargo tree -p <c> -e normal | grep -iE 'qt|tokio'` empty for `db-core`, `db-sql`, `db-exchange`, `db-driver-odbc`, `secret-store`; `grep -i qt` only for `db-drivers`, `db-driver-adbc`.

### Module trees

```
crates/db-core/src/
  value.rs      Value { Null, Bool, Int, Float, Decimal(String), Text, Bytes, Date, Time, DateTime, DateTimeTz, Uuid,
                        Json, Array, Document(Vec<(String,Value)>), Other{type_name,display} }; ColumnMeta{name,type_name,nullable,origin}; RowBatch
  schema.rs     SchemaSnapshot, Node{ref,kind,name,detail,children: NotLoaded|Loaded}, ObjectRef{path: Vec<String>} (never stored quoted),
                ObjectKind { Catalog, Schema, Table, View, MaterializedView, Column, Index, Constraint, Trigger, Routine, Sequence, Type, Role, User,
                             Collection, Field, Keyspace, KeyNamespace, Key(RedisType), Group }, IntrospectLevel { Names, Columns, Full }
  driver.rs     Driver { id, capabilities, connect }, Connection { dialect, server_info, introspect, execute, begin/commit/rollback, set_read_only,
                cancel_handle, ddl_of, apply(DmlPlan), close }, Execution { Rows(RowStream) | Affected | Multi | Ok }, RowStream::next_batch,
                CancelHandle (server-side), CancelToken(Arc<AtomicBool>) (consumer-side), Capabilities bitflags, ExecOptions{fetch_size 200, max_rows, timeout, read_only}
  dialect.rs    Dialect { ident_quote, string_escape, param_style, limit_syntax, supports_returning, tx_read_only_stmt }; quote_ident / quote_literal — the only place identifiers/literals are rendered
  dml.rs        EditBuffer (pending cell/row edits keyed by row + captured PK), DmlPlan{ Vec<Dml{sql, params}> }, NoPrimaryKey policy refuse|all-columns-where
  result.rs     ResultSet { columns, rows, bytes, complete }, MemoryCap, append_batch -> CapReached
  datasource.rs DataSource (typed view over app_config row), Secrets, ConnectSpec::from(source, secrets, tunnel_port), SslConfig{ Disable|Prefer|Require|VerifyCa|VerifyFull, …}
  tunnel.rs     Tunnel trait; CliTunnel: free port, argv (`ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes -L 127.0.0.1:<p>:<host>:<port> user@bastion`), readiness probe 10 s, Drop kills
  console.rs    consoles_dir, console_path(source, name), source_of(path), next_console_name
  history.rs    JSONL per source, cap 1000
  session.rs    Session: one Connection, tx mode Auto|Manual, read_only, execute_script(policy StopOnError|Continue|Ask)
  readonly.rs   ReadOnlyGuard: server statement per dialect + client classifier hook
  error.rs      DbError { code: DbErrorCode (append-only), message, position, sqlstate }

crates/db-sql/src/     split.rs (string/comment/dollar-quote/DELIMITER aware) · classify.rs (Read|Write|Ddl|Tx|Unknown) · parse.rs (sqlparser dialect map → Diagnostic)
                       completion.rs (context after FROM/JOIN/WHERE/INSERT INTO/ALTER…, alias resolution, ranking) · inspections.rs (unresolved table/column, ambiguous column,
                       DELETE/UPDATE without WHERE) · format.rs · ddl.rs (synthesize DDL where the engine returns none) · navigation.rs (identifier → ObjectRef) · mongo.rs (sugar → runCommand)
crates/db-exchange/src/ export/{csv,tsv,json,sql,html,markdown,xlsx}.rs · import/{csv,xlsx}.rs · dump.rs (pg_dump/pg_restore/mysqldump/mongodump argv + spawn)
                       · copy_table.rs · schema_compare.rs → MigrationScript · data_compare.rs → canonical TSV per side · er_diagram.rs → Mermaid erDiagram
crates/db-drivers/src/ lib.rs (OnceLock<tokio::Runtime>, 2 workers "db-io"; DriverRegistry::builtin()) · postgres.rs mysql.rs sqlite.rs mongodb.rs redis.rs cassandra.rs
                       · introspect/*.sql fixture-tested · testsupport.rs (feature db-integration) · tunnel.rs (RusshTunnel, F7)
crates/db-driver-adbc/src/ driver.rs · arrow.rs (the only Arrow code in the workspace) · install.rs (download → sha256 → unpack → ADBC manifest TOML → quarantine marker) · locate.rs (IDE dir → ADBC standard search)
crates/db-driver-odbc/src/ driver.rs · introspect.rs (SQLTables/SQLColumns/SQLPrimaryKeys/SQLForeignKeys)
```

Cancellation: `CancelToken` stops the consumer; `CancelHandle` (obtained before `execute`) stops the server — PG `cancel_query`, MySQL `KILL QUERY` on a second connection, SQLite `interrupt`, Mongo `killOp`; Scylla/Redis/ODBC/ADBC-without-cancel → drop + reconnect, reported as `CancelledByDisconnect`.
Paging: the stream stays open; "next page" pulls the next `page_size` (500) rows, no OFFSET re-query; changing WHERE/ORDER BY re-executes with the clause appended via dialect quoting; the open cursor pins the console connection and any new statement closes it first.

### Plugin-host contribution points

```toml
# crates/plugin-host/builtin/database-tools/plugin.toml (excerpt)
id = "database-tools"   name = "Database Tools"   version = "1.0.0"   api_version = 1   license = "MIT"

[[contributes.database-drivers]]
id = "postgresql"  name = "PostgreSQL"  family = "postgresql"  backend = "native"  native-id = "postgres"
default-port = 5432  url-template = "postgresql://{user}@{host}:{port}/{database}"  dump-tool = "pg_dump"  icon = "postgresql"

[[contributes.database-drivers]]
id = "duckdb"  name = "DuckDB"  family = "duckdb"  backend = "adbc"  url-template = "{database}"
[contributes.database-drivers.adbc]
manifest-name = "duckdb"  entrypoint = "duckdb_adbc_init"
[[contributes.database-drivers.adbc.artifacts]]
platform = "linux_amd64"  url = "https://github.com/duckdb/duckdb/releases/download/v1.5.5/libduckdb-linux-amd64.zip"  sha256 = "…"  library = "libduckdb.so"

[[contributes.database-drivers]]
id = "odbc"  name = "ODBC data source"  family = "generic"  backend = "odbc"

[[contributes.sql-dialects]]
id = "postgresql"  name = "PostgreSQL"  parser = "postgresql"  identifier-quote = "double"  param-style = "dollar"  keywords = "dialects/postgresql.keywords"

[[contributes.tool-windows]]
id = "database"         title = "Database"          area = "right"
[[contributes.tool-windows]]
id = "databaseResults"  title = "Database Results"  area = "bottom"

[[contributes.settings-pages]]
id = "database"  title = "Database"  scope = "project"
```

Validation in `plugin-api` (no filesystem): id charset + uniqueness per point; `backend` enum; port 1..=65535; url-template placeholders ⊆ `{user,host,port,database,options}`; sha256 64 lowercase hex; `https://` only; platform enum; `library`/`keywords` relative and `..`-free; `native` needs `native-id`, `adbc` needs `manifest-name` or ≥ 1 artifact; `area`/`scope` enums. Cross-plugin (`family` names a known dialect) checked in `plugin-host` at registry build → fail-soft `PluginLoadError` row.

Consumers:
- Driver crates never see `plugin-api`; `ui-shell` maps `DatabaseDriverContribution` → `db_core::DriverDescriptor` plain strings (the `toolchain` precedent).
- `tool-windows`: FFI `contributedToolWindows()` → new `cpp/tool_window_factories.{h,cpp}` `QHash<QString, DockFactory>` (`buildTools`, `containers`, `database`, `databaseResults`); `main_window.cpp:383-384` literal calls become one loop: row → factory → `DockRegistry::registerDock` + View-menu action `view.<id>`. No factory → log + skip. Disabled plugin → row absent → no dock/menu. Dock ids leave `status_bar.cpp:294`.
- `settings-pages`: same loop in `settings_dialog.cpp` replacing the hard-coded labels (l.72-97) and `scopedPage("buildTools"/"containers")` (l.311-326) with a `PageFactory` table; `ScopedField` stays an enum (ADR-0022) — a manifest `scope="project"` must match `ScopedField::from_id`, else global-only + a Plugins-page warning.
- Migration: `jvm-build-tools/plugin.toml` gains its `tool-windows`/`settings-pages` rows; a new `builtin/containers/plugin.toml` with **only** those rows (amends ADR-0055 by one sentence; no code moves). Host docks (`searchResults`, `problems`, `terminal`…) stay literal.

### `ui-shell` seam

Bridge `crates/ui-shell/src/bridge/database/` (each ≤ 1500 lines): `mod.rs` `DatabaseService` (sources, connect, tree rows `Vec<FfiDbTreeRow{depth,kind,label,detail,icon_key,node_id,actions}>`, lazy expand, object actions, `virtualDocumentOpened` for the Go-to-DDL scheme `db-ddl`) · `sessions.rs` `SessionWorker` (thread + mpsc + generation counter, tunnel lifetime) · `console.rs` `ConsoleService` (execute/cancel/commit/rollback/runFile, policy, history; signals `executionStarted`/`rowsAppended`/`executionFinished`) · `results.rs` `ResultProvider` (windowed `rowPage(resultId, first, count)` clamped like `MAX_HEX_ROWS_PER_REQUEST`, `setCell`, add/delete/clone, `dmlPreview`, submit/revert, transpose/aggregates computed in `db-core`, FK target) · `settings.rs` `DataSourceEditor` (draft/validate/commit, `testConnection` on a worker, secrets → `secret-store`) · `drivers.rs` `DriverInstallService` · `exchange.rs` `ExchangeService` (jobs → `db-exchange`; diagram → `PreviewProvider::requestPreview(tabId, "<source>/erd.mmd", mermaid)`) · `bridge/language/database.rs` (`database_completion` beside `container_completion` at `language/mod.rs:961`; inspections under source `database:inspections` via `store.replace`; intentions "Go to DDL"/"Format SQL" as `CodeActionItem{kind:"database.*"}`).

C++ (each ≤ 1200 lines): `database_panel` (QTreeWidget over flattened rows, filter, grouping, context menu from `actions` bits) · `database_results_panel` (bottom dock, tab per console like `run_console_panel.cpp:258`) · `result_table_model` (first `QAbstractTableModel` in the tree: `rowCount` from Rust, `data()` from an LRU of fetched pages, `fetchMore` pages) · `result_grid_view` (QTableView + toolbar: modes, page nav, WHERE/ORDER BY, row ops, aggregate footer) · `value_editor_dialog` · `database_console_bar` (strip above a `CodeEditor` whose path resolves via `source_of` or an attached `.sql`: source/schema pickers, tx mode, Run/Cancel) · `data_source_dialog` (general/SSH/SSL/options tabs, Test) · `database_settings_page` · `db_object_dialogs` · `db_exchange_dialogs` · `database_wiring` + `database_menu`. Every string `tr()` (ADR-0049). No business `if` in C++: actions, edit legality, NULL rendering, DML text all arrive from Rust.

Run configuration: `kind = "sql-script"` + `[run_configs.sql_script]{source_id, file, tx_mode, stop_on_error}`; `run-core/src/config.rs:85-99` returns `None` for the kind and `RunService` routes to `ConsoleService` (kind-tag selects executor, ADR-0056 shape). `run-core` gains no `db-*` edge.

### Persistence

```toml
[database]                      # ScopedField::Database; sources merge by id, project shadows global (ADR-0045 union rule)
page_size = 500   memory_cap_mib = 256   idle_close_minutes = 30   history_cap = 1000   allow_third_party_drivers = false
[[database.sources]]
id = "3f0c…"  name = "prod-replica"  driver = "postgresql"  group = "work/analytics"  color = "#3a7bd5"
host = "db.internal"  port = 5432  database = "shop"  user = "florian"  auth = "password"  read_only = true  history = false  url = ""
[database.sources.ssl]  mode = "verify-full"  ca_file = "…"
[database.sources.ssh]  host = "bastion"  port = 22  user = "florian"  auth = "agent"  key_file = ""
[database.sources.options]  application_name = "ide"
# project .ide/settings.toml additionally:
[database]  file_sources = { "sql/reports.sql" = "3f0c…" }
```

Secrets: never in TOML (structurally no field); `secret-store` service `ide.database`, keys `<id>`, `<id>/ssh`, `<id>/ssl-key`; keychain unavailable → session-only prompt. Consoles `<config_dir>/consoles/<source-id>/console*.sql`. History `<config_dir>/database/history/<id>.jsonl`. Drivers `<config_dir>/database/drivers/<id>/<version>/{manifest.toml,<lib>,.sha256}` (ADBC manifest format, reusable by `dbc`). Dock state via ADS `saveState` as today.

## 5. NFRs

| Attribute | Target | Verified by |
|---|---|---|
| First row | `SELECT 1` localhost PG/MySQL/SQLite: first `rowsAppended` ≤ 100 ms p95 after Run (warm SQLite ≤ 30 ms) | nightly bench; E2E timestamp on SQLite |
| 1M-row paging | UI thread never blocked > 16 ms per FFI call; scroll to end at page 500 keeps RSS ≤ cap + 64 MiB | `e2e_database` generated fixture, `IDE_E2E_EVENTS` timings, `/proc` RSS |
| Memory cap | fetch stops within one batch of `memory_cap_mib`; "Fetch more" resumes | unit test `ResultSet::append_batch` |
| Introspection | 5 000 tables × 20 cols, level Columns: roots+names ≤ 1.5 s, full ≤ 10 s; lazy schema ≤ 300 ms | nightly `schema-5k` fixture |
| Cancel | server cancel ≤ 500 ms p95; disconnect-cancel ≤ 1.5 s | nightly `pg_sleep(30)`; E2E recursive CTE on SQLite |
| Connect timeout | 10 s default, tunnel readiness 10 s, typed error never a hang | unit test with black-hole address |
| Download integrity | sha256 mismatch → deleted, `DriverChecksum` error, never loaded | unit test corrupted archive |
| Read-only | 100% of `Write`/`Ddl`-classified statements refused client-side + server `READ ONLY` where supported; ≥ 40 classifier cases/dialect incl. CTE-wrapped DML, `SELECT … INTO`, `CALL` | `db-sql::classify` tests; nightly negative test per engine |
| DML safety | all values bound; identifiers via `quote_ident`; adversarial fixtures (`a"b`, `` a`b ``, `a]b`, `Robert'); DROP`, NUL, unicode quotes) produce 0 unquoted occurrences | property tests; `security-audit` before F4 merges |
| Startup | 0 connections, 0 driver loads at app start | E2E asserts no `db-io` thread before first connect |
| Binary size | Linux release +≤ 20 MB all native drivers; ADBC/ODBC +≤ 3 MB; hard-fail +30 MB | CI artifact size |
| Cross-build | every new crate cross-builds **and links** for `x86_64-pc-windows-gnu` before its phase merges | per-phase spike in `ide-windows-builder` |
| Coverage | ≥ 80% patch on Qt-free crates with no real DB (SQLite + fake `Connection`) | existing gate |

## 6. Security posture (security-expert review before F1 and F8)

1. Secrets only in the OS keychain; history stores statement text, never bound values; `ConnectSpec: Debug` redacts.
2. TLS default `Prefer` on TCP, `VerifyFull` once a CA is set, `Disable` only explicit; no skip-verify flag in v1.
3. Read-only sources: client classifier + server session flag; data editor hidden, not disabled.
4. ADBC/ODBC load foreign machine code — same blast radius as `syntax_core::runtime` grammars, same defence: a quarantine marker `<config_dir>/database/drivers/.quarantine/<id>` around first load. Downloads: pinned `https://` + sha256 shipped in the built-in manifest; a consent dialog naming publisher/URL/hash on first install; `allow_third_party_drivers = false` default blocks installed-plugin rows from downloading; no auto-update.
5. The resolved ODBC driver path is shown before connecting.
6. SSH `BatchMode=yes`, host-key policy untouched, the tunnel bound to `127.0.0.1`.
7. Dump tools get passwords via `PGPASSWORD`/`MYSQL_PWD` or a 0600 defaults file, never argv.
8. Imports go through parameterised batched INSERTs; XLSX cached values only.

## 7. Risks and accepted debt

| # | Risk | Mitigation |
|---|---|---|
| R1 | Foundry ADBC drivers may not publish raw shared-lib assets (foundry pushes a `dbc` CLI / Python wheels) | two-tier resolve (IDE-managed where pinnable, else ADBC standard search + install hint); F8 opens with a one-day asset-enumeration spike for mssql/snowflake/bigquery/clickhouse/trino; third tier = shell out to `dbc` |
| R2 | `aws-lc-rs` enters transitively, breaks the MXE link | Cargo feature unification means a workspace-root `rustls` override does not suppress a *dependency's own* default features — the P0 spike found `aws-lc-rs` v1.18.1 in the tree via `russh`, `redis`, `scylla` and `tokio-postgres-rustls` even with the plan's pinned root `rustls` entry (§13); F1/F7/F8 must select each driver crate's own ring-only feature (or patch the dependency) per crate, and `cargo tree -i aws-lc-rs` empty joins `make lint` as a real per-driver-crate task, not a fact assumed already true |
| R3 | `odbc-sys` links `libodbc.so.2` at link time on Linux | `unixodbc-dev` in `linux-builder` (confirmed necessary and sufficient by the P0 spike, §13); bundle already copies `ldd` deps (Dockerfile l.359-382); cargo feature `odbc` |
| R4 | Arrow major cadence vs. the adbc pin | Arrow confined to `db-driver-adbc`, bumped together |
| R5 | a sync driver inside `block_on` starves the shared runtime | SQLite/Redis bypass the runtime; 2 workers + `spawn_blocking`; test two concurrent long queries |
| R6 | no CQL parser | keyword+schema completion only; documented |
| R7 | introspection SQL drift across engine majors | lenient parsing; nightly pins two majors per engine |
| R8 | per-PR E2E budget (12 of 16) | exactly one new per-PR flow `e2e_database` (SQLite); real-DB flows nightly `IDE_E2E_DB=1` |
| R9 | binary size / compile time | size gate; separate driver crates keep `cargo check -p db-core` fast |
| R10 | tiberius revives | a `native` mssql row later is additive |
| R11 | `mongodb` 3.9.1 fails to compile under this repo's pinned `rustc` 1.98.1 | new, found by the P0 spike (§13), not in the original risk table — `E0659` ambiguous `doc` item from `macro_magic`'s glob re-export of `bson::doc` colliding with the builtin `doc` attribute, 1226 downstream errors; F7 opens by re-testing against the `mongodb` version current at that time and, if unresolved, files upstream and evaluates a narrower feature set or a pinned older `macro_magic`/`bson` before writing any `db-drivers::mongodb` code |

Debt accepted with triggers: no CQL parser (a maintained crate); data compare as text not grid (cell-level navigation request or > 100k-row compares); migration script covers tables/columns/indexes/constraints, routines/views text-diffed (nightly DDL synthesis for two engines); no prepared-statement cache (> 20% of execute time in bench); `containers` builtin is manifest-only; wasm tool-window content (first third-party request → `api_version` 2).

## 8. ADRs (numbers continue from 0057)

- **ADR-0058** Database tools: one blocking driver seam, three backends, own value model. Rejected: Arrow common type, one big `db` crate, `tiberius`, bundled `duckdb`, async trait at the seam. NFR table attached. `secret-store` extraction in consequences.
- **ADR-0059** Generic `tool-windows`/`settings-pages` + `database-drivers`/`sql-dialects` points (amends ADR-0026; amends ADR-0055 by one sentence). Rejected: literal ids, factories in the manifest, an `api_version` bump.
- **ADR-0060** Consoles are files; results/data editing a dock with a windowed table model; states ADR-0020/0036 need no amendment and ADR-0033/0043 preview is reused not changed. Rejected: writable virtual documents, results as a `TabKind`.
- **ADR-0061** Security posture: keychain-only secrets, `ssh` CLI tunnels, rustls defaults, driver-download trust + quarantine. Rejected: `russh` now, auto-updating drivers.

Docs: `layering.md` rows + gate lines; `docs/README.md` (4 ADRs, `database-tools-plan.md`, `database-tools.md`, `db-integration.md`); `overview.md` §3; `project-structure.md` (both truthed up in F9).

### Architecture document `docs/architecture/database-tools.md` (arc42-lite, Mermaid diagrams)

The feature gets its own architecture doc beside `overview.md`, not only ADRs and the plan — the ADRs record *why*, this doc records *what is there*, kept truthful to the code like `overview.md` (CLAUDE.md "fix drift when you see it").
Sections, one full sentence per line:

1. Context and scope — parity target, what is out of scope, the plugin boundary (C4 level 1: IDE ↔ DBMS ↔ OS keychain ↔ `ssh` ↔ driver downloads).
2. Building blocks (C4 level 2/3) — the seven crates, `plugin-api`/`plugin-host` additions, `ui-shell::bridge::database` modules and the `database_*.cpp` files; a Mermaid component diagram with the allowed dependency edges (mirrors the `layering.md` rows).
3. Driver seam — `Driver`/`Connection`/`RowStream`/`CancelHandle` contract, `Capabilities` flags per backend, the three backends and which DBMS each serves, the `Value` model and per-driver type mapping tables.
4. Runtime views (Mermaid sequence diagrams) — connect (secrets → tunnel → TLS → session thread), execute + stream + page + cancel (two cancellation layers, generation counter), data-editor submit (EditBuffer → DmlPlan → apply → refresh), driver install (download → sha256 → quarantine → probe), Go-to-DDL (virtual document), ER diagram (schema → Mermaid → preview dock).
5. Threading and lifetimes — one session thread per console/source, the private tokio runtime in `db-drivers`, `qt_thread().queue` hand-off, idle close, what pins a connection.
6. Schema model — the `ObjectKind` table incl. the NoSQL mapping (Collection/Field/Keyspace/KeyNamespace/Key) and the actions matrix per kind.
7. Plugin contract — the four contribution points, TOML shapes, validation rules, consumer join points (factory tables), disabled-plugin behaviour, the wasm limitation.
8. Persistence — settings TOML (global/project merge), keychain keys, console files, history, driver directory layout.
9. Security posture — condensed §6 with pointers to ADR-0061.
10. Verification — the per-PR vs. nightly matrix, the `db-integration` feature, the `linux-db` image, the E2E flow, and the P0 cross-link spike result.
11. Known gaps and debt — the table from §7 with triggers.

Lifecycle: P0 writes the skeleton with the target design; every phase PR updates the sections it touches in the same PR (reviewer checks the doc diff against the code diff); F9 does the final truth-up and drift review.
Diagrams: Mermaid in-repo (the IDE's own preview renders them), syntax-reviewed before commit.

## 9. Task list (one PR per phase off `main`; phases run in parallel waves per §11; `make test` + `make lint` green per commit; Progress row updated in the same commit; no co-author line)

| Phase | Delivers | Verified without a DB | Nightly `linux-db` |
|---|---|---|---|
| **P0** | plan doc, ADR-0058..0061, README index, layering rows, `docs/architecture/database-tools.md` skeleton (§8 sections, target-design diagrams), `docs/architecture/db-integration.md`; cross-link spike (§13); `cargo tree -i aws-lc-rs` run and recorded (not empty — see R2); result recorded in this plan (wasmtime P0 precedent) | — | — |
| **G1** generic points | `plugin-api` `tool-windows`/`settings-pages` + validation; `plugin-host` accessors; `tool_window_factories.cpp`, page factory table; `jvm-build-tools` + new `containers` manifest migrated; disabled plugin hides dock/menu/page | plugin-api fixture tests; `e2e_containers` green + one assertion: disabling `containers` removes the View entry | — |
| **F1** foundation | `secret-store` extraction; `db-core`; `db-drivers` sqlite + postgres; `[database]` settings, `settings-model::database`, `ScopedField::Database`; `database-tools` plugin.toml (`database-drivers`, `sql-dialects` rows); Data Source dialog + settings page + Test connection; per-crate `aws-lc-rs` exclusion (R2) | SQLite; fake `Connection`; tunnel argv tests | PG connect/introspect/TLS matrix |
| **F2** tree | introspection levels, flattened rows + actions matrix, filters, grouping, refresh/force, Go to DDL (virtual doc), SQL generator, rename/drop/truncate/comment via `db_sql::ddl`; `database` dock | `e2e_database` part 1: add SQLite source → tree → DDL | PG/MySQL introspection fixtures; 5k-table NFR |
| **F3** console + grid | console files + bar; `db-sql` split/classify/parse/format; run statement/selection/file; tx modes; error policy; cancel; history; `ResultTableModel` + paging + cap; `databaseResults` dock; `sql-script` run config; completion/inspections seam | `e2e_database` part 2: console → run → grid → 1M rows → cancel | first-row/cancel NFRs on PG/MySQL |
| **F4** data editor | `EditBuffer`, inline + value editor, NULL/DEFAULT, add/delete/clone, DML preview (virtual doc), submit/revert, FK navigation, aggregates, transpose/tree/text, WHERE/ORDER BY; create/modify object dialogs | `e2e_database` part 3: edit → preview → submit → reopen; adversarial identifier tests; `security-audit` pass | RETURNING/affected-rows semantics per engine |
| **F5** exchange | export formats; CSV/XLSX import mapping; dump/restore via `process-exec` into the Run dock; copy table between sources | SQLite→SQLite copy; exporters fixture-tested | `pg_dump`/`mysqldump` in image |
| **F6** diagrams + compare | ER Mermaid → preview dock; schema compare (DiffView + migration script); data compare (DiffView) | SQLite fixtures; preview E2E reuse | cross-engine DDL synthesis fixtures |
| **F7** NoSQL + SSH fallback | mongodb/redis/scylla drivers (mongodb pending R11 resolution — see §7); `Collection`/`Key`/`Keyspace` kinds; Mongo sugar; RESP console; document grid mode; `RusshTunnel` (no-CLI fallback, password/passphrase auth, known-hosts prompt) | Mongo sugar + RESP parsers unit-tested | mongo, redis, scylla services |
| **F8** ADBC + ODBC | `db-driver-adbc` (locate, install, quarantine, Arrow conversion); `db-driver-odbc`; DuckDB + SQL Server + foundry rows after the R1 spike; install UI + consent; `unixodbc-dev` in `linux-builder` (confirmed by P0, §13); bundle check | DuckDB in-process behind a download-once cache; ODBC vs `libsqliteodbc` in the image | MSSQL container via ADBC/ODBC |
| **F9** docs + polish | architecture-doc truth-up (`database-tools.md` reviewed section by section against the code, diagrams regenerated); overview/project-structure truth; ADRs → Accepted; `tr()` sweep; Windows manual matrix (ODBC DSN, OpenSSH tunnel) | — | full matrix |

Each phase's PR carries a `| Task | Status | Commit |` table in this plan doc (jvm plan shape); sub-tasks are enumerated below, `open` except P0's rows.
Every phase PR also updates the sections of `docs/architecture/database-tools.md` it touches (definition of done, checked in review); a PR that changes a seam, thread, file layout or contribution point without a doc diff is not mergeable.

### P0 — this plan doc, ADR-0058..0061, docs, cross-link spike

| Task | Status | Commit |
|---|---|---|
| P0.1 — `database-tools-plan.md` reshaped to repo conventions, parity matrix + user answers kept | done | `f16e9aa` |
| P0.2 — ADR-0058 database driver seam (NFR table attached) | done | `a57c631` |
| P0.3 — ADR-0059 tool-window and settings-page contribution points | done | `a57c631` |
| P0.4 — ADR-0060 query consoles as files and result dock | done | `a57c631` |
| P0.5 — ADR-0061 database security posture | done | `a57c631` |
| P0.6 — `docs/architecture/database-tools.md` skeleton, 11 sections, Mermaid diagrams | done | `fc13504` |
| P0.7 — `docs/architecture/db-integration.md` (target design; `linux-db`/`db-compose.yml`/`make test-db`/`make e2e-db` land in F1) | done | `fc13504` |
| P0.8 — `docs/README.md` index lines (plan, 4 ADRs, `database-tools.md`, `db-integration.md`) | done | `fc13504` |
| P0.9 — `layering.md` rows for `secret-store`, `db-core`, `db-sql`, `db-exchange`, `db-drivers`, `db-driver-adbc`, `db-driver-odbc` (marked "planned, lands in phase X") | done | `fc13504` |
| P0.10 — Windows cross-link spike: throwaway crate outside the workspace, Linux link, Windows cross-link, `cargo tree -i aws-lc-rs`, result recorded (§13) | done | `f16e9aa` |

### G1 — generic contribution points

| Task | Status | Commit |
|---|---|---|
| G1.1 — `plugin-api`: `ContributionPoint::ToolWindows`/`SettingsPages`, `ToolWindowContribution`/`SettingsPageContribution`, validation + round-trip tests | done | db8db83 |
| G1.2 — `plugin-host` accessors (`tool_windows()`, `settings_pages()`) | done | d3e0f5a |
| G1.3 — `cpp/tool_window_factories.{h,cpp}`, `main_window.cpp:383-384` migrated to the factory-table loop | done | 4c0b133, c2d88a1 |
| G1.4 — `settings_dialog.cpp` page-factory table, `l.72-97`/`l.311-326` migrated | partial (presence-guarded, not a generic page-factory table; scope-mismatch fallback unexercised — see `database-tools.md` §7) | f18e9a7 |
| G1.5 — `jvm-build-tools/plugin.toml` gains `tool-windows`/`settings-pages` rows; new `builtin/containers/plugin.toml` (manifest-only, amends ADR-0055) | done | d3e0f5a |
| G1.6 — `e2e_containers` assertion: disabling `containers` removes the View entry | open (deferred to wave-end E2E by the main session) |  |

### F1 — foundation

| Task | Status | Commit |
|---|---|---|
| F1.1 — `secret-store` extraction from `container-registry/src/secrets.rs`; `container-registry` migrates in the same commit | done | 14d287b |
| F1.2 — `db-core`: `value`, `schema`, `driver`, `dialect`, `dml`, `result`, `datasource`, `tunnel` (`CliTunnel`), `console`, `history`, `session`, `readonly`, `error` | done | b26cb5e |
| F1.3 — `db-drivers`: sqlite + postgres backends, `DriverRegistry::builtin()`, private tokio runtime | done | b9f6894 |
| F1.4 — `[database]` `app-config` section; `settings-model::database`; `ScopedField::Database` | done | 8148486 |
| F1.5 — `database-tools` plugin.toml (`database-drivers`, `sql-dialects` rows for postgresql/sqlite) | done | ffaa1e1 |
| F1.6 — Data Source dialog + settings page + Test connection | done | 01a1b2b |
| F1.7 — per-driver-crate `aws-lc-rs` exclusion (R2); `cargo tree -i aws-lc-rs` empty joins `make lint` | done | b9f6894 |

### F2 — tree

| Task | Status | Commit |
|---|---|---|
| F2.1 — introspection levels (Names/Columns/Full), lazy `Node` expansion | done (sqlite+postgres; ADBC/ODBC compile but do not honour `level` yet) | 310fcb3 |
| F2.2 — flattened tree rows + actions matrix, filters, grouping, refresh/force | done | 310fcb3, 8544815 |
| F2.3 — Go to DDL via a `db-ddl` virtual document scheme | done (tab title is `<object>.sql`, not the literal wording — see database-tools.md §4) | 670e0a2 |
| F2.4 — SQL Generator; rename/drop/truncate/comment via `db_core::ddl` (moved from the planned `db_sql::ddl` — `db-sql` is F3's crate to create, not F2's) | done (confirmation dialog does not yet show the exact generated SQL; no auto-refresh after a successful action) | 310fcb3, 670e0a2 |
| F2.5 — `database` dock (`database_panel`), 5k-table NFR bench | dock done; **NFR bench not run** (see follow-up row below) | 670e0a2 |
| F2 follow-up — `e2e_database` E2E flow (needs `database_panel.cpp` row-rect `e2eMark` reporting first) | open | |
| F2 follow-up — 5 000-table SQLite `db-integration` NFR bench + §10 numbers | open | |
| F2 follow-up — Postgres TLS (`SslMode::{Prefer,Require,VerifyCa,VerifyFull}` via `rustls-pemfile`/`rustls-native-certs`, `ring` only) | open | |
| F2 follow-up — ADBC/ODBC `IntrospectLevel` honoured (currently always fetch at existing depth) | open | |

### F3 — console + grid

| Task | Status | Commit |
|---|---|---|
| F3.1 — console files + `database_console_bar`; source/schema pickers, tx mode | open | |
| F3.2 — `db-sql`: split/classify/parse/format | done | c512ce7 |
| F3.3 — run statement/selection/file; error policy Stop/Continue/Ask; cancel; history | open | |
| F3.4 — `ResultTableModel` + paging (500/page) + memory cap | open | |
| F3.5 — `databaseResults` dock, per-console tabs | open | |
| F3.6 — `sql-script` run-configuration kind | open | |
| F3.7 — completion/inspections seam (`database_completion`, source `database:inspections`) | open | |

### F4 — data editor

| Task | Status | Commit |
|---|---|---|
| F4.1 — `EditBuffer`, inline + value editor, NULL/DEFAULT | open | |
| F4.2 — add/delete/clone; DML preview (virtual document); submit/revert | open | |
| F4.3 — FK navigation; aggregates; transpose/tree/text modes; WHERE/ORDER BY | open | |
| F4.4 — create/modify object dialogs (table/column/index/FK/user) | open | |
| F4.5 — adversarial identifier fixtures; `security-audit` pass before merge | open | |

### F5 — exchange

| Task | Status | Commit |
|---|---|---|
| F5.1 — export formats (csv/tsv/json/sql/html/markdown/xlsx) | open | |
| F5.2 — CSV/XLSX import mapping | open | |
| F5.3 — dump/restore via `process-exec` into the Run dock | open | |
| F5.4 — copy table between sources | open | |

### F6 — diagrams + compare

| Task | Status | Commit |
|---|---|---|
| F6.1 — ER diagram → Mermaid `erDiagram` → preview dock | open | |
| F6.2 — schema compare → `MigrationScript` on `DiffView` | open | |
| F6.3 — data compare → canonical TSV per side on `DiffView` | open | |

### F7 — NoSQL + SSH fallback

| Task | Status | Commit |
|---|---|---|
| F7.1 — resolve R11 (mongodb build under pinned rustc) before writing `db-drivers::mongodb` | open | |
| F7.2 — mongodb driver + Collection/Field tree kinds + Mongo sugar (`runCommand`/`db.coll.find`) | open | |
| F7.3 — redis driver + Key/KeyNamespace tree kinds + RESP console | open | |
| F7.4 — scylla driver + Keyspace tree kind + CQL console | open | |
| F7.5 — `db_core::tunnel::Tunnel` trait; `RusshTunnel` in `db-drivers` (password/passphrase, known-hosts prompt) | open | |
| F7.6 — document grid mode | open | |

### F8 — ADBC + ODBC

| Task | Status | Commit |
|---|---|---|
| F8.1 — `db-driver-adbc`: locate, install, quarantine marker, Arrow conversion | done | 08d749e |
| F8.2 — `db-driver-odbc`: introspection via `SQLTables`/`SQLColumns`/`SQLPrimaryKeys`/`SQLForeignKeys` | done | 94b01f2 |
| F8.3 — R1 one-day asset-enumeration spike (mssql/snowflake/bigquery/clickhouse/trino) | done | 08d749e |
| F8.4 — DuckDB + SQL Server + foundry rows | partial (data only — `catalogue.toml`; plugin-manifest wiring is F8b) | 08d749e |
| F8.5 — install UI + consent dialog | open (F8b) | |
| F8.6 — `unixodbc-dev` in `linux-builder` (image change; confirmed necessary by the P0 spike, §13); bundle `ldd`-closure check | done | afa16d6 |

### F9 — docs + polish

| Task | Status | Commit |
|---|---|---|
| F9.1 — `database-tools.md` truth-up, section by section against the shipped code; diagrams regenerated | open | |
| F9.2 — `overview.md`/`project-structure.md` truth | open | |
| F9.3 — ADR-0058..0061 → Accepted | open | |
| F9.4 — `tr()` sweep across every new `cpp/` string | open | |
| F9.5 — Windows manual matrix (ODBC DSN, OpenSSH tunnel) | open | |

## 10. Verification

- Per PR: `make test`, `make lint` (+ the `aws-lc-rs` gate once F1 lands it), the layering greps above, the file-size gate, patch coverage ≥ 80%, `e2e_database` on SQLite (one flow, budget 13/16).
- Nightly/on demand (`docs/architecture/db-integration.md`, mirror of `jvm-integration.md`): `docker/Dockerfile` stage `linux-db` (client tools: `postgresql-client`, `default-mysql-client`, `mongodb-database-tools`, `unixodbc` + `libsqliteodbc`); servers from `docker/db-compose.yml` (postgres:17, mysql:8.4, mariadb:11, mongo:8, redis:7, scylladb/scylla:6; `mssql/server:2022` from F8) and `.github/workflows/nightly.yml` `services:`; `make test-db` = `cargo nextest run -p db-drivers -p db-driver-adbc -p db-driver-odbc --features db-integration`; `make e2e-db` = `IDE_E2E_DB=1` flows.
- Windows: the P0 cross-link spike (§13) and a spike per phase that adds a new crate; manual matrix in F9.
- Docs: `docs/architecture/database-tools.md` diff present in every phase PR; Mermaid blocks syntax-reviewed; `docs/README.md` line added in P0.
- Manual E2E per phase in the real app under Xvfb (headless harness memory): pixel-check the dock, grid and dialogs against the JetBrains layout before each PR merges.

## 11. Delivery — parallel waves with a resource guard

Machine facts (2026-09-21): 251 GB disk, 22 GB free at plan time (43 GB free measured at the start of the P0 session), `target/` = 64 GB (dev + release + debugging + Windows cross-build side by side), 31 GB RAM / 22–24 GB available, 16 cores, two stale agent worktrees present.
A fresh worktree build costs ~25 GB; three parallel worktree builds filled the disk on 2026-08-23. So: one shared `target/`, hard concurrency caps, a watchdog — parallelism buys parallel thinking/editing/testing, not parallel full compiles.

### Pre-flight (main session, before wave 0)

1. Reclaim: delete `target/release`, `target/debugging`, `target/x86_64-pc-windows-gnu` from inside a root container (build output is root-owned); `docker builder prune -f`; remove the two stale worktrees only if their branches are merged (else leave them, they are another session's work). Target: ≥ 50 GB free before wave 1, never start a wave below 30 GB.
2. Rebuild `linux-builder` once with `unixodbc-dev` + `libsqliteodbc` (F8 needs it; doing it once here avoids every agent rebuilding the image). The P0 spike installed both packages into a throwaway image layer to verify the fix and then discarded that image — the real Dockerfile change still lands in F8 (task F8.6).
3. The `ide-windows-builder` image is stale relative to `docker/Dockerfile`'s pinned `RUST_VERSION=1.98.1` — it still runs rustc 1.90.0 (the P0 spike hit this directly, §13). A rebuild is needed before any phase that adds a Windows-affecting crate runs its own cross-link spike; until then, `cargo update --precise` on the three `windows-*` crates that require 1.95 is the same workaround the P0 spike used, and is not a fix a phase agent should have to rediscover.
4. Write one reusable agent brief (rules below, Docker-only builds, gates, commit format, "open PR, do not merge, report").

### Worktree + build-cache rules (in every agent brief)

- One worktree per phase: `git worktree add .claude/worktrees/db-<phase> -b feat/database-<phase> main`, then `git submodule update --init --recursive`. Never touch the shared checkout.
- Shared build cache: every `make` call bind-mounts the main checkout's `target/` over the worktree's — `make test DOCKER_MOUNTS='-v "$PWD":/workspace -w /workspace -v /home/florian/projects/ide/target:/workspace/target <the registry/ccache/sccache volume flags from the Makefile>'`. Cargo's target lock serialises concurrent compiles automatically; a waiting agent sees "Blocking waiting for file lock" — expected, do not kill it, do not create a private `target/`.
- `CARGO_BUILD_JOBS=10` in every agent build so one compile leaves ~10 GB RAM for the others' tests, Xvfb and the editor.
- Iterate with `cargo check -p <crate>` / `cargo test -p <crate>` inside `make shell`; run full `make test` + `make lint` only before commit and before opening the PR.
- Before every `make test`/`make lint`: `df -h / | tail -1` and `free -g | head -2`; stop and report if free disk < 10 GB or available RAM < 4 GB — never delete anything to make room.
- Each phase appends its own `#[qobject]` block at the end of `bridge/ffi.rs` under a `// ---- database: <phase> ----` marker and adds its own files; no phase edits another phase's files. `main_window.cpp` / `settings_dialog.cpp` are touched only by G1 (the factory tables) and then by each phase through the factory registration line only. Conflicts on merge are resolved by the main session, never by force-push.
- Sonnet `senior-software-engineer` agents (the user's delivery shape); one agent per phase; the agent opens the PR and reports; the main session merges `--squash --delete-branch` when CI is green and removes the worktree.

### Concurrency caps

- Never more than 3 agents alive; never more than 2 whose phase compiles `ui-shell`. A third agent is allowed only for a Qt-free-crate-only phase (small `cargo test -p`).
- A new wave starts only after the previous wave's PRs are merged and `main` is green — later phases branch off the merged seam, not a stale one.
- The main session is the only one that merges, prunes, or runs `cargo clean`.

### Watchdog (main session, `/loop` every 5 min while agents run)

`df -h /`, `free -g`, `du -sh target`, `docker ps --filter name=ide`, `git worktree list`.
Thresholds: disk < 20 GB free → do not spawn the next agent; < 12 GB → message every running agent to finish its current step and stop; RAM available < 6 GB → same. After each wave: prune build artifacts only if disk drops below 25 GB — otherwise leave the cache alone (it is what makes the next wave fast). Stale containers from E2E runs (`docker ps` names starting `ide-`) are killed by the main session at wave end; the user's other containers (jagdonline, postgres, mailpit, redis, signoz) are never touched.

### Waves (dependency-driven)

| Wave | Agents (parallel) | Depends on | Notes |
|---|---|---|---|
| W0 | **P0** | — | 1 agent: plan doc, ADRs, arch-doc skeleton, Windows cross-link spike, layering rows. Main session does pre-flight reclaim + image rebuild in parallel (no cargo build overlap). |
| W1 | **G1** ∥ **F1** | P0 | 2 agents, both compile `ui-shell`. G1 = plugin points + factories; F1 = `secret-store`, `db-core`, `db-drivers` (sqlite, postgres), settings, Data Source dialog. Disjoint files except one factory-registration line each. |
| W2 | **F2** ∥ **F3** ∥ **F8a** | W1 merged | F2 tree dock, F3 console + grid (both `ui-shell`); F8a = `db-driver-adbc` + `db-driver-odbc` crates only (Qt-free, no UI) — the allowed third agent. |
| W3 | **F4** ∥ **F5** ∥ **F6** | W2 merged | F4 data editor (`ui-shell`), F5 exchange (mostly `db-exchange` + dialogs), F6 diagrams/compare (mostly `db-exchange` + preview/DiffView reuse). F5 and F6 are light on `ui-shell`; still count as 2 + 1 — F6 waits until F5's first full build is done if RAM is tight. |
| W4 | **F7** ∥ **F8b** | W3 merged | F7 NoSQL drivers + `RusshTunnel` (`ui-shell` for tree kinds/console); F8b driver install UI + catalogue rows + bundle check. |
| W5 | **F9** | W4 merged | 1 agent: architecture-doc truth-up, i18n sweep, ADRs → Accepted, Windows manual matrix. Main session: final `cargo clean` of dead worktree artifacts, prune worktrees/branches/containers. |

Critical path: P0 → F1 → F3 → F4 → F7 → F9 (six sequential PRs); everything else overlaps.
Nightly `linux-db` image and compose file land in F1 (services for postgres) and grow per wave; nightly runs are not on the critical path.

## 12. User answers (2026-09-21)

1. SQL Server: no `tiberius`; ADBC/ODBC — decision 7 stands.
2. SSH: `russh` fallback is in scope. `ssh` CLI stays the first choice (agent/FIDO/ProxyJump/config for free); when the CLI is absent (Windows without OpenSSH) or the bastion needs password/key-passphrase auth, the tunnel runs in-process on `russh` 0.63 (pure Rust, `ring`). Shape: `db_core::tunnel::Tunnel` trait with `CliTunnel` in `db-core` and `RusshTunnel` in `db-drivers` (it is async and needs the private runtime; `db-core` stays tokio-free). Known-hosts: read `~/.ssh/known_hosts`, prompt on unknown key with the fingerprint, never auto-accept. Lands in F7 together with SSH password auth, which `russh` answers directly — the `pty-core` scripted-session debt item is dropped.
3. `allow_third_party_drivers = false` default: accepted.
4. `libodbc.so.2` in the Linux bundle and the MSSQL nightly service: accepted.
5. History: always on, per-source `history = false` switch (shown beside Read-only in the Data Source dialog) and a "Clear history" action; statement text is stored, bound parameters never.

## 13. P0 cross-link spike result

A throwaway crate `xlink-spike` outside the workspace (`/tmp/.../scratchpad/xlink-spike/`, never committed to this repo) depended on every crate in §3's driver catalogue, one referenced symbol per crate so nothing dead-strips before linking, mounting the shared `ide-cargo-registry` volume but a private `CARGO_TARGET_DIR` inside the spike directory.

**Linux link (`ide-linux-builder`, rustc 1.98.1).**
Every crate compiled and linked **except** `odbc-api`, which failed with `unable to find library -lodbc` — `unixodbc-dev` is not installed in the current `linux-builder` image (confirms R3 exactly as predicted).
Installing `unixodbc-dev libsqliteodbc` into a throwaway container layer (`apt-get`, committed to a disposable image, discarded after the spike) made the full set link, `mongodb` excepted (below).
Release binary size for the full set minus `mongodb`: 448 KB — not representative of the driver crates' real linked-in code size, because the spike references one symbol per crate rather than exercising real functionality that would pull in the actual query/TLS/codec paths; treat the NFR's own "+≤20 MB" budget as the number to verify against once real driver code exists, not this figure.

**`mongodb` 3.9.1 does not compile at all** under this repo's pinned `rustc` 1.98.1: `error[E0659]: 'doc' is ambiguous`, 1226 downstream errors, from `macro_magic`'s `forward_tokens` macro re-exporting `bson::doc` into scope where it collides with the builtin `#[doc]` attribute (`action/search_index.rs`).
This is new — R11, not previously in the plan's risk table (§7).
It blocks F7's mongodb driver until resolved; the fix is upstream (a `macro_magic`/`mongodb` version bump, or reduced feature set) and is not attempted here, since it needs no crate in this repository to exist yet.

**`aws-lc-rs` is not empty**, contrary to decision 6's original wording.
`cargo tree -i aws-lc-rs` in the spike:

```
aws-lc-rs v1.18.1
├── russh v0.63.3
├── rustls v0.23.45
│   ├── redis v1.7.0
│   ├── scylla v1.9.0
│   ├── tokio-postgres-rustls v0.14.0
│   ├── tokio-rustls v0.26.5
│   │   ├── scylla v1.9.0 (*)
│   │   └── tokio-postgres-rustls v0.14.0 (*)
│   └── xlink-spike v0.1.0
└── rustls-webpki v0.103.15
    └── rustls v0.23.45 (*)
```

The spike's own root `rustls = { default-features = false, features = ["ring","std","tls12"] }` entry does not suppress this: Cargo unifies features for one resolved version of a crate across the whole dependency graph, so if `russh`, `redis`, `scylla` or `tokio-postgres-rustls` each specify their *own* `rustls` requirement without disabling its default `aws_lc_rs` feature, that feature turns on everywhere `rustls` is used, including in code this repository's own root manifest entry never asked to compile.
This is exactly R2's predicted failure mode, now confirmed rather than assumed; the mitigation is a per-driver-crate feature audit against each dependency's own default-feature list (recorded as a task in F1/F7/F8, not a fact decision 6 could claim as already true) — this is corrected in §3/§7 above.

**Windows cross-link (`ide-windows-builder`).**
The image's pinned Rust toolchain is stale relative to `docker/Dockerfile`'s current `RUST_VERSION=1.98.1` ENV — it still runs rustc 1.90.0.
Building against that toolchain failed immediately on `windows-registry`/`windows-result`/`windows-strings` 0.100.0, which require rustc ≥ 1.95 (`keyring`'s Windows backend pulls the `windows` crate family).
`cargo update -p windows-registry --precise 0.6.1` (which resolved compatible `windows-result`/`windows-strings` transitively) is the same kind of workaround `windows-builder`'s own Dockerfile history already documents for MXE-cross-build friction, and unblocked the build: with `mongodb` excluded (same reason as Linux) the crate cross-compiled and linked cleanly to `x86_64-pc-windows-gnu`, producing a valid PE32+ executable, both `dev` (61.7 MB, unstripped debug) and `release` (1.18 MB) profiles.
The stale image is recorded as a pre-flight task (§11 step 3) rather than fixed here — rebuilding `windows-builder` from `mxe-base` (no MXE rebuild needed, only the `rustup`/`cargo` layer) is a task for the main session before a wave that needs a fresh cross-link spike, not something this spike should do given it has no mandate to modify shared images.

**Versions resolved** (`Cargo.lock`, matching the plan's pinned versions exactly): `tokio-postgres` 0.7.18, `tokio-postgres-rustls` 0.14.0, `mysql_async` 0.37.1, `rusqlite` 0.40.2, `redis` 1.7.0, `scylla` 1.9.0, `adbc_core`/`adbc_driver_manager` 0.24.0, `arrow-array`/`-schema` 59.3.0, `odbc-api` 29.1.0, `russh` 0.63.3, `sqlparser` 0.63.0, `sqlformat` 0.5.0, `rust_xlsxwriter` 0.99.1, `calamine` 0.36.1, `csv` 1.4.0, `sha2` 0.11.0 (a second `sha2` 0.10.9 also resolves, pulled transitively by `mysql_async`/`rusqlite`'s own dependency graph — two major `sha2` lines coexisting is normal Cargo resolution, not a conflict), `keyring` 3.6.3, `rustls` 0.23.45.

**Disk.** `df -h /` before: 43 GB free. Lowest point during the spike (mid Windows-release build): 38 GB free. After deleting the spike's `target/` (5.5 GB peak) and the throwaway `unixodbc-dev` image layer: 43 GB free again — never below the 15 GB floor.

**Summary for §7/§3**: everything in the driver catalogue links on both platforms except `mongodb` (R11, blocks F7 until resolved) and `odbc-api` on Linux without `unixodbc-dev` (R3, confirmed, fix lands in F8.6). `aws-lc-rs` needs active per-crate exclusion work, not a passive pin (R2, corrected). The Windows toolchain image needs a rebuild before the next phase that changes a Windows-linked crate runs its own spike.
