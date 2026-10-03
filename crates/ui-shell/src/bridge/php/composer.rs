//! `ComposerService` (PHP parity plan, I8): the Composer tool window's
//! model. Translation only: which rows exist and what each action's argv
//! is are `php_core::composer_view`'s; launching is `RunService`'s.

use cxx_qt_lib::QString;

use php_core::composer_view::{self, ComposerAction};

use crate::bridge::convert::current_project_root;
use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};

#[derive(Default)]
pub struct ComposerServiceRust {}

fn ffi_row(kind: &str, name: &str, detail: String, installed: bool) -> ffi::FfiComposerRow {
    ffi::FfiComposerRow {
        kind: QString::from(kind),
        name: QString::from(name),
        detail: QString::from(detail.as_str()),
        installed,
    }
}

impl ffi::ComposerService {
    pub fn has_composer_json(&self) -> bool {
        current_project_root().is_some_and(|root| root.join("composer.json").is_file())
    }

    pub fn affects_rows(&self, path: &QString) -> bool {
        composer_view::is_manifest_path(std::path::Path::new(&path.to_string()))
    }

    pub fn rows(&self) -> Vec<ffi::FfiComposerRow> {
        let Some(root) = current_project_root() else {
            return Vec::new();
        };
        // An unreadable composer.json shows as an empty window; the file
        // itself is where the syntax error is reported.
        let Ok(Some(view)) = composer_view::view(&root) else {
            return Vec::new();
        };
        let scripts = view
            .scripts
            .iter()
            .map(|s| ffi_row("script", &s.name, s.commands.join(" && "), true));
        let packages = view.packages.iter().map(|p| {
            ffi_row(
                if p.dev { "dev-package" } else { "package" },
                &p.name,
                p.installed.clone().unwrap_or_else(|| p.constraint.clone()),
                p.installed.is_some(),
            )
        });
        scripts.chain(packages).collect()
    }

    pub fn action_config(&self, action: &QString, argument: &QString) -> ffi::FfiComposerAction {
        let action = match ComposerAction::parse(&action.to_string(), &argument.to_string()) {
            Ok(action) => action,
            Err(error) => {
                return ffi::FfiComposerAction {
                    result: errors::failure(errors::CODE_INVALID_ARGUMENT, error.to_string()),
                    config: ffi::FfiRunConfig::default(),
                }
            }
        };
        let args = action.args();
        let config = run_core::RunConfig {
            id: format!("composer-{}", args.join("-")),
            name: action.title(),
            program: composer_view::COMPOSER_PROGRAM.to_string(),
            args,
            cwd: Some("$PROJECT_DIR$".to_string()),
            toolchain: Some(run_core::ToolchainId::Php.as_str().to_string()),
            temporary: true,
            ..run_core::RunConfig::default()
        };
        ffi::FfiComposerAction {
            result: FfiResult::default(),
            config: crate::bridge::run::to_ffi_run_config(&config),
        }
    }
}
