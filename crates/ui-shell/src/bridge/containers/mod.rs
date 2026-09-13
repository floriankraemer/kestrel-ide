//! Containers (ADR-0055): the Settings > Containers accessors on
//! `AppSettings` ([`settings`], C1), the `ContainerService` QObject behind
//! the Containers dock ([`service`], C2), its lifecycle actions
//! ([`actions`], C3), its streaming sessions ([`sessions`], C3), and images/
//! networks/volumes ([`images`]/[`networks`]/[`volumes`], C4), and the
//! editor's compose lenses + "Pull image" ([`editor`], C6). One
//! directory rather than one file so each task's surface lands as its own
//! module under the size ceiling — every one of these adds its own `impl
//! ffi::ContainerService` block, the same split
//! `bridge/language/{mod,lsp_surface,refactor}.rs` already uses for one
//! QObject's surface across files.

mod actions;
mod editor;
mod images;
mod networks;
mod service;
mod sessions;
mod settings;
mod volumes;

pub use service::ContainerServiceRust;
