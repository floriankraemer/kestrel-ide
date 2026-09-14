//! The registry credential store (C7, ADR-0055): a password/token per
//! configured registry in the OS keychain, never in `settings.toml`
//! (`app_config::RegistrySetting` carries no secret field at all).
//!
//! Service name `"ide.containers"`, user = the registry's own id — the
//! same "one entry per stable id" shape a credential manager already
//! expects, so a user who goes looking for it in `seahorse`/Credential
//! Manager/Keychain Access finds one row per registry, not a single blob.
//!
//! No OS keychain is available on every machine this IDE runs on (ADR-0021
//! already documents this for `ai-chat-core`: the linux-builder image, CI,
//! and a minimal desktop all lack a running D-Bus Secret Service) — Linux
//! uses `keyring`'s `linux-native` backend (kernel keyutils, no D-Bus, no
//! system package needed to even build) for exactly that reason, and every
//! call here still maps a genuinely unreachable backend into
//! [`SecretError::Unavailable`] carrying one fixed, actionable message
//! rather than propagating a raw platform error, so a caller can show it
//! as-is and push/pull still work through the engine CLI's own credential
//! store (`docker login`).

use keyring::Entry;

/// Keychain service name every registry's entry is stored under.
const SERVICE: &str = "ide.containers";

/// The fixed hint shown whenever no OS keychain/backend is reachable —
/// asserted verbatim by the fallback-path test, so it must never be
/// reworded without updating that assertion too.
pub const NO_KEYCHAIN_HINT: &str =
    "No OS keychain available — run `docker login <address>` and leave the password empty";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No OS keychain/backend could be reached at all (headless Linux with
    /// no keyutils/Secret Service, a locked-down sandbox, ...). Carries
    /// [`NO_KEYCHAIN_HINT`] verbatim.
    Unavailable(String),
    /// The keychain is reachable but this call still failed (a corrupted
    /// entry, a permission error on one specific item, ...).
    Other(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Unavailable(message) | SecretError::Other(message) => {
                write!(f, "{message}")
            }
        }
    }
}

impl std::error::Error for SecretError {}

/// Classify a `keyring` error the same way [`crate::registry`]'s HTTP
/// client classifies a `reqwest` one: by matching on the crate's own
/// error variants, not by string-sniffing a platform message. `keyring`
/// v3's [`keyring::Error::NoStorageAccess`] and [`keyring::Error::
/// PlatformFailure`] are both "the backend itself could not be reached or
/// initialised" — the rest (`NoEntry`, a bad credential shape, ...) are
/// per-call failures, not a missing keychain.
fn classify(error: keyring::Error) -> SecretError {
    match error {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            SecretError::Unavailable(NO_KEYCHAIN_HINT.to_string())
        }
        other => SecretError::Other(other.to_string()),
    }
}

fn entry(registry_id: &str) -> Result<Entry, SecretError> {
    Entry::new(SERVICE, registry_id).map_err(classify)
}

/// The credential store. A unit struct rather than a set of free
/// functions only so call sites read `SecretStore::store(...)` next to
/// `RegistryClient::new(...)` — there is no per-instance state to hold.
pub struct SecretStore;

impl SecretStore {
    pub fn store(registry_id: &str, secret: &str) -> Result<(), SecretError> {
        entry(registry_id)?.set_password(secret).map_err(classify)
    }

    /// `Ok(None)` when nothing has been stored for this registry yet — not
    /// an error, the same "unset is not a failure" convention every other
    /// optional setting in this codebase follows.
    pub fn load(registry_id: &str) -> Result<Option<String>, SecretError> {
        match entry(registry_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(classify(error)),
        }
    }

    /// Idempotent: deleting an entry that was never stored is not an
    /// error, so "Remove registry" never has to check first.
    pub fn delete(registry_id: &str) -> Result<(), SecretError> {
        match entry(registry_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(classify(error)),
        }
    }

    /// Whether a secret is stored for this registry, without exposing it —
    /// `AppSettings::hasRegistrySecret`'s answer, so the Registries page
    /// can show "a password is stored" without ever reading it back.
    /// `Unavailable`/`Other` both read as "no" here: a page cannot show a
    /// secret it cannot reach either way, and `load`'s own error is what
    /// tells the user why.
    pub fn has(registry_id: &str) -> bool {
        matches!(Self::load(registry_id), Ok(Some(_)))
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
        match SecretStore::store(id, "s3cr3t") {
            Ok(()) => {
                assert_eq!(SecretStore::load(id), Ok(Some("s3cr3t".to_string())));
                assert!(SecretStore::has(id));
                assert_eq!(SecretStore::delete(id), Ok(()));
                assert_eq!(SecretStore::load(id), Ok(None));
            }
            Err(SecretError::Unavailable(message)) => {
                assert_eq!(message, NO_KEYCHAIN_HINT);
                assert!(!SecretStore::has(id));
            }
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn loading_a_registry_that_was_never_stored_is_none_not_an_error() {
        match SecretStore::load("c7-never-stored-registry") {
            Ok(None) => {}
            Ok(Some(_)) => panic!("nothing was ever stored for this id"),
            Err(SecretError::Unavailable(message)) => assert_eq!(message, NO_KEYCHAIN_HINT),
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn deleting_a_registry_that_was_never_stored_does_not_error() {
        match SecretStore::delete("c7-delete-never-stored-registry") {
            Ok(()) => {}
            Err(SecretError::Unavailable(message)) => assert_eq!(message, NO_KEYCHAIN_HINT),
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn the_unavailable_hint_names_the_cli_fallback() {
        assert!(NO_KEYCHAIN_HINT.contains("docker login"));
        assert!(NO_KEYCHAIN_HINT.contains("<address>"));
    }
}
