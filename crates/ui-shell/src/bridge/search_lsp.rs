//! `SearchModel` lookups that ask a language server before the index
//! (ADR-0016's precedence). A sibling of `search.rs`, which holds the
//! index-only half.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::convert::to_ffi_symbol_match;
use crate::bridge::ffi;
use crate::bridge::registry::push_lsp_job;
use crate::bridge::search::{hit, symbol_detail};

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

/// Ask every running server for `workspace/symbol` on the LSP worker, and get
/// the answer on a channel so the caller can run its index query meanwhile.
/// `None` when no servers are running at all.
pub(super) fn request_server_symbols(
    query: &str,
) -> Option<std::sync::mpsc::Receiver<Vec<lsp_core::WorkspaceSymbol>>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let query = query.to_string();
    push_lsp_job(Box::new(move |manager| {
        let _ = sender.send(manager.workspace_symbols(&query).unwrap_or_default());
    }))
    .then_some(receiver)
}

/// The server's answer, or nothing once it has had its time.
pub(super) fn server_symbols(
    receiver: std::sync::mpsc::Receiver<Vec<lsp_core::WorkspaceSymbol>>,
) -> Vec<lsp_core::WorkspaceSymbol> {
    receiver
        .recv_timeout(
            lsp_core::symbols::WORKSPACE_SYMBOL_TIMEOUT + std::time::Duration::from_millis(500),
        )
        .unwrap_or_default()
}

/// The symbol tier of Search Everywhere (Go to Symbol), or with
/// `classes_only` Go to Class: the index's ranked definitions, then whatever
/// the running servers' `workspace/symbol` adds that the index did not list.
pub(super) fn emit_symbol_tier(
    index: &index_core::TextIndex,
    query: &str,
    limit: usize,
    classes_only: bool,
    emit: &dyn Fn(Vec<ffi::FfiSearchHit>),
) {
    // ponytail: symbol rows carry no highlight positions —
    // `find_definitions_ranked` scores without reporting match indices.
    // Thread them through if the visual inconsistency with the file tier
    // starts to show.
    //
    // The server is asked first so it works while the index is queried.
    let from_server = request_server_symbols(query);
    let ranked_limit = if classes_only { usize::MAX } else { limit };
    let indexed: Vec<_> = index
        .find_definitions_ranked(query, ranked_limit)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            !classes_only
                || m.kind
                    .is_some_and(|k| k.category() == syntax_core::SymbolCategory::NestedTypes)
        })
        .take(limit)
        .collect();
    let root = index.root().to_path_buf();
    let known: Vec<(String, String, u32)> = indexed
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                m.path.to_string_lossy().into_owned(),
                m.line as u32,
            )
        })
        .collect();
    emit(
        indexed
            .into_iter()
            .map(|m| {
                let detail = with_location(&symbol_detail(&m), &m.path, &root);
                let mut row = hit(ffi::FfiHitKind::Symbol, &m.name, &detail, Vec::new());
                row.path = QString::from(m.path.to_string_lossy().as_ref());
                row.line = m.line as u32;
                row
            })
            .collect(),
    );
    let Some(receiver) = from_server else {
        return;
    };
    let fresh = lsp_core::beyond_index(
        known.iter().map(|(n, p, l)| (n.as_str(), p.as_str(), *l)),
        server_symbols(receiver),
    );
    emit(
        fresh
            .into_iter()
            .filter(|s| !classes_only || s.is_class_like())
            .take(limit)
            .map(|s| {
                let detail = match &s.container {
                    Some(container) => format!("{} in {container}", s.kind_word()),
                    None => s.kind_word().to_string(),
                };
                let detail = with_location(&detail, std::path::Path::new(&s.path), &root);
                let mut row = hit(ffi::FfiHitKind::Symbol, &s.name, &detail, Vec::new());
                row.path = QString::from(s.path.as_str());
                row.line = s.line;
                row
            })
            .collect(),
    );
}

/// `detail` followed by the file the symbol lives in, relative to the
/// project, so two classes with one name (the project's and a vendored
/// copy's) can be told apart in the list.
fn with_location(detail: &str, path: &std::path::Path, root: &std::path::Path) -> String {
    let shown = path.strip_prefix(root).unwrap_or(path);
    format!("{detail}  {}", shown.display())
}
