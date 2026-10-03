//! New > <file template> (ADR-0072): translation only.
//!
//! The rules — which names are valid, how the body renders, what the PSR-4
//! namespace of a directory is, that an existing file is never overwritten —
//! live in `settings_model::file_templates`, `php_core::psr4` and
//! `app_core`. This file looks up the template, collects the variables and
//! hands the rendered text to the session.

use core::pin::Pin;
use std::path::PathBuf;

use cxx_qt_lib::QString;
use settings_model::file_templates::{self, Vars};

use crate::bridge::errors;
use crate::bridge::ffi;

/// The namespace a new file of `language` gets in `dir`. Only PHP has one.
fn namespace_for(language: &str, project: &std::path::Path, dir: &std::path::Path) -> String {
    match language {
        "php" => php_core::psr4::namespace_for_dir(project, dir),
        _ => String::new(),
    }
}

fn refused(message: String) -> ffi::FfiCreateResult {
    ffi::FfiCreateResult {
        result: errors::failure(errors::CODE_REFUSED, message),
        path: QString::default(),
    }
}

impl ffi::ProjectTreeModel {
    /// The templates the New menu lists: those of a language the project
    /// uses (`settings_model::file_templates::is_offered`).
    pub fn file_templates(&self) -> Vec<ffi::FfiFileTemplate> {
        let root = self.session.borrow().root_path().map(PathBuf::from);
        plugin_host::registry()
            .file_templates()
            .filter(|(_, template)| {
                root.as_deref().is_none_or(|root| {
                    file_templates::is_offered(
                        template,
                        |marker| root.join(marker).is_file(),
                        |extension| project_model::contains_extension(root, extension),
                    )
                })
            })
            .map(|(_, template)| ffi::FfiFileTemplate {
                id: QString::from(template.id.as_str()),
                name: QString::from(template.name.as_str()),
            })
            .collect()
    }

    /// Create a file from template `template_id` named `name` in
    /// `parent_dir`, and say where it landed so the view can open it.
    pub fn create_from_template(
        mut self: Pin<&mut Self>,
        parent_dir: &QString,
        template_id: &QString,
        name: &QString,
    ) -> ffi::FfiCreateResult {
        let registry = plugin_host::registry();
        let id = template_id.to_string();
        let Some((_, template)) = registry.file_templates().find(|(_, t)| t.id == id) else {
            return refused(format!("there is no file template \"{id}\""));
        };
        let entity = match file_templates::entity_name(template, &name.to_string()) {
            Ok(entity) => entity,
            Err(message) => return refused(message),
        };
        let parent = PathBuf::from(parent_dir.to_string());
        let project = self.session.borrow().root_path().map(PathBuf::from);
        let now = chrono::Local::now();
        let vars = Vars {
            namespace: project
                .as_deref()
                .map(|root| namespace_for(&template.language, root, &parent))
                .unwrap_or_default(),
            name: entity.clone(),
            date: now.format("%Y-%m-%d").to_string(),
            year: now.format("%Y").to_string(),
        };
        let content = file_templates::render(template, &vars);
        let file_name = file_templates::file_name(template, &entity);
        let created = self
            .session
            .borrow_mut()
            .create_file_with(&parent, &file_name, &content);
        let path = created
            .as_ref()
            .map(|path| QString::from(path.to_string_lossy().as_ref()))
            .unwrap_or_default();
        let result = self
            .as_mut()
            .finish_mutation(created.map(|_| None), vec![parent]);
        ffi::FfiCreateResult { result, path }
    }
}
