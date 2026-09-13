//! What a launch is made of: a run configuration, the tasks that run before
//! it, and the debug adapter overrides a project may carry.
//!
//! Dumb persistence, like every other table in this crate: strings and
//! bools, no interpretation. What a toolchain id, a task kind or an adapter
//! id *means* is `run-core`'s and `dap-core`'s (ADR-0039, ADR-0041), which
//! is what keeps this crate depending on nothing.

use serde::{Deserialize, Serialize};

use crate::container_run::{ComposeRunSetting, ContainerImageRunSetting, ContainerfileRunSetting};
use crate::is_false;

/// One entry in a run configuration's `before_launch` list (B2-1).
///
/// A string `kind` plus the fields each kind needs, rather than a tagged
/// enum, for the same reason `RunConfigSetting::toolchain` is a string
/// (ADR-0039): what a task *means* is `run-core`'s, and this crate depends
/// on nothing. A kind this version does not know loses that one task
/// instead of failing the whole settings file.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BeforeLaunchSetting {
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// One `[[run_config]]` entry: a project-defined launch target (F4-4, ADR-0022).
///
/// Lives in the project layer, not here — see [`project_settings`] — but the
/// struct itself sits in this crate like `LanguageServerSetting` does, kept
/// out of `run-core` so persistence stays dumb (ADR-0017): `run-core`
/// re-exports this exact type as its own `RunConfig` rather than keeping a
/// second struct in sync with a manual mapping.
///
/// `id` is a stable opaque string, issued once and independent of `name`:
/// renaming or re-editing a configuration must not change what re-runs and
/// what persistence keys on, the same guarantee `app_core::TabId` gives tabs.
/// Environment values are stored literally — a config referencing a secret
/// is the user's problem, and the docs say so.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct RunConfigSetting {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// The build tool this configuration belongs to, as
    /// `run_core::ToolchainId::as_str` spells it — `None` for a hand-written
    /// one (R1-2).
    ///
    /// A plain string rather than the enum: the toolchain table is
    /// `run-core`'s, and `docs/architecture/layering.md` has this crate
    /// depending on nothing, so persistence stays dumb and `run-core` maps
    /// the string back. An unknown value therefore loads as "no toolchain"
    /// instead of failing the whole settings file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    /// What the toolchain runs — a Cargo bin, an npm script, a Make target.
    /// `None` for a hand-written configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// A configuration created on the fly by running from context, kept only
    /// until the temporary cap evicts it. Never written by the editor.
    #[serde(default, skip_serializing_if = "is_false")]
    pub temporary: bool,
    /// Whether a second launch opens a second console instead of replacing
    /// the running one.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_parallel: bool,
    /// What has to happen before this configuration's program starts
    /// (B2-1), in order. Empty for a configuration with nothing to prepare.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before_launch: Vec<BeforeLaunchSetting>,
    /// What kind of thing this configuration launches (C5, ADR-0056):
    /// `None`/`"process"` for today's plain program+args (the only kind
    /// before this field existed), or `"container-image"` /
    /// `"containerfile"` / `"compose"` — each paired with exactly one of
    /// [`RunConfigSetting::container_image`], [`RunConfigSetting::containerfile`]
    /// or [`RunConfigSetting::compose`]. A string rather than an enum for
    /// the same reason as [`RunConfigSetting::toolchain`] (ADR-0039):
    /// persistence stays dumb, and `run_core::RunConfigExt` maps the string
    /// back, so an unrecognised kind loads as a plain process rather than
    /// failing the whole settings file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_image: Option<ContainerImageRunSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containerfile: Option<ContainerfileRunSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose: Option<ComposeRunSetting>,
}

/// One `[[debug_adapter]]` entry: what the user says about the debug adapter
/// with this id (D1-4).
///
/// The same shape and the same job as [`LanguageServerSetting`]: replace the
/// command of an adapter the IDE ships knowledge of, or introduce one it has
/// never heard of. What an adapter *is* stays `dap-core`'s.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct DebugAdapterSetting {
    /// Adapter id, e.g. `"codelldb"`. The key both the shipped catalog and
    /// this table are keyed by.
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}
