//! Containers (ADR-0055): the Settings > Containers accessors on
//! `AppSettings` ([`settings`], C1) and the `ContainerService` QObject
//! behind the Containers dock ([`service`], C2). One directory rather than
//! one file so each later task's surface (C3 ops, C4 images, ...) lands as
//! its own module under the size ceiling.

mod service;
mod settings;

pub use service::ContainerServiceRust;
