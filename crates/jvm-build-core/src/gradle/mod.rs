//! The Gradle provider (A4): the init-script asset/argv, the JSON model
//! parser, and running a sync through the project's own wrapper.

pub mod init_script;
pub mod model_json;
pub mod sync;

pub use model_json::parse as parse_model;
pub use sync::{sync as run_sync, SyncError, SyncOptions};
