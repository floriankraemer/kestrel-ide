//! Registries (C7): lazy repository/tag loading for the tree's `Registry`
//! nodes, and the pull/push commands that run a login (when a secret is
//! stored) plus the actual `pull`/`tag`+`push` as one PTY session. A fifth
//! `impl ffi::ContainerService` block — see `mod.rs`'s doc comment for why
//! each task's surface gets its own file under the size ratchet.
//!
//! Repository/tag fetches run on a worker thread through
//! `container_registry::registry::RegistryClient`, the same "never block
//! the Qt thread on a network call" shape `test_registry_connection`
//! already uses on `AppSettings` — the client itself is Qt-free, this file
//! only owns the thread and the cache the fetched pages land in
//! (`ContainerServiceRust::registry_repos`/`registry_tags`, `service.rs`).

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::registry_ref::RegistryKind;
use container_core::session;
use container_core::tree::NodeKind;
use container_registry::registry::RegistryClient;
use container_registry::secrets::SecretStore;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiCommand, FfiResult};

use super::actions::parse_node_id;
use super::service;

/// How many repositories/tags one page fetches — generous for a tree
/// popup, not meant to page through a registry with thousands of
/// repositories one click at a time (a search/filter box is a later
/// task's, the same gap the plan's Registries mockup leaves for C7).
const PAGE_SIZE: u32 = 100;

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

fn find_registry(registry_id: &str) -> Option<app_config::RegistrySetting> {
    service::configured_registries()
        .into_iter()
        .find(|setting| setting.id == registry_id)
}

/// Pairs each tag with its fully-qualified reference — the shape
/// `load_registry_tags`/`load_more_registry_tags` both cache and
/// `tree::registry_tag_nodes` both take, pulled out so the two fetches
/// (first page, next page) share one place that knows how a reference is
/// built rather than repeating the `map` inline.
fn with_references(
    kind: RegistryKind,
    address: &str,
    repository: &str,
    tags: Vec<String>,
) -> Vec<(String, String)> {
    tags.into_iter()
        .map(|tag| {
            let reference =
                container_core::registry_ref::format_reference(kind, address, repository, &tag);
            (tag, reference)
        })
        .collect()
}

fn client_for(
    setting: &app_config::RegistrySetting,
    secret: &str,
) -> Result<RegistryClient, String> {
    RegistryClient::new(
        &setting.address,
        RegistryKind::from_id(&setting.kind),
        &setting.username,
        secret,
        &setting.gitlab_project,
    )
    .map_err(|error| error.to_string())
}

/// `(address, username)` when a secret is stored for `registry_id` — the
/// login-credential pair `container_core::session`'s registry sessions
/// take, `None` when there is nothing to log in with (anonymous pull, or
/// push through the CLI's own credential store).
fn login_credential(
    setting: &app_config::RegistrySetting,
) -> (Option<(String, String)>, Option<String>) {
    match SecretStore::load(&setting.id) {
        Ok(Some(secret)) => (
            Some((setting.address.clone(), setting.username.clone())),
            Some(secret),
        ),
        _ => (None, None),
    }
}

