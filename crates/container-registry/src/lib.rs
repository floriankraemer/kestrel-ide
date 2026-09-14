//! Network clients for container registries (Qt-free, ADR-0055).
//!
//! C6 lands Docker Hub search and tag listing ([`hub`]) behind the
//! editor's image-name completion. C7's registry-catalog client (the V2
//! token dance, catalog/tags for Hub, GitLab, plain V2 and generic
//! registries) and its `keyring` credential store belong in this same
//! crate — it exists precisely to be the one home for everything in the
//! container integration that talks HTTP.
//!
//! Split from `container-core` on purpose: `reqwest`'s blocking client
//! brings its own private tokio runtime into the dependency tree, and
//! `container-core` sits beneath `run-core`/`build-core`/`dap-core`, whose
//! layering gate (`.github/workflows/ci.yml`, `docs/architecture/
//! layering.md`) forbids tokio anywhere in their transitive tree. Nothing
//! here uses tokio directly; every call is a blocking request on a
//! `std::thread` the caller owns, the same shape `ai-chat-core` uses.

/// Docker Hub's public API: repository search and a repository's tags.
pub mod hub;
