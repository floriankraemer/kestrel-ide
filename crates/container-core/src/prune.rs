//! Clean Up (C4, ADR-0055): the Containers/Images/Networks/Volumes groups'
//! "Clean Up" menu matrix, one [`CleanUpKind`] per JetBrains-parity item.
//! [`available`] is checked before a menu item is even enabled — an
//! unsupported combination (Podman has no build-cache prune subcommand)
//! never reaches the CLI at all, rather than failing at runtime.

use crate::connection::Engine;

/// One "Clean Up" menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanUpKind {
    /// "Clean Up > All": every stopped container, every unused network,
    /// every dangling image and every unused volume.
    All,
    StoppedContainers,
    UnusedNetworks,
    UnusedVolumes,
    DanglingImages,
    BuildCache,
}

/// Whether `kind` is offered for `engine` at all. Podman has no
/// `builder prune`/`system prune --build` equivalent (checked against its
/// documented command surface, no `podman` binary on the fixture-capture
/// host to confirm live) — [`BuildCache`](CleanUpKind::BuildCache) is
/// unavailable there, everything else is engine-agnostic (ADR-0055).
pub fn available(kind: CleanUpKind, engine: Engine) -> bool {
    match kind {
        CleanUpKind::BuildCache => matches!(engine, Engine::Docker),
        _ => true,
    }
}

/// The argv (or two, for `All`) `kind` runs on `engine`. Empty when
/// [`available`] would say `false` — the caller checks that first, this is
/// never reached for `BuildCache` on Podman in practice.
pub fn commands(kind: CleanUpKind, engine: Engine) -> Vec<Vec<String>> {
    match kind {
        CleanUpKind::All => vec![
            vec!["system".to_string(), "prune".to_string(), "-f".to_string()],
            crate::volumes::prune_args(),
        ],
        CleanUpKind::StoppedContainers => vec![crate::ops::prune_args()],
        CleanUpKind::UnusedNetworks => vec![crate::networks::prune_args()],
        CleanUpKind::UnusedVolumes => vec![crate::volumes::prune_args()],
        CleanUpKind::DanglingImages => vec![crate::images::image_prune_args(false)],
        CleanUpKind::BuildCache => {
            if matches!(engine, Engine::Docker) {
                vec![vec![
                    "builder".to_string(),
                    "prune".to_string(),
                    "-f".to_string(),
                ]]
            } else {
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cache_is_docker_only() {
        assert!(available(CleanUpKind::BuildCache, Engine::Docker));
        assert!(!available(CleanUpKind::BuildCache, Engine::Podman));
    }

    #[test]
    fn every_other_kind_is_available_on_both_engines() {
        for kind in [
            CleanUpKind::All,
            CleanUpKind::StoppedContainers,
            CleanUpKind::UnusedNetworks,
            CleanUpKind::UnusedVolumes,
            CleanUpKind::DanglingImages,
        ] {
            assert!(available(kind, Engine::Docker));
            assert!(available(kind, Engine::Podman));
        }
    }

    #[test]
    fn all_runs_system_prune_then_volume_prune() {
        let cmds = commands(CleanUpKind::All, Engine::Docker);
        assert_eq!(
            cmds,
            vec![
                vec!["system".to_string(), "prune".to_string(), "-f".to_string()],
                vec!["volume".to_string(), "prune".to_string(), "-f".to_string()],
            ]
        );
        // Podman: same commands (ADR-0055 — the engine is the executable).
        assert_eq!(
            commands(CleanUpKind::All, Engine::Docker),
            commands(CleanUpKind::All, Engine::Podman)
        );
    }

    #[test]
    fn each_single_kind_delegates_to_its_own_module() {
        assert_eq!(
            commands(CleanUpKind::StoppedContainers, Engine::Docker),
            vec![crate::ops::prune_args()]
        );
        assert_eq!(
            commands(CleanUpKind::UnusedNetworks, Engine::Docker),
            vec![crate::networks::prune_args()]
        );
        assert_eq!(
            commands(CleanUpKind::UnusedVolumes, Engine::Docker),
            vec![crate::volumes::prune_args()]
        );
        assert_eq!(
            commands(CleanUpKind::DanglingImages, Engine::Docker),
            vec![crate::images::image_prune_args(false)]
        );
    }

    #[test]
    fn build_cache_commands_empty_on_podman() {
        assert!(commands(CleanUpKind::BuildCache, Engine::Podman).is_empty());
        assert_eq!(
            commands(CleanUpKind::BuildCache, Engine::Docker),
            vec![vec![
                "builder".to_string(),
                "prune".to_string(),
                "-f".to_string()
            ]]
        );
    }
}
