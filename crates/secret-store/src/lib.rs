//! A generic per-caller OS-keychain credential store.
//!
//! Extracted from `container-registry/src/secrets.rs` (ADR-0058 §5): C7's
//! registry credential store and F1's database-tools data-source
//! passwords are the same shape — one secret per stable id, in the OS
//! keychain, never in `settings.toml` — differing only in which
//! keychain *service* name their entries are filed under. That name is
//! now a constructor argument (`SecretStore::new("ide.containers")`,
//! `SecretStore::new("ide.database")`) rather than a crate-wide constant,
//! so two callers in the same process never collide on one id.
//!
//! No OS keychain is available on every machine this IDE runs on (ADR-0021
//! already documents this for `ai-chat-core`: the linux-builder image, CI,
//! and a minimal desktop all lack a running D-Bus Secret Service) — Linux
//! uses `keyring`'s `linux-native` backend (kernel keyutils, no D-Bus, no
//! system package needed to even build) for exactly that reason, and every
//! call here still maps a genuinely unreachable backend into
//! [`SecretError::Unavailable`] carrying one fixed, actionable message
//! rather than propagating a raw platform error.

use keyring::Entry;

/// The fixed hint shown whenever no OS keychain/backend is reachable —
/// asserted verbatim by the fallback-path test, so it must never be
/// reworded without updating that assertion too.
pub const NO_KEYCHAIN_HINT: &str =
    "No OS keychain available — store the secret another way and leave this field empty";

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

/// Classify a `keyring` error by matching on the crate's own error
/// variants, not by string-sniffing a platform message. `keyring` v3's
/// [`keyring::Error::NoStorageAccess`] and [`keyring::Error::
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

/// A keychain-backed credential store scoped to one service name. Cheap to
/// construct — it holds nothing but the name — so a caller builds one per
/// use (`SecretStore::new("ide.containers").load(id)`) rather than
/// threading a long-lived instance through.
pub struct SecretStore {
    service: String,
}

impl SecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, id: &str) -> Result<Entry, SecretError> {
        Entry::new(&self.service, id).map_err(classify)
    }

    pub fn store(&self, id: &str, secret: &str) -> Result<(), SecretError> {
        self.entry(id)?.set_password(secret).map_err(classify)
    }

    /// `Ok(None)` when nothing has been stored for this id yet — not an
    /// error, the same "unset is not a failure" convention every other
    /// optional setting in this codebase follows.
    pub fn load(&self, id: &str) -> Result<Option<String>, SecretError> {
        match self.entry(id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(classify(error)),
        }
    }

    /// Idempotent: deleting an entry that was never stored is not an
    /// error, so "Remove" never has to check first.
    pub fn delete(&self, id: &str) -> Result<(), SecretError> {
        match self.entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(classify(error)),
        }
    }

    /// Whether a secret is stored for this id, without exposing it.
    /// `Unavailable`/`Other` both read as "no" here: a page cannot show a
    /// secret it cannot reach either way, and `load`'s own error is what
    /// tells the user why.
    pub fn has(&self, id: &str) -> bool {
        matches!(self.load(id), Ok(Some(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SecretStore {
        SecretStore::new("ide.secret-store-tests")
    }

    #[test]
    fn no_keychain_backend_maps_to_the_documented_unavailable_message() {
        // The linux-builder/CI sandbox this runs in has no D-Bus Secret
        // Service and, per keyring v3's `linux-native` backend, may also
        // have no usable session keyutils ring (ADR-0021's exact
        // precedent) — so a real store attempt here either succeeds
        // (keyutils is reachable) or fails as `Unavailable`. Both branches
        // are asserted so this test passes in either environment and still
        // exercises the mapping this crate requires wherever it fails for
        // real, without being gated behind `#[ignore]`.
        let store = store();
        let id = "fallback-test-secret";
        match store.store(id, "s3cr3t") {
            Ok(()) => {
                assert_eq!(store.load(id), Ok(Some("s3cr3t".to_string())));
                assert!(store.has(id));
                assert_eq!(store.delete(id), Ok(()));
                assert_eq!(store.load(id), Ok(None));
            }
            Err(SecretError::Unavailable(message)) => {
                assert_eq!(message, NO_KEYCHAIN_HINT);
                assert!(!store.has(id));
            }
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn loading_a_secret_that_was_never_stored_is_none_not_an_error() {
        match store().load("never-stored-secret") {
            Ok(None) => {}
            Ok(Some(_)) => panic!("nothing was ever stored for this id"),
            Err(SecretError::Unavailable(message)) => assert_eq!(message, NO_KEYCHAIN_HINT),
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn deleting_a_secret_that_was_never_stored_does_not_error() {
        match store().delete("delete-never-stored-secret") {
            Ok(()) => {}
            Err(SecretError::Unavailable(message)) => assert_eq!(message, NO_KEYCHAIN_HINT),
            Err(other) => panic!("expected Unavailable, not {other:?}"),
        }
    }

    #[test]
    fn two_services_do_not_collide_on_the_same_id() {
        let a = SecretStore::new("ide.secret-store-tests-a");
        let b = SecretStore::new("ide.secret-store-tests-b");
        let id = "shared-id";
        // No keychain reachable in this sandbox — nothing to isolate.
        if let (Ok(()), Ok(())) = (a.store(id, "a-secret"), b.store(id, "b-secret")) {
            assert_eq!(a.load(id), Ok(Some("a-secret".to_string())));
            assert_eq!(b.load(id), Ok(Some("b-secret".to_string())));
            let _ = a.delete(id);
            let _ = b.delete(id);
        }
    }
}
