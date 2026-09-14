//! The Maven provider (A5): static/effective POM reads, verbose
//! dependency-tree parsing, plugin-goal extraction, and running `mvn`.

pub mod dep_tree;
pub mod effective_pom;
pub mod goals;
pub mod pom;
pub mod sync;
mod xml;

pub use pom::parse as parse_pom;
pub use sync::{sync as run_sync, SyncError, SyncOptions};
