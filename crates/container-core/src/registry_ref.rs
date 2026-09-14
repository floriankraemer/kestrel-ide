//! Registry reference formatting and push/pull argv (C7, ADR-0055): pure
//! rules over a configured registry's kind and address, shared by
//! `container-registry`'s HTTP client (which kind means which API,
//! [`RegistryKind`] re-exported from there) and by the bridge's push/pull
//! command builders in [`crate::session`]. Nothing here performs I/O.

use crate::run_config::quote;

/// Which registry API a configured registry speaks. Mirrors
/// `app_config::RegistrySetting::kind`'s string ("hub"/"gitlab"/"v2"/
/// "generic"; unrecognised values map to `Generic`, the safest default —
/// push-only, never assuming browse/pull support that was never verified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryKind {
    Hub,
    GitLab,
    DockerV2,
    Generic,
}

impl RegistryKind {
    pub fn id(self) -> &'static str {
        match self {
            RegistryKind::Hub => "hub",
            RegistryKind::GitLab => "gitlab",
            RegistryKind::DockerV2 => "v2",
            RegistryKind::Generic => "generic",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "hub" => RegistryKind::Hub,
            "gitlab" => RegistryKind::GitLab,
            "v2" => RegistryKind::DockerV2,
            _ => RegistryKind::Generic,
        }
    }
}

/// The full `address/repository:tag` a user sees and a pull/push argv
/// uses. Docker Hub's own CLI convention: a namespace-less repository is
/// `library/<repo>` under `docker.io`, everything else is `docker.io/
/// <repo>` — `docker pull nginx` and `docker pull docker.io/library/nginx`
/// name the same image, but only the fully qualified form still
/// disambiguates it from a same-named repository on another configured
/// registry, which is the point of formatting it at all. `tag` empty
/// yields the bare repository reference (a registry-repo node has no tag
/// yet, only its repository path).
pub fn format_reference(kind: RegistryKind, address: &str, repository: &str, tag: &str) -> String {
    let repository = repository.trim_matches('/');
    let base = if kind == RegistryKind::Hub {
        if repository.contains('/') {
            format!("docker.io/{repository}")
        } else {
            format!("docker.io/library/{repository}")
        }
    } else {
        let address = address.trim_matches('/');
        if address.is_empty() {
            repository.to_string()
        } else {
            format!("{address}/{repository}")
        }
    };
    if tag.is_empty() {
        base
    } else {
        format!("{base}:{tag}")
    }
}

/// `login [--username <user>] --password-stdin <address>` argv. `username`
/// empty omits `--username`, the CLI's own shape for a token-only login.
pub fn login_args(address: &str, username: &str) -> Vec<String> {
    let mut args = vec!["login".to_string()];
    if !username.is_empty() {
        args.push("--username".to_string());
        args.push(username.to_string());
    }
    args.push("--password-stdin".to_string());
    args.push(address.to_string());
    args
}

/// `push <reference>`.
pub fn push_args(reference: &str) -> Vec<String> {
    vec!["push".to_string(), reference.to_string()]
}

/// One shell line running `program argv...`, quoted for a script —
/// [`crate::run_config::quote`] reused so a login/pull/push line matches
/// the preview text run configurations already produce for other
/// commands.
pub fn shell_line(program: &str, argv: &[String]) -> String {
    std::iter::once(program)
        .chain(argv.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_ids_round_trip() {
        for kind in [
            RegistryKind::Hub,
            RegistryKind::GitLab,
            RegistryKind::DockerV2,
            RegistryKind::Generic,
        ] {
            assert_eq!(RegistryKind::from_id(kind.id()), kind);
        }
        assert_eq!(RegistryKind::from_id("nonsense"), RegistryKind::Generic);
    }

    #[test]
    fn hub_references_special_case_the_library_namespace() {
        assert_eq!(
            format_reference(RegistryKind::Hub, "docker.io", "nginx", "1.27"),
            "docker.io/library/nginx:1.27"
        );
        assert_eq!(
            format_reference(RegistryKind::Hub, "docker.io", "bitnami/redis", "7"),
            "docker.io/bitnami/redis:7"
        );
    }

    #[test]
    fn other_kinds_use_address_slash_repository_colon_tag() {
        assert_eq!(
            format_reference(RegistryKind::DockerV2, "ghcr.io", "acme/app", "latest"),
            "ghcr.io/acme/app:latest"
        );
        assert_eq!(
            format_reference(
                RegistryKind::GitLab,
                "registry.gitlab.com/ns/proj",
                "app",
                "v1"
            ),
            "registry.gitlab.com/ns/proj/app:v1"
        );
    }

    #[test]
    fn empty_tag_yields_a_bare_repository_reference() {
        assert_eq!(
            format_reference(RegistryKind::DockerV2, "ghcr.io", "acme/app", ""),
            "ghcr.io/acme/app"
        );
    }

    #[test]
    fn login_argv_omits_username_when_empty() {
        assert_eq!(
            login_args("ghcr.io", "alice"),
            vec![
                "login",
                "--username",
                "alice",
                "--password-stdin",
                "ghcr.io"
            ]
        );
        assert_eq!(
            login_args("ghcr.io", ""),
            vec!["login", "--password-stdin", "ghcr.io"]
        );
    }

    #[test]
    fn push_argv() {
        assert_eq!(
            push_args("ghcr.io/acme/app:v1"),
            vec!["push", "ghcr.io/acme/app:v1"]
        );
    }

    #[test]
    fn shell_line_quotes_only_what_needs_it() {
        assert_eq!(
            shell_line("docker", &["pull".to_string(), "nginx:1.27".to_string()]),
            "docker pull nginx:1.27"
        );
        assert_eq!(
            shell_line(
                "docker",
                &[
                    "login".to_string(),
                    "--username".to_string(),
                    "a b".to_string()
                ]
            ),
            "docker login --username 'a b'"
        );
    }
}
