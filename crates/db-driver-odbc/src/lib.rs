//! `db_core::Driver` over ODBC (F8.2, database-tools-plan.md): any data
//! source an ODBC driver manager entry exists for. Feature-gated
//! (`odbc`, default-on) so a box without `unixodbc-dev`'s header still
//! builds the rest of the workspace with `--no-default-features`
//! (ADR-0058, plan §13).

#![cfg(feature = "odbc")]

mod connect;
mod driver;
mod introspect;
mod values;

pub use connect::{build_connection_string, ConnectionString};
pub use driver::OdbcDriver;
