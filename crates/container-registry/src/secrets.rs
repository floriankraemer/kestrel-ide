//! The registry credential store (C7, ADR-0055): a password/token per
//! configured registry in the OS keychain, never in `settings.toml`
//! (`app_config::RegistrySetting` carries no secret field at all).
//!
//! The generic keychain mechanics ([`SecretStore`], [`SecretError`]) now
//! live in the [`secret_store`] crate (extracted per ADR-0058 §5, shared
//! with `db-core`'s data-source passwords). This module keeps only what
//! is C7-specific: the service name (`"ide.containers"`, user = the
//! registry's own id — the same "one entry per stable id" shape a
//! credential manager already expects) and the wording shown for
//! `SecretError::Unavailable`, since C7's actionable fallback (`docker
//! login`) is not something a shared crate can know — `db-core`'s own
//! callers word theirs differently.
pub use secret_store::{SecretError, SecretStore};

/// Keychain service name every registry's entry is stored under.
const SERVICE: &str = "ide.containers";

/// The fixed hint shown whenever no OS keychain/backend is reachable —
/// asserted verbatim by the fallback-path test, so it must never be
/// reworded without updating that assertion too.
pub const NO_KEYCHAIN_HINT: &str =
    "No OS keychain available — run `docker login <address>` and leave the password empty";

/// The one `SecretStore` C7 ever needs — cheap to construct, so callers
/// just ask for a fresh one rather than threading a shared instance
/// through.
pub fn store() -> SecretStore {
    SecretStore::new(SERVICE)
}

/// C7's own wording for a [`SecretError`], for callers that surface the
/// message to the user (`Other`'s message is already specific to the one
/// call that failed, so it passes through unchanged).
pub fn describe(error: &SecretError) -> String {
    match error {
        SecretError::Unavailable(_) => NO_KEYCHAIN_HINT.to_string(),
        SecretError::Other(message) => message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_keychain_backend_maps_to_the_documented_unavailable_message() {
        // The linux-builder/CI sandbox this runs in has no D-Bus Secret
        // Service and, per keyring v3's `linux-native` backend, may also
        // have no usable session keyutils ring (ADR-0021's exact
        // precedent) — so a real store attempt here either succeeds
        // (keyutils is reachable) or fails as `Unavailable`. Both branches
        // are asserted so this test passes in either environment and still
        // exercises the mapping this task requires wherever it fails for
        // real, without being gated behind `#[ignore]`.
        let id = "c7-fallback-test-registry";
        let store = store();
        match store.store(id, "s3cr3t") {
            Ok(()) => {
                assert_eq!(store.load(id), Ok(Some("s3cr3t".to_string())));
                assert!(store.has(id));
                assert_eq!(store.delete(id), Ok(()));
                assert_eq!(store.load(id), Ok(None));
            }
            Err(error @ SecretError::Unavailable(_)) => {
                assert_eq!(describe(&error), NO_KEYCHAIN_HINT);
                assert!(!store.has(id));
            }
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn loading_a_registry_that_was_never_stored_is_none_not_an_error() {
        match store().load("c7-never-stored-registry") {
            Ok(None) => {}
            Ok(Some(_)) => panic!("nothing was ever stored for this id"),
            Err(error @ SecretError::Unavailable(_)) => {
                assert_eq!(describe(&error), NO_KEYCHAIN_HINT)
            }
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn deleting_a_registry_that_was_never_stored_does_not_error() {
        match store().delete("c7-delete-never-stored-registry") {
            Ok(()) => {}
            Err(error @ SecretError::Unavailable(_)) => {
                assert_eq!(describe(&error), NO_KEYCHAIN_HINT)
            }
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn the_unavailable_hint_names_the_cli_fallback() {
        assert!(NO_KEYCHAIN_HINT.contains("docker login"));
        assert!(NO_KEYCHAIN_HINT.contains("<address>"));
    }
}
