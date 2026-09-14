//! Podman pod lifecycle (C9): `pod ls --format json` -> [`crate::model::Pod`]
//! already lands in [`crate::snapshot::EngineSnapshot`] (C2) — this module is
//! only the actions the dock's pod context menu needs, the same argv-builder
//! shape as [`crate::ops`]'s container actions, run through the same
//! [`crate::ops::run_op`] executor so they share its stderr classification.
//! Docker has no `pod` subcommand — every function here is only ever called
//! on a Podman connection.

/// `pod start <id>`.
pub fn start_args(id: &str) -> Vec<String> {
    vec!["pod".to_string(), "start".to_string(), id.to_string()]
}

/// `pod stop <id>`.
pub fn stop_args(id: &str) -> Vec<String> {
    vec!["pod".to_string(), "stop".to_string(), id.to_string()]
}

/// `pod restart <id>`.
pub fn restart_args(id: &str) -> Vec<String> {
    vec!["pod".to_string(), "restart".to_string(), id.to_string()]
}

/// `pod rm [-f] <id>`.
pub fn remove_args(id: &str, force: bool) -> Vec<String> {
    let mut args = vec!["pod".to_string(), "rm".to_string()];
    if force {
        args.push("-f".to_string());
    }
    args.push(id.to_string());
    args
}

/// `pod inspect <id>` — same shape as [`crate::session::inspect_json`], one
/// pod rather than one container.
pub fn inspect_args(id: &str) -> Vec<String> {
    vec!["pod".to_string(), "inspect".to_string(), id.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_argv() {
        assert_eq!(start_args("p1"), vec!["pod", "start", "p1"]);
        assert_eq!(stop_args("p1"), vec!["pod", "stop", "p1"]);
        assert_eq!(restart_args("p1"), vec!["pod", "restart", "p1"]);
        assert_eq!(inspect_args("p1"), vec!["pod", "inspect", "p1"]);
    }

    #[test]
    fn remove_argv_with_and_without_force() {
        assert_eq!(remove_args("p1", false), vec!["pod", "rm", "p1"]);
        assert_eq!(remove_args("p1", true), vec!["pod", "rm", "-f", "p1"]);
    }
}
