//! Settings > Containers (C1, ADR-0055): translation only, onto
//! `container_core::{connection, discovery, probe}`. `AppSettings` grows
//! its accessors here rather than in `bridge/settings.rs` — the same
//! per-feature-module split `tab_padding.rs`/`layouts.rs` already use to
//! stay under that file's own size ceiling.
//!
//! `testContainerConnection` is the one call that spawns a real process: it
//! runs on a worker thread and reports back through
//! `containerConnectionTested`, queued onto the Qt thread exactly as
//! `TestServiceRust::run_all` already does for a test run — never block the
//! UI thread on a daemon that may not be reachable.

use std::pin::Pin;
use std::time::Duration;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::connection::{ConnectionConfig, Engine};
use container_core::discovery;
use container_core::probe;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};
use crate::bridge::settings::commit_to_project;

fn to_ffi_container_settings(
    settings: &app_config::ContainerSettings,
) -> ffi::FfiContainerSettings {
    ffi::FfiContainerSettings {
        show_stopped_containers: settings.show_stopped_containers_or_default(),
        show_untagged_images: settings.show_untagged_images_or_default(),
        selinux_relabel: settings.selinux_relabel,
    }
}

fn from_ffi_container_settings(
    row: &ffi::FfiContainerSettings,
    previous: &app_config::ContainerSettings,
) -> app_config::ContainerSettings {
    app_config::ContainerSettings {
        show_stopped_containers: Some(row.show_stopped_containers),
        show_untagged_images: Some(row.show_untagged_images),
        selinux_relabel: row.selinux_relabel,
        connections: previous.connections.clone(),
        registries: previous.registries.clone(),
        targets: previous.targets.clone(),
    }
}

fn to_ffi_connection(
    setting: &app_config::ContainerConnectionSetting,
) -> ffi::FfiContainerConnection {
    ffi::FfiContainerConnection {
        id: QString::from(setting.id.as_str()),
        name: QString::from(setting.name.as_str()),
        engine: QString::from(setting.engine.as_str()),
        kind: QString::from(setting.kind.as_str()),
        path: QString::from(setting.path.as_str()),
        url: QString::from(setting.url.as_str()),
        cert_dir: QString::from(setting.cert_dir.as_str()),
        identity: QString::from(setting.identity.as_str()),
        distro: QString::from(setting.distro.as_str()),
        resource_name: QString::from(setting.resource_name.as_str()),
        executable: QString::from(setting.executable.as_str()),
        compose_executable: QString::from(setting.compose_executable.as_str()),
    }
}

fn from_ffi_connection(
    row: &ffi::FfiContainerConnection,
) -> app_config::ContainerConnectionSetting {
    app_config::ContainerConnectionSetting {
        id: row.id.to_string(),
        name: row.name.to_string(),
        engine: row.engine.to_string(),
        kind: row.kind.to_string(),
        path: row.path.to_string(),
        url: row.url.to_string(),
        cert_dir: row.cert_dir.to_string(),
        identity: row.identity.to_string(),
        distro: row.distro.to_string(),
        resource_name: row.resource_name.to_string(),
        executable: row.executable.to_string(),
        compose_executable: row.compose_executable.to_string(),
    }
}

fn engine_id(engine: Engine) -> &'static str {
    engine.id()
}

impl ffi::AppSettings {
    pub fn container_settings(&self) -> ffi::FfiContainerSettings {
        let containers = match *self.scope.borrow() {
            settings_model::Scope::Project => crate::bridge::convert::load_project_settings()
                .containers
                .unwrap_or_default(),
            _ => crate::bridge::convert::load_settings().containers,
        };
        to_ffi_container_settings(&containers)
    }

    pub fn save_container_settings(&self, settings: &ffi::FfiContainerSettings) -> FfiResult {
        if *self.scope.borrow() == settings_model::Scope::Project {
            let previous = crate::bridge::convert::load_project_settings()
                .containers
                .unwrap_or_default();
            let updated = from_ffi_container_settings(settings, &previous);
            return commit_to_project(|project| project.containers = Some(updated));
        }
        let config_dir = app_core::resolve_config_dir();
        let previous = crate::bridge::convert::load_settings().containers;
        let updated = from_ffi_container_settings(settings, &previous);
        match app_config::update(&config_dir, |loaded| loaded.containers = updated) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }

