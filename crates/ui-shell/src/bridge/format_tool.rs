//! Reformat Code and format-on-save through a `formatters` contribution
//! (ADR-0070): php-cs-fixer, Pint or phpcbf instead of the language server.
//!
//! Which one runs is `settings_model::formatting::plan`'s rule; this file
//! resolves the program's host, runs it off the Qt thread and publishes the
//! result as minimal line edits through the same pending-edit path an LSP
//! reformat uses, so Ctrl+Z undoes the whole reformat in one step.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A Reformat Code run that has not finished within this is abandoned.
pub(crate) const FORMAT_TIMEOUT: Duration = Duration::from_secs(30);

/// Format-on-save blocks the Qt thread (the save has to wait for the text),
/// so it gets a much shorter leash.
pub(crate) const FORMAT_ON_SAVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything needed to run the configured formatter on a worker thread.
pub(crate) struct ToolFormat {
    def: analysis_core::FormatterDef,
    host: process_exec::host::ExecHost,
    root: PathBuf,
    php_binary: String,
}

impl ToolFormat {
    /// The tool the settings choose for `language_id`, or `None` when the
    /// language server formats. The program itself is not probed here:
    /// that can mean a `wsl.exe` or container round trip, so a missing tool
    /// is discovered by [`Self::run`] on the worker thread instead.
    pub(crate) fn resolve(language_id: &str, has_selection: bool) -> Option<Self> {
        let root = crate::bridge::convert::current_project_root()?;
        let settings = crate::bridge::convert::load_resolved_settings();
        let php = settings_model::php::resolve(&settings);
        let contributions: Vec<plugin_api::FormatterContribution> = plugin_host::registry()
            .formatters()
            .map(|(_, f)| f.clone())
            .collect();
        let settings_model::formatting::FormatPlan::Tool(id) = settings_model::formatting::plan(
            settings_model::formatting::configured_formatter(&php, language_id),
            language_id,
            has_selection,
            &contributions,
            |_| true,
        ) else {
            return None;
        };
        let contribution = contributions.iter().find(|c| c.id == id)?;
        Some(Self::for_contribution(contribution, &settings, root))
    }

    /// The formatter `id`, narrowed to the one rule `code` names (Q7: "Fix
    /// with phpcbf"), regardless of which formatter the settings choose.
    /// `None` when no such formatter exists or it declares no `fix-args`.
    pub(crate) fn fixer(id: &str, code: &str) -> Option<Self> {
        let root = crate::bridge::convert::current_project_root()?;
        let settings = crate::bridge::convert::load_resolved_settings();
        let (_, contribution) = plugin_host::registry()
            .formatters()
            .find(|(_, f)| f.id == id)
            .map(|(p, f)| (p.id().to_string(), f.clone()))?;
        let mut tool = Self::for_contribution(&contribution, &settings, root);
        tool.def = tool.def.for_rule(code)?;
        Some(tool)
    }

    fn for_contribution(
        contribution: &plugin_api::FormatterContribution,
        settings: &app_config::Settings,
        root: PathBuf,
    ) -> Self {
        Self {
            host: crate::bridge::php::tool_host(
                contribution.requires_interpreter.as_deref(),
                settings,
                &root,
            ),
            def: analysis_core::FormatterDef::from_contribution(contribution),
            root,
            php_binary: settings_model::php::resolve(settings).interpreter,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.def.name
    }

    pub(crate) fn run(
        &self,
        text: &str,
        path: &Path,
        timeout: Duration,
    ) -> Result<String, analysis_core::FormatError> {
        analysis_core::format(
            &self.def,
            text,
            path,
            &self.root,
            &self.host,
            &self.php_binary,
            timeout,
        )
    }
}
