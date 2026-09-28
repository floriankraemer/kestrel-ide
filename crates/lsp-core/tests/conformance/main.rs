//! One integration-test binary for the real-server conformance suites, so
//! the crate's dependency tree links once instead of once per file.
//! `support` is the server-agnostic plumbing shared by both suites.
mod support;

mod csharp_conformance;
mod real_server_conformance;