    pub fn container_connections(&self) -> Vec<ffi::FfiContainerConnection> {
        let containers = match *self.scope.borrow() {
            settings_model::Scope::Project => crate::bridge::convert::load_project_settings()
                .containers
                .unwrap_or_default(),
            _ => crate::bridge::convert::load_settings().containers,
        };
        containers
            .connections
            .iter()
            .map(to_ffi_connection)
            .collect()
    }

    pub fn save_container_connections(
        &self,
        connections: Vec<ffi::FfiContainerConnection>,
    ) -> FfiResult {
        let rows: Vec<app_config::ContainerConnectionSetting> =
            connections.iter().map(from_ffi_connection).collect();

        if *self.scope.borrow() == settings_model::Scope::Project {
            let previous = crate::bridge::convert::load_project_settings()
                .containers
                .unwrap_or_default();
            let updated = app_config::ContainerSettings {
                connections: rows,
                ..previous
            };
            return commit_to_project(|project| project.containers = Some(updated));
        }
        let config_dir = app_core::resolve_config_dir();
        let previous = crate::bridge::convert::load_settings().containers;
        let updated = app_config::ContainerSettings {
            connections: rows,
            ..previous
        };
        match app_config::update(&config_dir, |loaded| loaded.containers = updated) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }

    /// Every connection this build can find on its own: `docker context ls`,
    /// `podman system connection list`/`podman machine list` (each skipped,
    /// not failed, when its CLI is missing or refuses — one absent engine
    /// should not hide the other's contexts), plus the Colima/Rancher
    /// Desktop/rootless-Podman socket presets, which need no CLI at all.
    pub fn discover_container_connections(&self) -> Vec<ffi::FfiDiscoveredConnection> {
        let work_dir = std::env::current_dir().unwrap_or_default();
        let mut found = Vec::new();

        if let Ok(output) = process_exec::run(
            "docker",
            &["context", "ls", "--format", "json"],
            &work_dir,
            None,
            Duration::from_secs(5),
            &[],
        ) {
            if output.status.success() {
                found.extend(discovery::parse_docker_contexts(&String::from_utf8_lossy(
                    &output.stdout,
                )));
            }
        }

        if let Ok(output) = process_exec::run(
            "podman",
            &["system", "connection", "list", "--format", "json"],
            &work_dir,
            None,
            Duration::from_secs(5),
            &[],
        ) {
            if output.status.success() {
                found.extend(discovery::parse_podman_connections(
                    &String::from_utf8_lossy(&output.stdout),
                ));
            }
        }

        if let Ok(output) = process_exec::run(
            "podman",
            &["machine", "list", "--format", "json"],
            &work_dir,
            None,
            Duration::from_secs(5),
            &[],
        ) {
            if output.status.success() {
                found.extend(discovery::parse_podman_machines(&String::from_utf8_lossy(
                    &output.stdout,
                )));
            }
        }

        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        if !home.is_empty() {
            found.push(discovery::colima_socket_preset(&home));
            found.push(discovery::rancher_desktop_socket_preset(&home));
        }
        #[cfg(unix)]
        if let Ok(output) =
            process_exec::run("id", &["-u"], &work_dir, None, Duration::from_secs(5), &[])
        {
            if let Ok(uid) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
            {
                found.push(discovery::default_rootless_podman_socket(uid));
            }
        }

        found
            .into_iter()
            .map(|candidate| {
                let (path, url, distro, resource_name) = match &candidate.kind {
                    container_core::connection::ConnectionKind::UnixSocket { path }
                    | container_core::connection::ConnectionKind::NamedPipe { path } => {
                        (path.clone(), String::new(), String::new(), String::new())
                    }
                    container_core::connection::ConnectionKind::Tcp { url, .. } => {
                        (String::new(), url.clone(), String::new(), String::new())
                    }
                    container_core::connection::ConnectionKind::Ssh { url, .. } => {
                        (String::new(), url.clone(), String::new(), String::new())
                    }
                    container_core::connection::ConnectionKind::Wsl { distro } => {
                        (String::new(), String::new(), distro.clone(), String::new())
                    }
                    container_core::connection::ConnectionKind::Context { name }
                    | container_core::connection::ConnectionKind::PodmanMachine { name } => {
                        (String::new(), String::new(), String::new(), name.clone())
                    }
                    container_core::connection::ConnectionKind::Auto
                    | container_core::connection::ConnectionKind::Minikube => {
                        (String::new(), String::new(), String::new(), String::new())
                    }
                };
                ffi::FfiDiscoveredConnection {
                    label: QString::from(candidate.label.as_str()),
                    engine: QString::from(engine_id(candidate.engine)),
                    kind: QString::from(candidate.kind.id()),
                    path: QString::from(path.as_str()),
                    url: QString::from(url.as_str()),
                    distro: QString::from(distro.as_str()),
                    resource_name: QString::from(resource_name.as_str()),
                }
            })
            .collect()
    }

