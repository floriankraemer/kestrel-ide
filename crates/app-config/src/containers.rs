//! The `[containers]` section: named Docker/Podman connections, registry
//! entries (no secrets), run targets, and the two dock filters.
//!
//! Persistence only, like [`crate::terminal`]. *What* a connection kind
//! means — which CLI flags or environment variables it becomes — is
//! `container-core`'s rule (ADR-0055): this crate stores every
//! engine/kind-shaped field as a plain string, never as an enum, so a
//! settings file written by a build that knows about one more connection
//! kind still round-trips unchanged through a build that does not (the same
//! "persistence stays dumb" rule ADR-0017/ADR-0039 already draw for run
//! configurations and language servers).
//!
//! Global by default, project-overridable (ADR-0022): which daemon a
//! checkout talks to is a property of the project at least as often as of
//! the person, the same reasoning [`crate::terminal`] documents for the
//! shell.

use serde::{Deserialize, Serialize};

/// The `[containers]` section.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct ContainerSettings {
    /// Named engine connections, in the order the user added them.
    #[serde(default, rename = "connection", skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<ContainerConnectionSetting>,

    /// Configured registries. No credentials here — those live in the OS
    /// keychain (ADR-0055) and are looked up by `registry.id` at the point
    /// of use, never persisted alongside it.
    #[serde(default, rename = "registry", skip_serializing_if = "Vec::is_empty")]
    pub registries: Vec<RegistrySetting>,

    /// Run targets (C8): a placeholder today. Kept as a real field rather
    /// than added later so [`ScopedField::Containers`] and every round-trip
    /// test written against this struct do not have to change shape again
    /// when C8 fills it in.
    #[serde(default, rename = "target", skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ContainerTargetSetting>,

    /// Whether the Containers dock (C2) shows stopped containers by
    /// default. `Option` rather than a bare `bool`: unlike a shell id, "on"
    /// and "never chosen" are the same default here, but the field still
    /// needs to tell "the user turned this off" apart from "this build's
    /// default changed" the way [`Settings::mcp_enabled`] does for the same
    /// reason — a filter a project deliberately narrows should stay
    /// narrowed even if a later release flips the shipped default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_stopped_containers: Option<bool>,

    /// Whether the Images console shows untagged (`<none>`) images by
    /// default. Same `Option` reasoning as
    /// [`ContainerSettings::show_stopped_containers`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_untagged_images: Option<bool>,

    /// Advanced: append the SELinux `:z` relabel suffix to bind mounts this
    /// IDE creates for a container (recreate/run-target). Off by default —
    /// wrong on a non-SELinux host, and no default we could compute here
    /// beats an explicit opt-in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub selinux_relabel: bool,
}

/// Default: dock filters both show everything, the least surprising first
/// run of a feature nobody has configured yet.
pub const DEFAULT_SHOW_STOPPED_CONTAINERS: bool = true;
pub const DEFAULT_SHOW_UNTAGGED_IMAGES: bool = true;

impl ContainerSettings {
    /// [`Self::show_stopped_containers`], defaulted.
    pub fn show_stopped_containers_or_default(&self) -> bool {
        self.show_stopped_containers
            .unwrap_or(DEFAULT_SHOW_STOPPED_CONTAINERS)
    }

    /// [`Self::show_untagged_images`], defaulted.
    pub fn show_untagged_images_or_default(&self) -> bool {
        self.show_untagged_images
            .unwrap_or(DEFAULT_SHOW_UNTAGGED_IMAGES)
    }
}

/// One named connection to a Docker or Podman engine.
///
/// Every field a connection kind could need is a plain, always-present
/// string (empty when unused) rather than an `Option` per kind or a nested
/// enum: `toml`'s serde support has no clean way to flatten "one of these
/// shapes, tagged by another field" without either a lot of boilerplate or
/// losing unknown-kind round-tripping, and a project file this is meant to
/// be readable in a diff. `container_core::connection` is what turns this
/// row into a typed `ConnectionKind`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerConnectionSetting {
    /// Stable id, generated when the connection is added. Never shown; it
    /// is what the run-target/run-configuration `run_on` string points at.
    #[serde(default)]
    pub id: String,
    /// The name shown in the Settings page and the Containers dock tree.
    #[serde(default)]
    pub name: String,
    /// `"docker"` or `"podman"`. Unrecognised values map to
    /// `container_core::connection::Engine::Docker`.
    #[serde(default)]
    pub engine: String,
    /// One of `"auto"`, `"unix_socket"`, `"tcp"`, `"named_pipe"`,
    /// `"context"`, `"ssh"`, `"wsl"`, `"podman_machine"`, `"minikube"`.
    /// Unrecognised values map to `ConnectionKind::Auto`.
    #[serde(default)]
    pub kind: String,
    /// Socket path (`unix_socket`) or named-pipe path (`named_pipe`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Daemon URL (`tcp`, `ssh`) — e.g. `tcp://host:2376`, `ssh://user@host`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// TLS certificate directory (`tcp`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cert_dir: String,
    /// SSH identity (private key) file (`ssh`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub identity: String,
    /// WSL distro name (`wsl`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub distro: String,
    /// Docker context name (`context`) or Podman machine/connection name
    /// (`podman_machine`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_name: String,
    /// Overrides the engine's default CLI name (e.g. `podman-remote`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub executable: String,
    /// Overrides the compose CLI invocation (e.g. a standalone
    /// `docker-compose` binary instead of the `docker compose` plugin).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compose_executable: String,
}

