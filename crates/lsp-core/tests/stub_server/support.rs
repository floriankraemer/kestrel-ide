//! Shared harness for the `stub_server` integration-test binary's topic
//! modules (originally split out of `stub_server_session.rs`, #162):
//! stub-server configs, the polling helper, and the two session setups
//! every topic module needs. Declared once as `mod support;` in
//! `tests/stub_server/main.rs`; each topic module pulls it in with
//! `use crate::support::*;`.
//!
//! No `#![allow(dead_code)]` needed: all topic modules now share one
//! binary, so a helper only one module uses is not dead code crate-wide.

pub use std::sync::mpsc::Receiver;
pub use std::time::{Duration, Instant};

pub use lsp_core::catalog::ServerConfig;
#[allow(unused_imports)]
pub use lsp_core::manager::LspError;
pub use lsp_core::manager::{LspEvent, LspManager};
pub use serde_json::json;

pub const STUB: &str = env!("CARGO_BIN_EXE_stub_server");
pub const LANG: &str = "stub";

pub fn config(command: &str, args: &[&str]) -> ServerConfig {
    ServerConfig {
        id: LANG.into(),
        language_id: LANG.into(),
        name: "stub".into(),
        command: command.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        enabled: true,
        settings_section: None,
        settings: serde_json::Value::Null,
        initialization_options: serde_json::Value::Null,
        diagnostics: true,
        posix_only: false,
        exec: lsp_core::catalog::ServerExec::Host,
        priority: 0,
        source: lsp_core::catalog::ServerSource::Builtin,
    }
}
pub fn stub_config() -> ServerConfig {
    config(STUB, &[])
}
/// The stub dies mid-session on the first `didOpen`. The env var is passed
/// through `env(1)` so the test process' own environment stays untouched —
/// integration tests share one process.
pub fn dying_stub_config() -> ServerConfig {
    config("env", &["STUB_LSP_DIE_ON_DIDOPEN=1", STUB])
}
/// C6: a stub configured with a `workspace/configuration` section and
/// starting settings, the way the `csharp` plugin's `ServerConfig` is.
pub fn stub_config_with_settings() -> ServerConfig {
    ServerConfig {
        settings_section: Some("csharp".into()),
        settings: json!({"analyzersEnabled": true}),
        ..stub_config()
    }
}
/// C7: the stub advertises `completionProvider.resolveProvider: true`, the
/// way csharp-ls does.
pub fn stub_config_with_completion_resolve() -> ServerConfig {
    config("env", &["STUB_LSP_COMPLETION_RESOLVE=1", STUB])
}
/// C9: the stub advertises `semanticTokensProvider` statically in
/// `initialize`'s result, the way rust-analyzer does.
pub fn stub_config_with_semantic_tokens_static() -> ServerConfig {
    config("env", &["STUB_LSP_SEMANTIC_TOKENS_STATIC=1", STUB])
}
/// Drain events until one matches, or fail. Non-matching events are skipped:
/// a server may legitimately emit log notifications we don't care about.
pub fn wait_for<T>(
    rx: &Receiver<LspEvent>,
    what: &str,
    mut pick: impl FnMut(&LspEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = rx
            .recv_timeout(remaining)
            .unwrap_or_else(|e| panic!("waiting for {what}: {e}"));
        if let Some(value) = pick(&event) {
            return value;
        }
    }
}
pub fn formatting_options(tab_size: u32) -> lsp_core::formatting::FormattingOptions {
    lsp_core::formatting::FormattingOptions {
        tab_size,
        ..lsp_core::formatting::FormattingOptions::default()
    }
}
/// A started stub with one open document, which is what every F2 request
/// needs before it can resolve the document's language.
pub fn session_with_open_document() -> (LspManager, Receiver<LspEvent>, &'static str) {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn main() {}")
        .expect("didOpen");
    (manager, rx, uri)
}
/// C10: the stub advertises `codeLensProvider` statically in `initialize`'s
/// result, the way rust-analyzer would.
pub fn stub_config_with_code_lens_static() -> ServerConfig {
    config("env", &["STUB_LSP_CODE_LENS_STATIC=1", STUB])
}
/// C11: the stub advertises `callHierarchyProvider` statically in
/// `initialize`'s result.
pub fn stub_config_with_call_hierarchy_static() -> ServerConfig {
    config("env", &["STUB_LSP_CALL_HIERARCHY_STATIC=1", STUB])
}
/// C11: the type-hierarchy twin of `stub_config_with_call_hierarchy_static`.
pub fn stub_config_with_type_hierarchy_static() -> ServerConfig {
    config("env", &["STUB_LSP_TYPE_HIERARCHY_STATIC=1", STUB])
}

/// E1/L3: a stub with the tagged profile (`src/bin/stub_server/profile.rs`),
/// running as server `id` for [`LANG`] and advertising `caps`.
pub fn tagged_config(id: &str, caps: &str) -> ServerConfig {
    let tag = format!("STUB_LSP_TAG={id}");
    let caps = format!("STUB_LSP_CAPS={caps}");
    ServerConfig {
        id: id.into(),
        ..config("env", &[&tag, &caps, STUB])
    }
}