    /// "Test connection": probes on a worker thread and reports through
    /// `containerConnectionTested`. `connection` is read into an owned
    /// `ContainerConnectionSetting` before spawning — the worker must not
    /// touch anything `Pin<&mut AppSettings>`-shaped, the same rule
    /// `TestServiceRust::run_all` follows for its own worker thread.
    pub fn test_container_connection(
        self: Pin<&mut Self>,
        connection: &ffi::FfiContainerConnection,
    ) -> FfiResult {
        let setting = from_ffi_connection(connection);
        let config = ConnectionConfig::from_setting(&setting);
        let engine = config.engine;
        let qt_thread = self.qt_thread();

        std::thread::spawn(move || {
            let work_dir = std::env::current_dir().unwrap_or_default();
            let mut invocation = config.invocation();
            if matches!(
                config.kind,
                container_core::connection::ConnectionKind::Minikube
            ) {
                match container_core::connection::minikube_docker_env(&work_dir) {
                    Ok(env) => invocation = invocation.with_extra_env(env),
                    Err(error) => {
                        let _ = qt_thread.queue(move |mut settings: Pin<&mut ffi::AppSettings>| {
                            settings.as_mut().container_connection_tested(
                                false,
                                QString::from(error.to_string().as_str()),
                            );
                        });
                        return;
                    }
                }
            }
            let result = probe::probe(&invocation, engine, &work_dir);
            let _ = qt_thread.queue(move |mut settings: Pin<&mut ffi::AppSettings>| {
                let (ok, message) = match result {
                    Ok(info) => (true, info.to_string()),
                    Err(error) => (false, error.to_string()),
                };
                settings
                    .as_mut()
                    .container_connection_tested(ok, QString::from(message.as_str()));
            });
        });

        FfiResult::default()
    }
}

fn to_ffi_registry(setting: &app_config::RegistrySetting) -> ffi::FfiRegistrySetting {
    ffi::FfiRegistrySetting {
        id: QString::from(setting.id.as_str()),
        name: QString::from(setting.name.as_str()),
        kind: QString::from(setting.kind.as_str()),
        address: QString::from(setting.address.as_str()),
        username: QString::from(setting.username.as_str()),
        gitlab_project: QString::from(setting.gitlab_project.as_str()),
        token_auth: setting.token_auth,
    }
}

fn from_ffi_registry(row: &ffi::FfiRegistrySetting) -> app_config::RegistrySetting {
    app_config::RegistrySetting {
        id: row.id.to_string(),
        name: row.name.to_string(),
        kind: row.kind.to_string(),
        address: row.address.to_string(),
        username: row.username.to_string(),
        gitlab_project: row.gitlab_project.to_string(),
        token_auth: row.token_auth,
    }
}

impl ffi::AppSettings {
    pub fn registries(&self) -> Vec<ffi::FfiRegistrySetting> {
        let containers = match *self.scope.borrow() {
            settings_model::Scope::Project => crate::bridge::convert::load_project_settings()
                .containers
                .unwrap_or_default(),
            _ => crate::bridge::convert::load_settings().containers,
        };
        containers.registries.iter().map(to_ffi_registry).collect()
    }

