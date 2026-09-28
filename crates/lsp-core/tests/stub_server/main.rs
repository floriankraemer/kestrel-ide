//! One integration-test binary for the `stub_server`-driven suite, so the
//! crate's dependency tree links once instead of once per topic. Each
//! module was originally its own `stub_server_*.rs` test file, split out of
//! `stub_server_session.rs` (#162); `support` is the shared harness.
mod support;

mod code_lens;
mod completion;
mod f2_surface;
mod formatting;
mod hierarchy;
mod lifecycle;
mod metadata;
mod navigation;
mod progress;
mod refactor;
mod registration;
mod semantic_tokens;
