//! The Intelephense licence key, remembered after the first keychain read.
//!
//! The keychain can stall (a slow Secret Service), and the key is needed on
//! every project open and settings save. The first read is the only one that
//! touches it; the PHP settings page updates the cache when it writes.

use std::sync::Mutex;

/// `None` inside the mutex: not read yet. `Some(None)`: read, no key.
#[derive(Default)]
pub struct LicenceCache {
    value: Mutex<Option<Option<String>>>,
}

impl LicenceCache {
    pub const fn new() -> Self {
        Self {
            value: Mutex::new(None),
        }
    }

    /// The key, calling `load` only when nothing is remembered. Concurrent
    /// callers wait for one read rather than each making their own.
    pub fn get(&self, load: impl FnOnce() -> Option<String>) -> Option<String> {
        let mut value = self.value.lock().unwrap();
        value.get_or_insert_with(load).clone()
    }

    /// Remember what the keychain now holds (`None` after a removal).
    pub fn set(&self, key: Option<String>) {
        *self.value.lock().unwrap() = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn the_keychain_is_read_once_and_a_missing_key_is_remembered_too() {
        let cache = LicenceCache::new();
        let reads = Cell::new(0);
        let load = || {
            reads.set(reads.get() + 1);
            None
        };
        assert_eq!(cache.get(load), None);
        assert_eq!(cache.get(load), None);
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn a_write_replaces_what_is_remembered_without_another_read() {
        let cache = LicenceCache::new();
        assert_eq!(cache.get(|| Some("old".into())).as_deref(), Some("old"));
        cache.set(Some("new".into()));
        assert_eq!(
            cache.get(|| panic!("must not read the keychain again")),
            Some("new".into())
        );
        cache.set(None);
        assert_eq!(cache.get(|| panic!("removal is remembered")), None);
    }
}
