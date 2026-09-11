//! Rules behind the Settings pages that have to interpret what they persist
//! — the three language-platform ones (plan tasks T4, G3, L6) and AI
//! Providers (AC12) — kept out of `ui-shell` because they deserve unit tests
//! and neither `bridge.rs` nor `cpp/` may hold a rule
//! (`docs/architecture/layering.md`).
//!
//! It exists as its own crate rather than living in `app-config` because
//! every one of these pages joins *persisted settings* to something that
//! knows what the settings mean — the syntax scope vocabulary and the
//! runtime language loader in `syntax-core`, the shipped server catalog in
//! `lsp-core` — and `app-config` deliberately depends on neither
//! (ADR-0017, and the module docs of `app_config::syntax_colors`).
//!
//! Qt-free, like every crate below `ui-shell`.

pub mod ai;
pub mod analysis;
pub mod editing;
/// Which handler a file-association rule (or a shipped default) names for a
/// path — see [`file_associations::resolve_handler`].
pub mod file_associations;
pub mod languages;
pub mod plugins;
pub mod scope;
pub mod servers;
pub mod syntax_colors;

pub use ai::{
    default_provider, default_providers, default_tool_policy, key_status, known_tools,
    set_tool_policy, tool_policy, validate as validate_provider, AiProviderDraft, AiProviderRow,
    DefaultProvider, KeyStatus, ProviderField, ProviderKind, ToolPolicy, ValidationProblem,
};
pub use editing::{resolve_for_language, EditingDraft, EditingField, EditingProblem, EditingRules};
pub use file_associations::{handler_name, resolve_handler, HandlerKind, ALL_HANDLERS};
pub use languages::{
    explain, scan_manifests, toggle, LanguageAction, LanguageRow, LanguageSource, LanguageStatus,
    LanguageToggle, ManifestInfo, Problem,
};
pub use plugins::{PluginProblem, PluginRow, PluginSource, PluginStatus, PluginToggle};
pub use scope::{origin, origin_for_view, resolve, Scope, ScopedField};
pub use servers::{can_have_server, lsp_language_id, ServerDraft, ServerRow, ServerRowStatus};
pub use syntax_colors::{
    ordered_scopes, scope_family, scope_sample, unknown_scope_warning, unknown_scopes, Origin,
    SyntaxColorDraft, FAMILY_ORDER,
};
