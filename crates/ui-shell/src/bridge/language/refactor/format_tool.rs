//! Reformat Code through a `formatters` contribution (ADR-0070): php-cs-fixer,
//! Pint or phpcbf instead of the language server.
//!
//! Which one runs is `settings_model::formatting::plan`'s rule; this file
//! resolves the program's host, runs it off the Qt thread and publishes the
//! result as minimal line edits through the same pending-edit path an LSP
//! reformat uses, so Ctrl+Z undoes the whole reformat in one step.

use core::pin::Pin;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cxx_qt::Threading;

use crate::bridge::ffi;

/// A formatter tool run that has not finished within this is abandoned.
pub(crate) const FORMAT_TIMEOUT: Duration = Duration::from_secs(30);

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
        Some(Self {
            host: crate::bridge::php::tool_host(
                contribution.requires_interpreter.as_deref(),
                &settings,
                &root,
            ),
            def: analysis_core::FormatterDef::from_contribution(contribution),
            root,
            php_binary: php.interpreter,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.def.name
    }

    pub(crate) fn run(
        &self,
        text: &str,
        path: &Path,
    ) -> Result<String, analysis_core::FormatError> {
        analysis_core::format(
            &self.def,
            text,
            path,
            &self.root,
            &self.host,
            &self.php_binary,
            FORMAT_TIMEOUT,
        )
    }
}

impl ffi::LanguageService {
    /// Reformat the whole file `path` with `tool`. A tool that turns out not
    /// to be installed falls back to the language server, silently: the
    /// setting names a formatter the machine may not have.
    pub(crate) fn format_with_tool(
        mut self: Pin<&mut Self>,
        tool: ToolFormat,
        path: String,
        buffer_revision: i64,
    ) {
        let Some(text) = self.session.borrow().content_for_path(Path::new(&path)) else {
            return;
        };
        self.edits.borrow_mut().begin(buffer_revision);
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = tool.run(&text, Path::new(&path));
            let _ = qt_thread.queue(move |service: Pin<&mut Self>| match result {
                Err(analysis_core::FormatError::NotInstalled) => {
                    service.format_whole_with_lsp(&path, buffer_revision)
                }
                Err(error) => service
                    .finish_refactor(Err(format!("{} could not format: {error}", tool.name()))),
                Ok(formatted) => {
                    let edits = lsp_core::edits_between(&text, &formatted);
                    if edits.is_empty() {
                        return service.finish_refactor(Ok(()));
                    }
                    let plan = lsp_core::EditPlan {
                        buffers: vec![lsp_core::DocumentEdits {
                            uri: lsp_core::uri_from_path(&path),
                            path,
                            version: None,
                            edits,
                        }],
                        files: Vec::new(),
                        ops: Vec::new(),
                        touches_other_files: false,
                    };
                    service.publish_refactor("Reformat Code".to_string(), plan, None);
                }
            });
        });
    }
}
