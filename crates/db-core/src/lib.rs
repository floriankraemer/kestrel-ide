//! `db-core`: the Qt-free, tokio-free heart of Database Tools (ADR-0058) —
//! the value model, the driver seam every backend implements, the schema
//! tree, the DML-plan builder, and the persistence-shaped helpers
//! (`console`/`history`/`datasource`) every consumer (`db-sql`,
//! `db-exchange`, `db-drivers`, `ui-shell`) programs against instead of
//! re-deriving a rule apiece.
//!
//! No tokio, no `secret-store`: secrets are read by `ui-shell` and handed
//! in as [`datasource::Secrets`] — the crate that owns a rule never owns
//! the OS integration reading its input (`layering.md`'s `db-core` row).

pub mod console;
pub mod datasource;
pub mod ddl;
pub mod dialect;
pub mod dml;
pub mod driver;
pub mod error;
pub mod history;
pub mod readonly;
pub mod result;
pub mod schema;
pub mod session;
pub mod tree;
pub mod tunnel;
pub mod value;
