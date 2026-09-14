//! `ContainerService`'s editor-facing surface (C6): compose code lenses
//! and the "Pull image" intention's landing. An eighth `impl
//! ffi::ContainerService` block — see `mod.rs`'s doc comment for the
//! split.
//!
//! Translation only: what a lens says is `container_core::lenses`'s,
//! which connection a pull goes to is one rule below (`pull_connection`),
//! and what a click does is a signal the view wires to the Containers
//! dock (`containerLogRequested`) or the desktop (`openUrlRequested`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;

use cxx_qt_lib::QString;

use container_core::lenses::{self, ComposeLens, LensKind};
use container_core::tree::ConnectionState;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiComposeLens, FfiResult};

/// The lenses last computed per open compose file, so a click by index
/// resolves against exactly what the editor is showing.
#[derive(Default)]
pub(crate) struct LensState(RefCell<BTreeMap<String, Vec<ComposeLens>>>);

fn to_ffi_lens(lens: &ComposeLens) -> FfiComposeLens {
    match &lens.kind {
        LensKind::Status {
            running,
            total,
            exit_code,
        } => FfiComposeLens {
            line: lens.line,
            is_open_url: false,
            running: *running as i64,
            total: *total as i64,
            exit_code: exit_code.unwrap_or(-1),
            host_port: QString::default(),
            clickable: !lens.container_node_id.is_empty(),
        },
        LensKind::OpenUrl { host_port } => FfiComposeLens {
            line: lens.line,
            is_open_url: true,
            running: -1,
            total: -1,
            exit_code: -1,
            host_port: QString::from(host_port.as_str()),
            clickable: true,
        },
    }
}

impl ffi::ContainerService {
    /// Whether `path`'s code lenses are this service's rather than the
    /// language server's — the compose file-name rule, answered here so
    /// the view's click routing never guesses.
    pub fn owns_lenses(&self, path: &QString) -> bool {
        container_core::compose_file::is_compose_file(Path::new(&path.to_string()))
    }

    /// The lenses for compose file `path` with live contents `text`, from
    /// every connected snapshot; empty for any other file. Remembered for
    /// `runLens`.
    pub fn compose_lenses(&self, path: &QString, text: &QString) -> Vec<FfiComposeLens> {
        if !self.owns_lenses(path) {
            return Vec::new();
        }
        let path = path.to_string();
        let connections = self.connections.borrow();
        let snapshots: Vec<(&str, &container_core::snapshot::EngineSnapshot)> = connections
            .iter()
            .filter_map(|(id, connection)| {
                connection
                    .snapshot
                    .as_ref()
                    .map(|snapshot| (id.as_str(), snapshot))
            })
            .collect();
        let computed = lenses::compose_lenses(Path::new(&path), &text.to_string(), &snapshots);
        let ffi = computed.iter().map(to_ffi_lens).collect();
        self.lenses.0.borrow_mut().insert(path, computed);
        ffi
    }

    /// A click on lens `index` of `path`: a status lens selects its
    /// container in the Containers dock (`containerLogRequested`), an
    /// "Open localhost:<port>" lens asks the view to open the URL.
    pub fn run_lens(mut self: Pin<&mut Self>, path: &QString, index: u32) {
        let lens = self
            .lenses
            .0
            .borrow()
            .get(&path.to_string())
            .and_then(|lenses| lenses.get(index as usize))
            .cloned();
        match lens {
            Some(ComposeLens {
                kind: LensKind::OpenUrl { host_port },
                ..
            }) => {
                let url = format!("http://localhost:{host_port}");
                self.as_mut()
                    .open_url_requested(QString::from(url.as_str()));
            }
            Some(lens) if !lens.container_node_id.is_empty() => {
                self.as_mut()
                    .container_log_requested(QString::from(lens.container_node_id.as_str()));
            }
            _ => {}
        }
    }

    /// Every connected engine's image names, `\n`-joined — the local half
    /// of the editor's image-name completion, which the view forwards to
    /// `LanguageService::setLocalImages` on each `treeChanged`.
    pub fn local_image_names(&self) -> QString {
        let connections = self.connections.borrow();
        let mut names: Vec<String> = connections
            .values()
            .filter_map(|connection| connection.snapshot.as_ref())
            .flat_map(|snapshot| snapshot.images.iter())
            .flat_map(|image| image.repo_tags.iter().cloned())
            .collect();
        names.sort();
        names.dedup();
        QString::from(names.join("\n").as_str())
    }

    /// The editor's "Pull image <reference>" intention: picks the
    /// connection ([`pull_connection`]) and hands `pullRequested` to the
    /// view, which opens the same Pull tab the Images console uses. The
    /// refusal when no connection exists is the returned `FfiResult`.
    pub fn pull_image(mut self: Pin<&mut Self>, reference: &QString) -> FfiResult {
        let connection_id = {
            let connections = self.connections.borrow();
            let connected: Vec<&String> = connections
                .iter()
                .filter(|(_, c)| matches!(c.state, ConnectionState::Connected(_)))
                .map(|(id, _)| id)
                .collect();
            pull_connection(
                &connected,
                &super::service::configured_connections()
                    .iter()
                    .map(|setting| setting.id.as_str())
                    .collect::<Vec<_>>(),
            )
        };
        let Some(connection_id) = connection_id else {
            return errors::failure(errors::CODE_REFUSED, "No container connection configured");
        };
        self.as_mut()
            .pull_requested(QString::from(connection_id.as_str()), reference.clone());
        FfiResult::default()
    }
}

/// The connection a pull from the editor goes to: the first connected
/// one, else the only configured one — never a guess among several
/// disconnected ones.
fn pull_connection(connected: &[&String], configured: &[&str]) -> Option<String> {
    connected
        .first()
        .map(|id| (*id).clone())
        .or_else(|| match configured {
            [only] => Some((*only).to_string()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::pull_connection;

    #[test]
    fn first_connected_wins_then_the_only_configured_then_nothing() {
        let a = "a".to_string();
        let b = "b".to_string();
        assert_eq!(
            pull_connection(&[&b, &a], &["a", "b"]).as_deref(),
            Some("b")
        );
        assert_eq!(pull_connection(&[], &["a"]).as_deref(), Some("a"));
        assert_eq!(pull_connection(&[], &["a", "b"]), None);
        assert_eq!(pull_connection(&[], &[]), None);
    }
}
