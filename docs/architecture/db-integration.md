# DB integration: checking the database drivers against real engines

`docker/db-compose.yml`, `make test-db`/`db-ci` and the `db-integration` feature landed in F1 for PostgreSQL; F7 added `mongo`, `redis`, `scylla` and `sshd` services to the same compose file (env `IDE_DB_MONGO_URL`, `IDE_DB_REDIS_URL`, `IDE_DB_CASSANDRA_HOSTS`, `IDE_DB_SSH_HOST`/`IDE_DB_SSH_PORT`/`IDE_DB_SSH_USER`/`IDE_DB_SSH_PASSWORD`); MSSQL (F8) and `make e2e-db` are still target design.
`db-ci` itself still only runs `db-drivers`' Postgres suite (`cargo nextest run -p db-drivers --features db-integration`) — F7's `mongodb.rs`/`redis.rs`/`cassandra.rs`/`ssh.rs` integration tests exist and compile (gated behind the same `db-integration` feature, `#[ignore]`d) but were not run against a live server in this sandbox (no network services available there); wiring `db-ci` to bring up and reach the new services is the next increment, not done in this pass.
F1's `db-ci` deliberately does **not** get its own `linux-db` Docker stage yet — it runs `db-drivers`' PostgreSQL integration tests inside the existing `linux-builder` image over `--network host`, reaching `docker/db-compose.yml`'s `postgres` service on its published loopback port. The `linux-db` stage (client tools: `postgresql-client`, `default-mysql-client`, `mongodb-database-tools`, `unixodbc` + `libsqliteodbc`) lands in F8.6 alongside the ODBC driver's own Dockerfile change, once a real client-tool binary (`pg_dump`, `mysqldump`, …) is actually exercised by a test — nothing in F1's PostgreSQL suite needs one.

Every unit test in `db-drivers`/`db-driver-adbc`/`db-driver-odbc` runs against SQLite (in-process, no server needed) or a fixture-recorded wire response.
That proves the row-mapping and SQL-generation code is right about a snapshot of what an engine once returned.
It does not prove a real PostgreSQL, MySQL, MongoDB, Redis, Scylla or SQL Server still speaks the way the fixture says it does, or that this codebase's own introspection queries still parse a current engine's system catalog.

This suite closes that gap, the same way `docs/architecture/jvm-integration.md` closes it for Gradle/Maven and `docs/architecture/lsp-conformance.md` closes it for the LSP client.

## Running it

```sh
make test-db
make e2e-db
```

`make test-db` brings up `docker/db-compose.yml`'s services (F1: `postgres:17` only), then runs `db-ci` (`cargo nextest run -p db-drivers --features db-integration`, growing to `-p db-driver-adbc -p db-driver-odbc` once those crates exist) inside `linux-builder` over `--network host`, and tears the compose stack down whether or not the tests passed.
`make e2e-db` runs the `IDE_E2E_DB=1`-gated E2E flows (connect/introspect/execute against a real engine, one flow per backend family) under Xvfb, the same shape `e2e_build_tools.rs` already uses for `IDE_E2E_JVM=1`.

The `db-integration`-feature tests are gated behind that Cargo feature, not `#[ignore]`, so `cargo test --workspace`/`make test` never builds or runs them — the feature simply is not enabled there, exactly `jvm-integration`'s own shape.

## Why it is not a per-PR gate

No PostgreSQL, MySQL, MariaDB, MongoDB, Redis, Scylla or SQL Server server exists in `linux-builder`, and standing one up for every PR would slow `make test`/`make lint` for work most PRs never touch — the same `lsp-conformance`/`jvm-integration` reasoning: a separate image and a nightly/on-demand run, not a gate every commit pays for.
The per-PR gate instead runs `e2e_database`, one flow, entirely against SQLite (`db-drivers`' `sqlite` feature is required by `e2e` for exactly this reason) — real, but not a real *server*.

## What it verifies

Services come from `docker/db-compose.yml`: `postgres:17`, `mysql:8.4`, `mariadb:11`, `mongo:8`, `redis:7`, `scylladb/scylla:6`, and (from F8) `mcr.microsoft.com/mssql/server:2022`.
`.github/workflows/nightly.yml` gains a `db-integration` job with the same services under `services:`.

Per backend, the integration tests exercise: connect (plaintext and TLS where the engine supports it), introspection (the real system catalog, not a fixture — this is what catches an engine-version drift a fixture cannot), execute + page + cancel against a real cursor, and the read-only classifier's server-side flag actually taking effect on a live session.
Fixture-based unit tests stay the first line of defense (fast, no server, run on every PR); this suite is the second line, proving the fixtures still describe reality.

## Why every version is pinned

An unpinned engine turns this suite into a random number generator: "the introspection query broke because PostgreSQL 18 renamed a system view" arriving as a red nightly build is exactly how a suite like this stops being trusted — the same reasoning `lsp-conformance.md`/`jvm-integration.md` both state for their own pinned dependencies.
Bumping a pin (an engine's major version, a client-tool version) is a deliberate commit with its own fixture updates, not a background `apt-get upgrade`.

## Manual DuckDB smoke test (F8b)

`db-driver-adbc`'s own unit tests never load a real shared library (`install_from_bytes`/`quarantine` are exercised against byte fixtures, per this doc's opening paragraph), and `make test-db` does not grow a DuckDB service — there is no server to bring up, DuckDB runs in-process.
Exercising the real load path — `AdbcDriver::connect` actually calling into a DuckDB shared library through `adbc_driver_manager` — is a manual step, done once per phase that touches `db-driver-adbc`'s loader, not a CI gate.

With no IDE-managed install present (`<config_dir>/database/drivers/duckdb/current/manifest.toml` absent), `AdbcDriver` falls back to `DriverLocation::SystemName("duckdb")`, which asks the ADBC driver manager to resolve `duckdb` through its own standard search (`LOAD_FLAG_SEARCH_SYSTEM`/`LOAD_FLAG_SEARCH_USER`/`LOAD_FLAG_SEARCH_ENV`) — the same path a user hits before ever pressing "Install..." in the Data Source dialog (F8.5's `SystemSearch` status).
To give that search something to find, install DuckDB's ADBC driver with `apache/arrow-adbc`'s own CLI:

```sh
pip install adbc_driver_manager   # ships the `dbc` console script
dbc install duckdb                # registers duckdb's ADBC driver in the user-level manifest search path
```

Then, inside `linux-builder` (`make shell`), add a `sqlite`-shaped data source through the running app (or a throwaway `cargo run -p app`) with driver `duckdb` and a `database` field pointing at a scratch `.duckdb` file, and confirm: connect succeeds, the tree introspects at least one empty schema, and `SELECT 1` executes.
A failure here with the driver actually installed points at `AdbcDriver`'s load/connect path (or an `AdbcVersion`/entrypoint mismatch) rather than at the install/quarantine machinery `db-driver-adbc`'s unit tests already cover.

## Status

F1 landed `db-compose.yml` (PostgreSQL only), `make test-db`/`db-ci`, `db-drivers`' `db-integration` feature, and the `sqlite`/`postgres` unit-tested backends themselves.
F7 added the `mongo`/`redis`/`scylla`/`sshd` services to `db-compose.yml` and the `mongodb`/`redis`/`cassandra` drivers plus `RusshTunnel` (all unit-tested against fixtures/fakes; `db-ci` itself was not extended to reach these new services yet).
`make e2e-db` and the `linux-db` image are still not built.
Later phases add MySQL/MariaDB/SQL Server's services, driver crates and tests, and wire `db-ci` to the F7 services, per `database-tools-plan.md`'s task list.
