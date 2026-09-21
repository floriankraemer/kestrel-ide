//! The `sql-script` run configuration's own sub-table (database-tools-plan
//! F3.6, ADR-0056 shape): a `[[run_config]]` row whose `kind` is
//! `"sql-script"` pairs with exactly one of these, the same "one sub-table
//! per kind" shape [`crate::container_run`]'s three structs already give
//! the container kinds.
//!
//! Persistence only (ADR-0017): what `source_id`/`tx_mode` *mean* is
//! `run-core`'s and `ui-shell`'s, not this crate's.

use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !b
}

/// A `sql-script` run configuration's own options.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct SqlScriptRunSetting {
    /// Which `[database.sources]` row to run the script against.
    #[serde(default)]
    pub source_id: String,
    /// The `.sql` file to run, project-relative or absolute.
    #[serde(default)]
    pub file: String,
    /// `"single_transaction"` wraps the whole script in one transaction
    /// (any failure rolls everything back); anything else (including
    /// unset) auto-commits each statement — the same free-form-string
    /// shape `RunConfigSetting::kind` uses (ADR-0039), so an unrecognised
    /// value reads as the auto-commit default rather than failing the
    /// whole settings file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tx_mode: String,
    /// Halts the script at its first failing statement rather than running
    /// every statement regardless. Meaningless (and ignored) once
    /// `tx_mode` is `"single_transaction"`, which always stops and rolls
    /// back on the first failure.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stop_on_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_tx_mode_and_stop_on_error_write_nothing() {
        let text = toml::to_string(&SqlScriptRunSetting::default()).expect("serialize");
        assert!(!text.contains("tx_mode"));
        assert!(!text.contains("stop_on_error"));
    }

    #[test]
    fn round_trips_every_field() {
        let setting = SqlScriptRunSetting {
            source_id: "ds-1".to_string(),
            file: "sql/migrate.sql".to_string(),
            tx_mode: "single_transaction".to_string(),
            stop_on_error: true,
        };
        let text = toml::to_string(&setting).expect("serialize");
        let parsed: SqlScriptRunSetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, setting);
    }
}
