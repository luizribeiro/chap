use serde::Deserialize;
use std::{
    borrow::ToOwned,
    collections::HashMap,
    string::String,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    vec::Vec,
};

const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StateSettings {
    /// Maximum key and value bytes stored by one plugin.
    pub max_bytes: u64,
}

impl Default for StateSettings {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// A per-instance byte quota (the carried limit) was exceeded.
    QuotaExceeded(u64),
    /// The key was empty (v0's only key rule).
    InvalidKey,
}

#[derive(Clone)]
pub struct StateStore {
    inner: Arc<Mutex<StoreData>>,
    max_bytes: u64,
}

#[derive(Default)]
struct StoreData {
    data_by_plugin: HashMap<String, PluginData>,
}

#[derive(Default)]
struct PluginData {
    entries: HashMap<String, Vec<u8>>,
    bytes: u64,
}

impl StateStore {
    pub fn new(settings: StateSettings) -> Self {
        Self {
            inner: Arc::new(Mutex::new(StoreData::default())),
            max_bytes: settings.max_bytes,
        }
    }

    pub fn get(&self, plugin_id: &str, key: &str) -> Result<Option<Vec<u8>>, StateError> {
        validate_key(key)?;

        let store = self.lock();
        Ok(store
            .data_by_plugin
            .get(plugin_id)
            .and_then(|plugin_data| plugin_data.entries.get(key))
            .cloned())
    }

    pub fn set(&self, plugin_id: &str, key: &str, value: Vec<u8>) -> Result<(), StateError> {
        validate_key(key)?;

        let quota_error = || StateError::QuotaExceeded(self.max_bytes);
        let new_entry_bytes = entry_bytes(key, &value);

        let mut store = self.lock();
        let (current_bytes, previous_bytes) = store
            .data_by_plugin
            .get(plugin_id)
            .map(|plugin_data| {
                (
                    plugin_data.bytes,
                    plugin_data
                        .entries
                        .get(key)
                        .map_or(0, |previous_value| entry_bytes(key, previous_value)),
                )
            })
            .unwrap_or_default();
        let next_bytes = current_bytes
            .saturating_sub(previous_bytes)
            .checked_add(new_entry_bytes)
            .filter(|bytes| *bytes <= self.max_bytes)
            .ok_or_else(quota_error)?;

        let plugin_data = store
            .data_by_plugin
            .entry(plugin_id.to_owned())
            .or_default();
        plugin_data.bytes = next_bytes;
        plugin_data.entries.insert(key.to_owned(), value);
        Ok(())
    }

    pub fn delete(&self, plugin_id: &str, key: &str) -> Result<(), StateError> {
        validate_key(key)?;

        let mut store = self.lock();
        let Some(plugin_data) = store.data_by_plugin.get_mut(plugin_id) else {
            return Ok(());
        };
        let Some(value) = plugin_data.entries.remove(key) else {
            return Ok(());
        };
        plugin_data.bytes = plugin_data.bytes.saturating_sub(entry_bytes(key, &value));
        let plugin_is_empty = plugin_data.entries.is_empty();
        if plugin_is_empty {
            store.data_by_plugin.remove(plugin_id);
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, StoreData> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn entry_bytes(key: &str, value: &[u8]) -> u64 {
    u64::try_from(key.len() + value.len()).unwrap_or(u64::MAX)
}

fn validate_key(key: &str) -> Result<(), StateError> {
    if key.is_empty() {
        Err(StateError::InvalidKey)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::vec;

    use super::{StateError, StateSettings, StateStore};

    fn store_with_quota(max_bytes: u64) -> StateStore {
        StateStore::new(StateSettings { max_bytes })
    }

    #[test]
    fn stores_and_reads_values() {
        let store = StateStore::new(StateSettings::default());

        assert_eq!(store.get("calendar", "cursor"), Ok(None));

        let cursor = br#"{"last_event":"evt_1042"}"#.to_vec();
        store.set("calendar", "cursor", cursor.clone()).unwrap();

        assert_eq!(store.get("calendar", "cursor"), Ok(Some(cursor)));
    }

    #[test]
    fn delete_removes_values_and_ignores_missing_keys() {
        let store = StateStore::new(StateSettings::default());
        store
            .set("mail", "refresh-token", b"token-123".to_vec())
            .unwrap();

        assert_eq!(store.delete("mail", "refresh-token"), Ok(()));
        assert_eq!(store.get("mail", "refresh-token"), Ok(None));
        assert_eq!(store.delete("mail", "refresh-token"), Ok(()));
    }

    #[test]
    fn plugins_are_isolated() {
        let store = StateStore::new(StateSettings::default());
        store
            .set("a", "cursor", b"plugin-a-cursor".to_vec())
            .unwrap();

        assert_eq!(store.get("b", "cursor"), Ok(None));
        assert_eq!(
            store.get("a", "cursor"),
            Ok(Some(b"plugin-a-cursor".to_vec()))
        );
    }

    #[test]
    fn set_uses_last_writer_wins() {
        let store = StateStore::new(StateSettings::default());
        store.set("search", "page", b"first".to_vec()).unwrap();
        store.set("search", "page", b"second".to_vec()).unwrap();

        assert_eq!(store.get("search", "page"), Ok(Some(b"second".to_vec())));
    }

    #[test]
    fn quota_rejects_without_modifying_and_frees_replaced_or_deleted_bytes() {
        let store = store_with_quota(64);
        let original = vec![b'a'; 55];
        store.set("plugin", "cache", original.clone()).unwrap();

        assert_eq!(
            store.set("plugin", "cache", vec![b'b'; 60]),
            Err(StateError::QuotaExceeded(64))
        );
        assert_eq!(store.get("plugin", "cache"), Ok(Some(original)));

        let smaller = vec![b'c'; 40];
        store.set("plugin", "cache", smaller.clone()).unwrap();
        store.set("plugin", "other", vec![b'd'; 14]).unwrap();
        assert_eq!(
            store.set("plugin", "fresh", vec![b'e'; 14]),
            Err(StateError::QuotaExceeded(64))
        );

        store.delete("plugin", "other").unwrap();
        store.set("plugin", "fresh", vec![b'e'; 14]).unwrap();
        assert_eq!(store.get("plugin", "cache"), Ok(Some(smaller)));
        assert_eq!(store.get("plugin", "fresh"), Ok(Some(vec![b'e'; 14])));

        store
            .set("other-plugin", "payload", vec![b'f'; 57])
            .unwrap();
    }

    #[test]
    fn long_keys_consume_quota() {
        let store = store_with_quota(64);
        for key in [
            "cache-entry-001",
            "cache-entry-002",
            "cache-entry-003",
            "cache-entry-004",
        ] {
            assert_eq!(key.len(), 15);
            store.set("plugin", key, vec![b'x']).unwrap();
        }

        assert_eq!(
            store.set("plugin", "cache-entry-005", vec![b'x']),
            Err(StateError::QuotaExceeded(64))
        );
    }

    #[test]
    fn empty_keys_are_rejected() {
        let store = StateStore::new(StateSettings::default());

        assert_eq!(store.get("plugin", ""), Err(StateError::InvalidKey));
        assert_eq!(
            store.set("plugin", "", b"value".to_vec()),
            Err(StateError::InvalidKey)
        );
        assert_eq!(store.delete("plugin", ""), Err(StateError::InvalidKey));
    }
}
