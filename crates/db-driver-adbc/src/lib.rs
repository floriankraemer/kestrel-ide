//! `db_core::Driver` over Apache ADBC (F8.1, database-tools-plan.md): a
//! generic bridge to any ADBC driver (DuckDB, and whichever of
//! mssql/snowflake/bigquery/clickhouse/trino F8.3's spike found a pinnable
//! artifact for — now `builtin/database-tools/plugin.toml`'s
//! `database-drivers` rows, F8b). The only Arrow-aware code in the
//! workspace lives in [`arrow`] (layering.md's `db-driver-adbc` row).

pub mod arrow;
mod driver;
pub mod install;
mod introspect;
pub mod locate;
pub mod quarantine;

pub use driver::AdbcDriver;
