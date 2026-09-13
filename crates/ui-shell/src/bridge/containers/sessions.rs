//! Files tab and streaming sessions (C3): `listFiles`/`openFile`/
//! `downloadFile`, and the four `*SessionCommand` calls that hand
//! `TerminalSupervisor::setCommand` its argv for Log/Terminal/Exec/Attach.
//! A third `impl ffi::ContainerService` block — see `mod.rs`'s doc comment.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::files::{self, EntryKind};
use container_core::session;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiCommand, FfiFileEntry, FfiResult};

use super::actions::parse_container_node_id;
use super::service;

/// The dialog's history combo keeps at most this many past Exec commands
/// per container, most recent first — generous for "what did I just type"
/// without growing without bound across a long session.
const EXEC_HISTORY_CAP: usize = 10;

fn to_ffi_kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Dir => "dir",
        EntryKind::File => "file",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

fn to_ffi_file_entry(entry: files::FileEntry) -> FfiFileEntry {
    FfiFileEntry {
        name: QString::from(entry.name.as_str()),
        kind: QString::from(to_ffi_kind(entry.kind)),
        size: entry.size,
        mtime_epoch: entry.mtime_epoch.unwrap_or(-1),
        mode: QString::from(entry.mode.as_str()),
        target: QString::from(entry.target.as_str()),
    }
}

fn to_ffi_command(spec: pty_core::ShellSpec) -> FfiCommand {
    FfiCommand {
        program: QString::from(spec.program.as_str()),
        args: QString::from(spec.args.join("\n").as_str()),
        env: QString::from(
            spec.env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("\n")
                .as_str(),
        ),
    }
}

/// Resolve `node_id` to `(connection_id, resource_id, Invocation)`, or an
/// `FfiResult`/empty `FfiCommand` failure — the shared first step of every
/// call in this file.
fn resolve(
    node_id: &QString,
) -> Result<(String, String, container_core::connection::Invocation), FfiResult> {
    let node_id_str = node_id.to_string();
    let Some((connection_id, resource_id)) = parse_container_node_id(&node_id_str) else {
        return Err(errors::failure(
            errors::CODE_INVALID_ARGUMENT,
            format!("'{node_id_str}' is not a container node"),
        ));
    };
    let invocation = service::connection_invocation(&connection_id)?;
    Ok((connection_id, resource_id, invocation))
}

impl ffi::ContainerService {
    pub fn list_files(mut self: Pin<&mut Self>, node_id: &QString, dir: &QString) -> FfiResult {
        let (_, resource_id, invocation) = match resolve(node_id) {
            Ok(resolved) => resolved,
            Err(result) => return result,
        };
        let node_id_str = node_id.to_string();
        let dir_str = if dir.to_string().is_empty() {
            "/".to_string()
        } else {
            dir.to_string()
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        let for_signal = dir_str.clone();
        std::thread::spawn(move || {
            let result = files::list_dir(&invocation, &resource_id, &dir_str, &work_dir);
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ContainerService>| match result {
                    Ok(entries) => {
                        service.as_mut().files_ready(
                            QString::from(node_id_str.as_str()),
                            QString::from(for_signal.as_str()),
                            entries.into_iter().map(to_ffi_file_entry).collect(),
                        );
                    }
                    Err(err) => {
                        service.as_mut().action_finished(
                            QString::from(node_id_str.as_str()),
                            false,
                            QString::from(err.message.as_str()),
                        );
                    }
                },
            );
        });
        FfiResult::default()
    }

    /// Read `path` out of the container and open it as a read-only virtual
    /// document, keyed `"<conn>/container/<id>/fs<path>"` — language
    /// detection runs on the key's last segment (`AppSession::
    /// open_virtual_document`'s own rule), so a `Dockerfile`/`*.py`/... at
    /// the end of `path` highlights for free.
    pub fn open_file(mut self: Pin<&mut Self>, node_id: &QString, path: &QString) -> FfiResult {
        let (connection_id, resource_id, invocation) = match resolve(node_id) {
            Ok(resolved) => resolved,
            Err(result) => return result,
        };
        let path_str = path.to_string();
        let work_dir = service::work_dir();
        let text = match files::read_file(&invocation, &resource_id, &path_str, &work_dir) {
            Ok(text) => text,
            Err(err) => return errors::failure(errors::CODE_REFUSED, err.message),
        };
        let key = format!("{connection_id}/container/{resource_id}/fs{path_str}");
        let opened = self
            .session
            .borrow_mut()
            .open_virtual_document("container", &key, &text);
        self.as_mut().virtual_document_opened(
            opened.id.raw(),
            QString::from(opened.title.as_str()),
            opened.newly_opened,
        );
        FfiResult::default()
    }