    pub fn save_registries(&self, registries: Vec<ffi::FfiRegistrySetting>) -> FfiResult {
        let rows: Vec<app_config::RegistrySetting> =
            registries.iter().map(from_ffi_registry).collect();

        if *self.scope.borrow() == settings_model::Scope::Project {
            let previous = crate::bridge::convert::load_project_settings()
                .containers
                .unwrap_or_default();
            let updated = app_config::ContainerSettings {
                registries: rows,
                ..previous
            };
            return commit_to_project(|project| project.containers = Some(updated));
        }
        let config_dir = app_core::resolve_config_dir();
        let previous = crate::bridge::convert::load_settings().containers;
        let updated = app_config::ContainerSettings {
            registries: rows,
            ..previous
        };
        match app_config::update(&config_dir, |loaded| loaded.containers = updated) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }

    /// "Test connection" for a registry (C7): runs `RegistryClient::
    /// test_connection` on a worker thread — a real network call, never on
    /// the Qt thread — and reports through `registryTested`.
    pub fn test_registry_connection(
        self: Pin<&mut Self>,
        registry: &ffi::FfiRegistrySetting,
        secret: &QString,
    ) -> FfiResult {
        let setting = from_ffi_registry(registry);
        let secret = secret.to_string();
        let qt_thread = self.qt_thread();

        std::thread::spawn(move || {
            let kind = container_core::registry_ref::RegistryKind::from_id(&setting.kind);
            let result = container_registry::registry::RegistryClient::new(
                &setting.address,
                kind,
                &setting.username,
                &secret,
                &setting.gitlab_project,
            )
            .and_then(|client| client.test_connection());
            let (ok, message) = match result {
                Ok(info) => (true, info.to_string()),
                Err(error) => (false, error.to_string()),
            };
            let _ = qt_thread.queue(move |mut settings: Pin<&mut ffi::AppSettings>| {
                settings
                    .as_mut()
                    .registry_tested(ok, QString::from(message.as_str()));
            });
        });

        FfiResult::default()
    }

    /// Store `secret` in the OS keychain for `id` — runs synchronously
    /// (a local keychain call, not a network one, so no worker thread is
    /// needed the way `testRegistryConnection` needs one).
    pub fn store_registry_secret(&self, id: &QString, secret: &QString) -> FfiResult {
        match container_registry::secrets::SecretStore::store(&id.to_string(), &secret.to_string())
        {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    pub fn has_registry_secret(&self, id: &QString) -> bool {
        container_registry::secrets::SecretStore::has(&id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_settings_round_trip() {
        let settings = app_config::ContainerSettings {
            show_stopped_containers: Some(false),
            show_untagged_images: Some(true),
            selinux_relabel: true,
            ..app_config::ContainerSettings::default()
        };
        let ffi = to_ffi_container_settings(&settings);
        assert!(!ffi.show_stopped_containers);
        assert!(ffi.show_untagged_images);
        assert!(ffi.selinux_relabel);

        let previous = app_config::ContainerSettings::default();
        let restored = from_ffi_container_settings(&ffi, &previous);
        assert_eq!(restored.show_stopped_containers, Some(false));
        assert_eq!(restored.show_untagged_images, Some(true));
        assert!(restored.selinux_relabel);
    }

    #[test]
    fn ffi_connection_round_trips_every_field() {
        let setting = app_config::ContainerConnectionSetting {
            id: "c1".to_string(),
            name: "Docker (local)".to_string(),
            engine: "docker".to_string(),
            kind: "tcp".to_string(),
            path: String::new(),
            url: "tcp://10.0.0.5:2376".to_string(),
            cert_dir: "/certs".to_string(),
            identity: String::new(),
            distro: String::new(),
            resource_name: String::new(),
            executable: String::new(),
            compose_executable: "docker-compose".to_string(),
        };
        let ffi = to_ffi_connection(&setting);
        let restored = from_ffi_connection(&ffi);
        assert_eq!(restored, setting);
    }

    #[test]
    fn ffi_registry_round_trips_every_field() {
        let setting = app_config::RegistrySetting {
            id: "r1".to_string(),
            name: "GHCR".to_string(),
            kind: "v2".to_string(),
            address: "ghcr.io".to_string(),
            username: "alice".to_string(),
            gitlab_project: String::new(),
            token_auth: true,
        };
        let ffi = to_ffi_registry(&setting);
        let restored = from_ffi_registry(&ffi);
        assert_eq!(restored, setting);
    }
}