impl ffi::ContainerService {
    pub fn load_registry_repositories(
        mut self: Pin<&mut Self>,
        registry_id: &QString,
    ) -> FfiResult {
        let registry_id = registry_id.to_string();
        let Some(setting) = find_registry(&registry_id) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no registry with id '{registry_id}' is configured"),
            );
        };
        let secret = SecretStore::load(&registry_id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let (repositories, next) = client_for(&setting, &secret)
                .and_then(|client| {
                    client
                        .catalog(PAGE_SIZE, None)
                        .map_err(|error| error.to_string())
                })
                .map(|page| (page.items, page.next))
                .unwrap_or_default();
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                service
                    .registry_repos
                    .borrow_mut()
                    .insert(registry_id.clone(), repositories);
                service
                    .registry_repo_next
                    .borrow_mut()
                    .insert(registry_id.clone(), next);
                service.as_mut().registry_children_ready(QString::from(
                    format!("registry/{registry_id}").as_str(),
                ));
            });
        });
        FfiResult::default()
    }

    /// C7 review follow-up: continues `loadRegistryRepositories`/
    /// `loadRegistryTags` past their first page. `more_node_id` is a
    /// [`tree::registry_more_node`]'s own id — its parent (the id with the
    /// trailing `/more` stripped) is either a `Registry` node
    /// (`"registry/<id>"`) or a `RegistryRepo` node
    /// (`"registry/<id>/repo/<repository>"`), which is all this needs to
    /// tell repositories-paging from tags-paging apart, the same
    /// `"/repo/"`-presence test `loadRegistryTags` already uses.
    pub fn load_more_registry_children(self: Pin<&mut Self>, more_node_id: &QString) -> FfiResult {
        let more_node_id = more_node_id.to_string();
        let Some(parent_id) = more_node_id.strip_suffix("/more") else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                "not a registry \"load more\" node id",
            );
        };
        let Some(rest) = parent_id.strip_prefix("registry/") else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                "not a registry \"load more\" node id",
            );
        };
        match rest.split_once("/repo/") {
            Some((registry_id, repository)) => {
                self.load_more_registry_tags(registry_id, repository, parent_id)
            }
            None => self.load_more_registry_repositories(rest),
        }
    }

    fn load_more_registry_repositories(mut self: Pin<&mut Self>, registry_id: &str) -> FfiResult {
        let registry_id = registry_id.to_string();
        let Some(cursor) = self
            .registry_repo_next
            .borrow()
            .get(&registry_id)
            .cloned()
            .flatten()
        else {
            return errors::failure(errors::CODE_REFUSED, "no further page for this registry");
        };
        let Some(setting) = find_registry(&registry_id) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no registry with id '{registry_id}' is configured"),
            );
        };
        let secret = SecretStore::load(&registry_id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let (more, next) = client_for(&setting, &secret)
                .and_then(|client| {
                    client
                        .catalog(PAGE_SIZE, Some(&cursor))
                        .map_err(|error| error.to_string())
                })
                .map(|page| (page.items, page.next))
                .unwrap_or_default();
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                service
                    .registry_repos
                    .borrow_mut()
                    .entry(registry_id.clone())
                    .or_default()
                    .extend(more);
                service
                    .registry_repo_next
                    .borrow_mut()
                    .insert(registry_id.clone(), next);
                service.as_mut().registry_children_ready(QString::from(
                    format!("registry/{registry_id}").as_str(),
                ));
            });
        });
        FfiResult::default()
    }

    pub fn load_registry_tags(mut self: Pin<&mut Self>, repo_node_id: &QString) -> FfiResult {
        let repo_node_id_str = repo_node_id.to_string();
        // `"registry/<id>/repo/<repository...>"` — the repository itself
        // may contain `/`, so only the fixed prefix is split off.
        let Some(rest) = repo_node_id_str.strip_prefix("registry/") else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "not a registry-repo node id");
        };
        let Some((registry_id, repository)) = rest.split_once("/repo/") else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "not a registry-repo node id");
        };
        let Some(setting) = find_registry(registry_id) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no registry with id '{registry_id}' is configured"),
            );
        };
        let repository = repository.to_string();
        let kind = RegistryKind::from_id(&setting.kind);
        let address = setting.address.clone();
        let secret = SecretStore::load(&setting.id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let qt_thread = self.as_mut().qt_thread();
        let repo_node_id_for_signal = repo_node_id_str.clone();
        std::thread::spawn(move || {
            let (tags, next) = client_for(&setting, &secret)
                .and_then(|client| {
                    client
                        .tags(&repository, PAGE_SIZE, None)
                        .map_err(|error| error.to_string())
                })
                .map(|page| (page.items, page.next))
                .unwrap_or_default();
            let tagged = with_references(kind, &address, &repository, tags);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                service
                    .registry_tags
                    .borrow_mut()
                    .insert(repo_node_id_for_signal.clone(), tagged);
                service
                    .registry_tag_next
                    .borrow_mut()
                    .insert(repo_node_id_for_signal.clone(), next);
                service
                    .as_mut()
                    .registry_children_ready(QString::from(repo_node_id_for_signal.as_str()));
            });
        });
        FfiResult::default()
    }

    fn load_more_registry_tags(
        mut self: Pin<&mut Self>,
        registry_id: &str,
        repository: &str,
        repo_node_id: &str,
    ) -> FfiResult {
        let repo_node_id = repo_node_id.to_string();
        let Some(cursor) = self
            .registry_tag_next
            .borrow()
            .get(&repo_node_id)
            .cloned()
            .flatten()
        else {
            return errors::failure(errors::CODE_REFUSED, "no further page for this repository");
        };
        let Some(setting) = find_registry(registry_id) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no registry with id '{registry_id}' is configured"),
            );
        };
        let repository = repository.to_string();
        let kind = RegistryKind::from_id(&setting.kind);
        let address = setting.address.clone();
        let secret = SecretStore::load(&setting.id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let (tags, next) = client_for(&setting, &secret)
                .and_then(|client| {
                    client
                        .tags(&repository, PAGE_SIZE, Some(&cursor))
                        .map_err(|error| error.to_string())
                })
                .map(|page| (page.items, page.next))
                .unwrap_or_default();
            let more = with_references(kind, &address, &repository, tags);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                service
                    .registry_tags
                    .borrow_mut()
                    .entry(repo_node_id.clone())
                    .or_default()
                    .extend(more);
                service
                    .registry_tag_next
                    .borrow_mut()
                    .insert(repo_node_id.clone(), next);
                service
                    .as_mut()
                    .registry_children_ready(QString::from(repo_node_id.as_str()));
            });
        });
        FfiResult::default()
    }

    pub fn pull_from_registry_command(
        &self,
        tag_node_id: &QString,
        connection_id: &QString,
    ) -> FfiCommand {
        let tag_node_id = tag_node_id.to_string();
        // `"registry/<id>/repo/<repository>/tag/<tag>"`.
        let Some(rest) = tag_node_id.strip_prefix("registry/") else {
            return FfiCommand::default();
        };
        let Some((registry_id, _)) = rest.split_once("/repo/") else {
            return FfiCommand::default();
        };
        let Some(setting) = find_registry(registry_id) else {
            return FfiCommand::default();
        };
        let reference = self
            .registry_tags
            .borrow()
            .iter()
            .find_map(|(repo_node_id, tags)| {
                tags.iter()
                    .find(|(tag, _)| format!("{repo_node_id}/tag/{tag}") == tag_node_id)
                    .map(|(_, reference)| reference.clone())
            });
        let Some(reference) = reference else {
            return FfiCommand::default();
        };
        let Ok(invocation) = service::connection_invocation(&connection_id.to_string()) else {
            return FfiCommand::default();
        };
        let (credential, secret) = login_credential(&setting);
        let credential_refs = credential
            .as_ref()
            .map(|(address, username)| (address.as_str(), username.as_str()));
        to_ffi_command(session::pull_from_registry_session(
            &invocation,
            &reference,
            credential_refs,
            secret.as_deref(),
        ))
    }

    pub fn push_image_command(
        &self,
        image_node_id: &QString,
        registry_id: &QString,
        repository: &QString,
        tag: &QString,
    ) -> FfiCommand {
        let Some((connection_id, image_id)) =
            parse_node_id(&image_node_id.to_string(), NodeKind::Image)
        else {
            return FfiCommand::default();
        };
        let Some(setting) = find_registry(&registry_id.to_string()) else {
            return FfiCommand::default();
        };
        let Ok(invocation) = service::connection_invocation(&connection_id) else {
            return FfiCommand::default();
        };
        let kind = RegistryKind::from_id(&setting.kind);
        let destination = container_core::registry_ref::format_reference(
            kind,
            &setting.address,
            &repository.to_string(),
            &tag.to_string(),
        );
        let (credential, secret) = login_credential(&setting);
        let credential_refs = credential
            .as_ref()
            .map(|(address, username)| (address.as_str(), username.as_str()));
        to_ffi_command(session::push_to_registry_session(
            &invocation,
            &image_id,
            &destination,
            credential_refs,
            secret.as_deref(),
        ))
    }
}