    pub fn download_file(
        mut self: Pin<&mut Self>,
        node_id: &QString,
        path: &QString,
        host_dest: &QString,
    ) -> FfiResult {
        let (_, resource_id, invocation) = match resolve(node_id) {
            Ok(resolved) => resolved,
            Err(result) => return result,
        };
        let node_id_str = node_id.to_string();
        let path_str = path.to_string();
        let host_dest_str = host_dest.to_string();
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let args = files::download_args(&resource_id, &path_str, &host_dest_str);
            let result = container_core::ops::run_op(&invocation, &args, &work_dir);
            let (ok, message) = match result {
                Ok(_) => (true, format!("Downloaded to {host_dest_str}")),
                Err(err) => (false, err.message),
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                service.as_mut().action_finished(
                    QString::from(node_id_str.as_str()),
                    ok,
                    QString::from(message.as_str()),
                );
            });
        });
        FfiResult::default()
    }

    pub fn log_session_command(&self, node_id: &QString, tail: u32) -> FfiCommand {
        let Ok((_, resource_id, invocation)) = resolve(node_id) else {
            return FfiCommand::default();
        };
        to_ffi_command(session::logs_session(&invocation, &resource_id, tail))
    }

    pub fn terminal_session_command(&self, node_id: &QString, as_root: bool) -> FfiCommand {
        let Ok((_, resource_id, invocation)) = resolve(node_id) else {
            return FfiCommand::default();
        };
        to_ffi_command(session::terminal_session(
            &invocation,
            &resource_id,
            as_root,
        ))
    }

    pub fn attach_session_command(&self, node_id: &QString) -> FfiCommand {
        let Ok((_, resource_id, invocation)) = resolve(node_id) else {
            return FfiCommand::default();
        };
        to_ffi_command(session::attach_session(&invocation, &resource_id))
    }

    /// The Exec dialog's command, and this container's history entry for
    /// it (capped at [`EXEC_HISTORY_CAP`], most recent first, a repeat of
    /// the same command moved to the front rather than duplicated).
    pub fn exec_session_command(
        self: Pin<&mut Self>,
        node_id: &QString,
        command: &QString,
    ) -> FfiCommand {
        let Ok((_, resource_id, invocation)) = resolve(node_id) else {
            return FfiCommand::default();
        };
        let command_str = command.to_string();
        let tokens: Vec<String> = command_str.split_whitespace().map(str::to_string).collect();
        if tokens.is_empty() {
            return FfiCommand::default();
        }
        {
            let mut history = self.exec_history.borrow_mut();
            let entry = history.entry(resource_id.clone()).or_default();
            entry.retain(|previous| previous != &command_str);
            entry.insert(0, command_str.clone());
            entry.truncate(EXEC_HISTORY_CAP);
        }
        to_ffi_command(session::exec_session(&invocation, &resource_id, &tokens))
    }

    pub fn exec_history(&self, node_id: &QString) -> QString {
        let Some((_, resource_id)) = parse_container_node_id(&node_id.to_string()) else {
            return QString::from("");
        };
        let history = self.exec_history.borrow();
        let joined = history
            .get(&resource_id)
            .map(|entries| entries.join("\n"))
            .unwrap_or_default();
        QString::from(joined.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_kind_words() {
        assert_eq!(to_ffi_kind(EntryKind::Dir), "dir");
        assert_eq!(to_ffi_kind(EntryKind::File), "file");
        assert_eq!(to_ffi_kind(EntryKind::Symlink), "symlink");
        assert_eq!(to_ffi_kind(EntryKind::Other), "other");
    }

    #[test]
    fn command_conversion_joins_args_and_env() {
        let spec = pty_core::ShellSpec::new("docker", vec!["logs".into(), "-f".into()])
            .with_env(vec![("A".to_string(), "1".to_string())]);
        let command = to_ffi_command(spec);
        assert_eq!(command.program.to_string(), "docker");
        assert_eq!(command.args.to_string(), "logs\n-f");
        assert_eq!(command.env.to_string(), "A=1");
    }
}
