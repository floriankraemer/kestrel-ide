//! `SearchModel` lookups that ask a language server before the index
//! (ADR-0016's precedence). A sibling of `search.rs`, which holds the
//! index-only half.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::convert::to_ffi_symbol_match;
use crate::bridge::ffi;

impl ffi::SearchModel {
    pub fn implementations_at(
        self: Pin<&mut Self>,
        name: &QString,
        path: &QString,
        line: u32,
        character: u32,
    ) {
        let name = name.to_string();
        let uri = lsp_core::uri_from_path(&path.to_string());
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        let job_name = name.clone();
        let queued = crate::bridge::registry::push_lsp_job(Box::new(move |manager| {
            let rows: Vec<_> =
                match lsp_core::usable_targets(manager.implementation(&uri, line, character)) {
                    Some(targets) => targets
                        .into_iter()
                        .map(|t| index_core::SymbolMatch {
                            name: job_name.clone(),
                            kind: None,
                            path: std::path::PathBuf::from(t.path),
                            line: t.line as usize,
                            col: t.column as usize,
                            is_definition: true,
                            container: None,
                        })
                        .collect(),
                    None => slot
                        .read()
                        .unwrap()
                        .ready()
                        .and_then(|index| index.find_implementations(&job_name).ok())
                        .unwrap_or_default(),
                };
            for m in rows {
                let row = to_ffi_symbol_match(m);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().usages_found(row);
                });
            }
            let _ = qt_thread.queue(|mut model: Pin<&mut Self>| {
                model.as_mut().usages_finished();
            });
        }));
        if !queued {
            // No servers running at all: the index alone answers.
            self.find_implementations(&QString::from(name.as_str()));
        }
    }
}
