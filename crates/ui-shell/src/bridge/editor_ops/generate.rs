//! Alt+Insert > Constructor, Getters, Setters (ADR-0072): translation only.
//!
//! What can be generated for the class around the caret, and the code for
//! it, are `php_core::generate`'s; this file reads the caret and turns the
//! insertion into the edits the view applies.

use cxx_qt_lib::QString;
use editor_core::transaction::{TextEdit, Transaction};
use php_core::generate::{self, Kind};

use super::{language_of, EditorOpsRust};
use crate::bridge::ffi;

const KINDS: [(ffi::FfiGenerateKind, Kind, &str); 4] = [
    (
        ffi::FfiGenerateKind::Constructor,
        Kind::Constructor,
        "Constructor",
    ),
    (ffi::FfiGenerateKind::Getters, Kind::Getters, "Getters"),
    (ffi::FfiGenerateKind::Setters, Kind::Setters, "Setters"),
    (
        ffi::FfiGenerateKind::GettersAndSetters,
        Kind::GettersAndSetters,
        "Getters and Setters",
    ),
];

fn domain_kind(kind: ffi::FfiGenerateKind) -> Kind {
    KINDS
        .iter()
        .find(|(ffi_kind, _, _)| *ffi_kind == kind)
        .map_or(Kind::Constructor, |(_, domain, _)| *domain)
}

impl EditorOpsRust {
    /// The PHP class at the primary caret, when the tab is a PHP file.
    fn php_class_at_caret(&self, tab_id: u64, text: &str) -> Option<generate::Class> {
        let language = language_of(&self.session.borrow(), tab_id);
        if language.id() != "php" {
            return None;
        }
        generate::class_at(text, self.selection_of(tab_id).primary().head)
    }
}

impl ffi::EditorOps {
    /// The generators Alt+Insert lists for the class at the caret: empty
    /// outside a PHP class, otherwise every kind, greyed with its reason
    /// when it has nothing to do.
    pub fn generate_options(&self, tab_id: u64, text: &QString) -> Vec<ffi::FfiGenerateOption> {
        let Some(class) = self.php_class_at_caret(tab_id, &text.to_string()) else {
            return Vec::new();
        };
        KINDS
            .iter()
            .map(|(ffi_kind, kind, title)| {
                let availability = class.members(*kind);
                ffi::FfiGenerateOption {
                    kind: *ffi_kind,
                    title: QString::from(*title),
                    enabled: availability.is_ok(),
                    reason: QString::from(availability.err().unwrap_or_default().as_str()),
                }
            })
            .collect()
    }

    /// The properties the picker offers for `kind`.
    pub fn generate_members(
        &self,
        tab_id: u64,
        text: &QString,
        kind: ffi::FfiGenerateKind,
    ) -> Vec<ffi::FfiGenerateMember> {
        let Some(class) = self.php_class_at_caret(tab_id, &text.to_string()) else {
            return Vec::new();
        };
        class
            .members(domain_kind(kind))
            .unwrap_or_default()
            .into_iter()
            .map(|member| ffi::FfiGenerateMember {
                name: QString::from(member.name.as_str()),
                label: QString::from(member.label.as_str()),
            })
            .collect()
    }

    /// The edits that generate `kind` for the picked properties.
    pub fn generate_code(
        &self,
        tab_id: u64,
        text: &QString,
        kind: ffi::FfiGenerateKind,
        selected: Vec<ffi::FfiGenerateMember>,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let Some(class) = self.php_class_at_caret(tab_id, &text) else {
            return Vec::new();
        };
        let language = language_of(&self.session.borrow(), tab_id);
        let unit = self.indent_style(language).unit();
        let selected: Vec<String> = selected.iter().map(|m| m.name.to_string()).collect();
        match class.generate(domain_kind(kind), &selected, &unit) {
            Ok(insertion) => self.commit(
                tab_id,
                &text,
                Transaction::new(vec![TextEdit::insert(insertion.offset, insertion.text)]),
            ),
            Err(_) => Vec::new(),
        }
    }
}
