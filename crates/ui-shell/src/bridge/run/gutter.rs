//! The Dockerfile/compose gutter's half of `RunService` (C5/C6, ADR-0056):
//! which files get a marker, on which lines, and what each marker's popup
//! runs. A second `impl ffi::RunService` block, split out of `mod.rs` for
//! the file-size ceiling the same way `bridge/language/lsp_surface.rs` is.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use cxx_qt_lib::QString;

use crate::bridge::ffi;

use super::{current_project_root, effective_container_settings, no_project, unknown_run_config};

impl ffi::RunService {
    /// Whether `path`'s gutter should show the Dockerfile/Containerfile
    /// popup (C5, ADR-0056) — `syntax_core`'s own `dockerfile` language
    /// entry already covers `Dockerfile`, `Containerfile`, `*.dockerfile`,
    /// `*.containerfile` and `Dockerfile.<stage>` (ADR-0018: one detection
    /// table), so this reuses it rather than a second file-name rule.
    pub fn can_run_containerfile(&self, path: &QString) -> bool {
        syntax_core::language_for_path(Path::new(&path.to_string())).id() == "dockerfile"
    }

    /// Whether `path`'s gutter popup should say "Containerfile" rather than
    /// "Dockerfile" (C9) — Podman's naming convention;
    /// `run_core::detect::is_named_containerfile` is the one rule.
    pub fn is_named_containerfile(&self, path: &QString) -> bool {
        run_core::detect::is_named_containerfile(Path::new(&path.to_string()))
    }

    /// Whether `path`'s gutter should show the compose popup — a file-name
    /// rule (compose is a YAML *flavor*, not a language, ADR-0018), the
    /// same one [`crate::bridge::run::detect_compose`] would suggest it
    /// from.
    pub fn can_run_compose_file(&self, path: &QString) -> bool {
        run_core::is_compose_file_name(Path::new(&path.to_string()))
    }

    /// The Dockerfile gutter's "Build image": `docker build` only, as a
    /// tracked console — no run configuration created or remembered, since
    /// this is a one-off build, not something to reappear in the run
    /// toolbar's picker.
    pub fn build_containerfile(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let Some(mut config) = run_core::containerfile_config(&root, &file) else {
            return unknown_run_config("this file is outside the project");
        };
        if let Some(setting) = config.containerfile.as_mut() {
            setting.run_built_image = false; // build only — see `to_launch_spec_in`'s dispatch.
        }
        let containers = effective_container_settings();
        let context = run_core::MacroContext::for_file(&root, &file).with_containers(containers);
        let spec = {
            use run_core::RunConfigExt as _;
            config.to_launch_spec_in(&context)
        };
        self.as_mut()
            .spawn_ad_hoc_console(format!("{}-build", config.id), spec);
        ffi::FfiResult::default()
    }

    /// The Dockerfile gutter's "Run container": build then run, remembered
    /// as a temporary configuration (so it also appears in the run
    /// toolbar's picker and can be rerun), same as [`Self::run_context`].
    pub fn run_containerfile(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::containerfile_config(&root, &file) else {
            return unknown_run_config("this file is outside the project");
        };
        self.as_mut().remember_and_launch(config, &root, &file)
    }

    /// The Dockerfile gutter's "New configuration...": remembers the
    /// temporary configuration without launching it, returning its id so
    /// the caller (`containers_panel.cpp`/the gutter popup) can open the
    /// run-config dialog already pointed at it.
    pub fn new_containerfile_configuration(mut self: Pin<&mut Self>, path: &QString) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::containerfile_config(&root, &file) else {
            return QString::default();
        };
        self.as_mut().remember_only(config)
    }

    /// The compose file gutter's "Run": the whole file, every service,
    /// remembered as a temporary configuration and launched.
    pub fn run_compose_file(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::compose_config(&root, &first_connection_id(), &file, None)
        else {
            return unknown_run_config("this file is outside the project");
        };
        self.as_mut().remember_and_launch(config, &root, &file)
    }

    /// A service line's marker (C6): `compose up` for that one service,
    /// remembered as its own temporary configuration.
    pub fn run_compose_service(
        mut self: Pin<&mut Self>,
        path: &QString,
        service: &QString,
    ) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let service = service.to_string();
        let Some(config) =
            run_core::compose_config(&root, &first_connection_id(), &file, Some(&service))
        else {
            return unknown_run_config("this file is outside the project");
        };
        self.as_mut().remember_and_launch(config, &root, &file)
    }

    /// The service line's "New configuration..." (C6).
    pub fn new_compose_service_configuration(
        mut self: Pin<&mut Self>,
        path: &QString,
        service: &QString,
    ) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let file = PathBuf::from(path.to_string());
        let service = service.to_string();
        let Some(config) =
            run_core::compose_config(&root, &first_connection_id(), &file, Some(&service))
        else {
            return QString::default();
        };
        self.as_mut().remember_only(config)
    }

    /// The gutter lines that carry a run marker (C6): every service of a
    /// compose file plus its `services:` key, or line 0 for any other
    /// runnable file, or none. `text` is the live buffer — the marker must
    /// follow an unsaved edit that adds a service.
    pub fn run_lines(&self, path: &QString, text: &QString) -> Vec<u32> {
        if self.can_run_compose_file(path) {
            return run_core::compose_run_lines(&text.to_string());
        }
        if self.can_run_file(path) || self.can_run_containerfile(path) {
            return vec![0];
        }
        Vec::new()
    }

    /// The compose service declared at `line`, or empty on the `services:`
    /// line (which runs the whole project) — what the marker's popup is
    /// scoped to (C6).
    pub fn compose_service_at(&self, path: &QString, text: &QString, line: u32) -> QString {
        if !self.can_run_compose_file(path) {
            return QString::default();
        }
        run_core::compose_service_at(&text.to_string(), line)
            .map(|service| QString::from(service.as_str()))
            .unwrap_or_default()
    }

    /// The compose file gutter's "New configuration...".
    pub fn new_compose_file_configuration(mut self: Pin<&mut Self>, path: &QString) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::compose_config(&root, &first_connection_id(), &file, None)
        else {
            return QString::default();
        };
        self.as_mut().remember_only(config)
    }
}

/// The connection a gutter-created compose configuration runs on: the
/// first configured one (empty when none is), the same default
/// `run_compose_file` above has used since C5.
fn first_connection_id() -> String {
    effective_container_settings()
        .connections
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_default()
}
