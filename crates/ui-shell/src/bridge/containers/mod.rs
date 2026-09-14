//! Containers (ADR-0055): the Settings > Containers accessors on
//! `AppSettings` ([`settings`], C1), the `ContainerService` QObject behind
//! the Containers dock ([`service`], C2), its lifecycle actions
//! ([`actions`], C3), its streaming sessions ([`sessions`], C3), and images/
//! networks/volumes ([`images`]/[`networks`]/[`volumes`], C4), the
//! editor's compose lenses + "Pull image" ([`editor`], C6), and registries
//! ([`registries`], C7). One
//! directory rather than one file so each task's surface lands as its own
//! module under the size ceiling — every one of these adds its own `impl
//! ffi::ContainerService` block, the same split
//! `bridge/language/{mod,lsp_surface,refactor}.rs` already uses for one
//! QObject's surface across files.

mod actions;
/// The container node's Dashboard tab and "Recreate with changes" (C9).
mod dashboard;
mod editor;
mod images;
mod networks;
mod registries;
mod service;
mod sessions;
mod settings;
mod volumes;

/// C7: `bridge/language/containers.rs` needs the configured registry list
/// (for `<address>/` completion matching) from outside this module's own
/// subtree — re-exported rather than making `service` itself `pub(crate)`,
/// so everything else in it stays reachable only from siblings, as before.
pub(crate) use service::configured_registries;
pub use service::ContainerServiceRust;
