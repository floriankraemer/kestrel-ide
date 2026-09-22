# DB integration: checking the database drivers against real engines

`docker/db-compose.yml`, `make test-db`/`db-ci` and the `db-integration` feature landed in F1 for PostgreSQL; F7/FX added `mongo`, `redis`, `scylla` and `sshd` services to the same compose file, and `db-ci` now passes every service's env into the test run and runs the `#[ignore]`d sshd tunnel test explicitly (phase FX); FZ added `mysql`/`mariadb` services and the native driver they exercise.
SQL Server (`mssql/server:2022`) still has no compose service; `make e2e-db` and a dedicated `linux-db` Docker stage are still not built — both remain target design, not shipped.
`db-ci` runs `db-drivers`' whole `db-integration`-gated suite (`cargo nextest run -p db-drivers --features db-integration`), plus a second, explicit `--run-ignored only ssh` pass for the sshd-backed tunnel test (`Makefile`'s `db-ci` target) — the tunnel test is `#[ignore]`d so an ordinary `cargo nextest run -p db-drivers --features db-integration` (and `make test`, which never enables the feature at all) does not need the `sshd` compose service up.
`test-db` itself still runs inside the existing `linux-builder` image over `--network host` rather than a dedicated `linux-db` stage — every service is reached on its published loopback port (below), so no client tool (`pg_dump`, `mysqldump`, …) has needed installing yet. The `linux-db` stage (client tools: `postgresql-client`, `default-mysql-client`, `mongodb-database-tools`, `unixodbc` + `libsqliteodbc`) is still planned for F8.6-equivalent follow-up work, once a real client-tool binary is actually exercised by a test.

Every unit test in `db-drivers`/`db-driver-adbc`/`db-driver-odbc` runs against SQLite (in-process, no server needed) or a fixture-recorded wire response.
That proves the row-mapping and SQL-generation code is right about a snapshot of what an engine once returned.
It does not prove a real PostgreSQL, MySQL, MongoDB, Redis, Scylla or SQL Server still speaks the way the fixture says it does, or that this codebase's own introspection queries still parse a current engine's system catalog.

This suite closes that gap, the same way `docs/architecture/jvm-integration.md` closes it for Gradle/Maven and `docs/architecture/lsp-conformance.md` closes it for the LSP client.

## Running it

```sh
make test-db
```

`make test-db` brings up every service in `docker/db-compose.yml` (`docker compose -f docker/db-compose.yml up -d --wait`), runs `db-ci` inside `linux-builder` over `--network host` with one env var per service, and tears the compose stack down (`down -v`) whether or not the tests passed:

| Env var | Value | Service |
|---|---|---|
| `IDE_DB_POSTGRES_URL` | `postgres://ide:ide@127.0.0.1:55432/ide_test` | `postgres` (`postgres:17`) |
| `IDE_DB_MONGO_URL` | `mongodb://127.0.0.1:55017` | `mongo` (`mongo:8`) |
| `IDE_DB_REDIS_URL` | `redis://127.0.0.1:56379` | `redis` (`redis:7`) |
| `IDE_DB_CASSANDRA_HOSTS` | `127.0.0.1:59042` | `scylla` (`scylladb/scylla:2026.2`) |
| `IDE_DB_MYSQL_URL` | `mysql://ide:ide@127.0.0.1:53306/ide_test` | `mysql` (`mysql:8.4`) |
| `IDE_DB_MARIADB_URL` | `mysql://ide:ide@127.0.0.1:53307/ide_test` | `mariadb` (`mariadb:11`) |
| `IDE_DB_SSH_HOST` / `IDE_DB_SSH_PORT` / `IDE_DB_SSH_USER` / `IDE_DB_SSH_PASSWORD` | `127.0.0.1` / `52222` / `ide` / `ide` | `sshd` (`linuxserver/openssh-server:10.3_p1-r1-ls237`), tunnelling to `postgres` |

Every port is published bound to `127.0.0.1` only (ADR-0061 §5's "never a wildcard address" posture, applied to this throwaway nightly/on-demand fixture stack too, not just the SSH tunnel's forwarded port).
`db-ci` (inside the image) runs `cargo nextest run -p db-drivers --features db-integration`, then a second explicit pass, `cargo nextest run -p db-drivers --features db-integration --run-ignored only ssh`, for the sshd-backed tunnel test alone — it stays `#[ignore]`d so neither `make test` nor a bare `cargo nextest run --features db-integration` needs the `sshd` service up.
`make e2e-db` (an `IDE_E2E_DB=1`-gated E2E flow per backend family, click-driven rather than `db-drivers`' own unit-level integration tests) is still not built — planned, not shipped; the E2E flows this branch does ship (`e2e_database`, `e2e_database_console`) run entirely against SQLite, per §"Why it is not a per-PR gate" below.

The `db-integration`-feature tests are gated behind that Cargo feature, not `#[ignore]` (the sshd tunnel test is the one exception, `#[ignore]`d *within* the feature for the reason above), so `cargo test --workspace`/`make test` never builds or runs any of them — the feature simply is not enabled there, exactly `jvm-integration`'s own shape.

## Why it is not a per-PR gate

No PostgreSQL, MySQL, MariaDB, MongoDB, Redis, Scylla or SQL Server server exists in `linux-builder` by default, and standing one up for every PR would slow `make test`/`make lint` for work most PRs never touch — the same `lsp-conformance`/`jvm-integration` reasoning: a separate image and a nightly/on-demand run, not a gate every commit pays for.
The per-PR gate instead runs `e2e_database`, one flow, entirely against SQLite (`db-drivers`' `sqlite` feature is required by `e2e` for exactly this reason) — real, but not a real *server*.

## What it verifies

Services in `docker/db-compose.yml` today: `postgres:17`, `mysql:8.4`, `mariadb:11`, `mongo:8`, `redis:7`, `scylladb/scylla:2026.2`, `linuxserver/openssh-server:10.3_p1-r1-ls237`.
`mcr.microsoft.com/mssql/server:2022` has no compose service yet — still planned, tracked as open follow-up work rather than shipped.
`.github/workflows/nightly.yml` gaining a `db-integration` job with these services under `services:` is likewise still open.

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
F7 added the `mongo`/`redis`/`scylla`/`sshd` services to `db-compose.yml` and the `mongodb`/`redis`/`cassandra` drivers plus `RusshTunnel`.
Phase FX wired `db-ci` to actually reach every one of those services (the env-var table above) and to run the sshd-backed tunnel test explicitly (`--run-ignored only ssh`) — the "not extended yet" gap this section used to describe is closed.
Phase FZ added the `mysql`/`mariadb` services, the `mysql.rs` native driver (both engines share it, `family = "mysql"`), and `SessionWorker`'s idle-close/reconnect timer.
`make e2e-db`, a dedicated `linux-db` image stage, and SQL Server's compose service remain open — not built on this branch, no phase claims otherwise.
