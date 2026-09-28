//! One integration-test binary for jvm-build-core, so the crate's
//! dependency tree links once instead of once per file. Both modules keep
//! their own `#![cfg(feature = "jvm-integration")]` as their first line.
mod gradle_integration;
mod maven_integration;
