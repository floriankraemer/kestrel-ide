//! Debugging (D1): a Debug Adapter Protocol client.
//!
//! Shaped like `lsp-core` deliberately (ADR-0041): a supervised child
//! process, `Content-Length` framing — the same framing, shared through
//! `stdio-framing` rather than written twice — blocking request/response on
//! plain threads, and a catalog of adapters layered under the project's own
//! overrides. One client, N adapters: codelldb for Rust and C/C++, debugpy
//! for Python, java-debug for the JVM.
//!
//! Which adapter a project implies is `run_core::toolchain`'s answer
//! (ADR-0039), not a second table here.
//!
//! Qt-free and tokio-free by design (see `docs/architecture/layering.md`'s
//! `dap-core` row): long work runs on a `std::thread` and results reach the
//! UI through `CxxQtThread::queue()`, never an ambient runtime.

pub mod breakpoints;
pub mod catalog;
pub mod error;
pub mod inline_values;
pub mod launch;
pub mod protocol;
pub mod session;

pub use breakpoints::{Breakpoint, BreakpointStore, SuspendPolicy};
pub use catalog::Adapter;
pub use error::DapError;
pub use inline_values::{inline_values, InlineValue};
pub use protocol::{Capabilities, Message, Scope, StackFrame, Stopped, Thread, Variable};
pub use session::{local_path, source_path, DapSession, SessionListener};
