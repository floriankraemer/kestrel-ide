//! Containers (ADR-0055): the Settings > Containers accessors on
//! `AppSettings` ([`settings`], C1), the `ContainerService` QObject behind
//! the Containers dock ([`service`], C2), its lifecycle actions
//! ([`actions`], C3) and its streaming sessions ([`sessions`], C3). One
//! directory rather than one file so each later task's surface (C4 images,
//! ...) lands as its own module under the size ceiling — `actions.rs` and
//! `sessions.rs` each add their own `impl ffi::ContainerService` block,
//! the same split `bridge/language/{mod,lsp_surface,refactor}.rs` already
//! uses for one QObject's surface across files.

mod actions;
mod service;
mod sessions;
mod settings;

pub use service::ContainerServiceRust;