/// A configured registry, credentials excluded (ADR-0055: those live in
/// the OS keychain, keyed by `id` — `container_registry::secrets::
/// SecretStore`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct RegistrySetting {
    /// Stable id, generated when the registry is added. What the OS
    /// keychain entry is keyed by, and what a run configuration/tree node
    /// id points at — never shown.
    #[serde(default)]
    pub id: String,
    /// The name shown in the Settings page and the Containers dock tree.
    #[serde(default)]
    pub name: String,
    /// `"hub"`, `"gitlab"`, `"v2"`, or `"generic"`. Unrecognised values map
    /// to `container_core::registry_ref::RegistryKind::Generic`
    /// (push-only) the same "never assume support" default that crate
    /// documents. GHCR/Quay are `"v2"` with a prefilled `address` — a
    /// Settings-page convenience, not a kind of their own.
    #[serde(default)]
    pub kind: String,
    /// Host\[:port\]\[/path\], e.g. `ghcr.io`, `registry.gitlab.com`,
    /// `docker.io` — empty for Docker Hub, whose address is implicit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// GitLab only: `group/project` or a numeric project id, so browsing
    /// lists that project's container registry rather than falling back to
    /// the plain V2 catalog (which most GitLab tokens cannot read at all).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gitlab_project: String,
    /// Whether the stored secret is a personal access token rather than a
    /// password — some registries (GitLab, GHCR) special-case this in
    /// their own UI wording, but it changes no argv here: `--password-stdin`
    /// takes either one identically.
    #[serde(default)]
    pub token_auth: bool,
}

/// A run target (C8). Empty placeholder for now — the shape lands with C8,
/// this only reserves the section so [`ContainerSettings`] never has to
/// change its own field list again.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerTargetSetting {
    #[serde(default)]
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_mean_no_connections_and_both_filters_on() {
        let settings = ContainerSettings::default();
        assert!(settings.connections.is_empty());
        assert!(settings.registries.is_empty());
        assert!(settings.targets.is_empty());
        assert!(settings.show_stopped_containers_or_default());
        assert!(settings.show_untagged_images_or_default());
        assert!(!settings.selinux_relabel);
    }

    #[test]
    fn an_untouched_section_writes_nothing() {
        let text = toml::to_string(&ContainerSettings::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn a_configured_section_round_trips_through_toml() {
        let settings = ContainerSettings {
            connections: vec![
                ContainerConnectionSetting {
                    id: "local-docker".to_string(),
                    name: "Docker (local)".to_string(),
                    engine: "docker".to_string(),
                    kind: "unix_socket".to_string(),
                    path: "/var/run/docker.sock".to_string(),
                    ..ContainerConnectionSetting::default()
                },
                ContainerConnectionSetting {
                    id: "remote-podman".to_string(),
                    name: "Podman (SSH)".to_string(),
                    engine: "podman".to_string(),
                    kind: "ssh".to_string(),
                    url: "ssh://user@example.com".to_string(),
                    identity: "/home/user/.ssh/id_ed25519".to_string(),
                    executable: "podman-remote".to_string(),
                    ..ContainerConnectionSetting::default()
                },
            ],
            registries: vec![
                RegistrySetting {
                    id: "ghcr".to_string(),
                    name: "GHCR".to_string(),
                    address: "ghcr.io".to_string(),
                    username: "florian".to_string(),
                    kind: "v2".to_string(),
                    gitlab_project: String::new(),
                    token_auth: true,
                },
                RegistrySetting {
                    id: "gl".to_string(),
                    name: "GitLab".to_string(),
                    address: "registry.gitlab.com".to_string(),
                    username: "florian".to_string(),
                    kind: "gitlab".to_string(),
                    gitlab_project: "team/app".to_string(),
                    token_auth: true,
                },
            ],
            targets: Vec::new(),
            show_stopped_containers: Some(false),
            show_untagged_images: Some(true),
            selinux_relabel: true,
        };

        let text = toml::to_string(&settings).expect("serialize");
        let parsed: ContainerSettings = toml::from_str(&text).expect("deserialize");

        assert_eq!(parsed, settings);
    }

    #[test]
    fn a_registry_setting_round_trips_with_no_secret_field_at_all() {
        // ADR-0055: secrets never touch TOML — asserting the struct has no
        // secret-shaped field to serialize is the point of this test, not
        // just that the fields it does have round-trip.
        let registry = RegistrySetting {
            id: "ghcr".to_string(),
            name: "GHCR".to_string(),
            kind: "v2".to_string(),
            address: "ghcr.io".to_string(),
            username: "florian".to_string(),
            gitlab_project: String::new(),
            token_auth: true,
        };
        let text = toml::to_string(&registry).expect("serialize");
        assert!(!text.to_lowercase().contains("password"));
        assert!(!text.to_lowercase().contains("secret"));
        assert!(!text.to_lowercase().contains("token") || text.contains("token_auth"));
        let parsed: RegistrySetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, registry);
    }

    #[test]
    fn an_untouched_registry_setting_writes_only_its_id_kind_and_flag() {
        let text = toml::to_string(&RegistrySetting::default()).expect("serialize");
        assert_eq!(
            text.trim(),
            "id = \"\"\nname = \"\"\nkind = \"\"\ntoken_auth = false"
        );
    }

    #[test]
    fn unset_filters_default_to_showing_everything() {
        let settings = ContainerSettings::default();
        assert!(settings.show_stopped_containers.is_none());
        assert!(settings.show_untagged_images.is_none());
        assert!(settings.show_stopped_containers_or_default());
        assert!(settings.show_untagged_images_or_default());
    }

    #[test]
    fn an_explicit_false_stays_false_across_a_round_trip() {
        let settings = ContainerSettings {
            show_stopped_containers: Some(false),
            ..ContainerSettings::default()
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: ContainerSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.show_stopped_containers, Some(false));
        assert!(!parsed.show_stopped_containers_or_default());
    }
}
