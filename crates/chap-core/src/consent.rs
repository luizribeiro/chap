use lockgate::{ConsentManifest, ConsentRecord, DriftReport};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConsentReview {
    pub manifest: ConsentManifest,
    pub prior: Option<ConsentRecord>,
    pub drift: Option<DriftReport>,
}

#[derive(Clone, Debug)]
pub struct ConsentStore {
    path: PathBuf,
}

impl ConsentStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, instance_id: &str) -> Option<ConsentRecord> {
        self.records()
            .and_then(|records| records.get(instance_id).cloned())
            .filter(|record| record.instance_id == instance_id)
    }

    pub fn save(&self, record: ConsentRecord) -> Result<(), String> {
        let mut records = match self.read_records() {
            Ok(records) => records.unwrap_or_default(),
            Err(read_error) => {
                self.preserve_unreadable()
                    .map_err(|preserve_error| format!("{read_error}; {preserve_error}"))?;
                BTreeMap::new()
            }
        };
        records.insert(record.instance_id.clone(), record);
        self.write(&records)
    }

    pub fn remove(&self, instance_id: &str) -> Result<(), String> {
        let Some(mut records) = self.records() else {
            return Ok(());
        };
        if records.remove(instance_id).is_none() {
            return Ok(());
        }
        if records.is_empty() {
            return match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "failed to remove consent store `{}`: {error}",
                    self.path.display()
                )),
            };
        }
        self.write(&records)
    }

    fn records(&self) -> Option<BTreeMap<String, ConsentRecord>> {
        self.read_records().ok().flatten()
    }

    fn read_records(&self) -> Result<Option<BTreeMap<String, ConsentRecord>>, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "failed to read consent store `{}`: {error}",
                    self.path.display()
                ));
            }
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            format!(
                "failed to decode consent store `{}`: {error}",
                self.path.display()
            )
        })
    }

    fn preserve_unreadable(&self) -> Result<PathBuf, String> {
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("consent.json");
        let base = self.path.with_file_name(format!("{file_name}.corrupt"));
        let preserved = if base.exists() {
            self.path
                .with_file_name(format!("{file_name}.corrupt.{}", uuid::Uuid::now_v7()))
        } else {
            base
        };
        fs::rename(&self.path, &preserved).map_err(|error| {
            format!(
                "failed to preserve unreadable consent store `{}` as `{}`: {error}",
                self.path.display(),
                preserved.display()
            )
        })?;
        Ok(preserved)
    }

    fn write(&self, records: &BTreeMap<String, ConsentRecord>) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(records).map_err(|error| {
            format!(
                "failed to encode consent store `{}`: {error}",
                self.path.display()
            )
        })?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create consent store directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("consent.json");
        let temporary = self
            .path
            .with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::now_v7()));
        fs::write(&temporary, bytes).map_err(|error| {
            format!(
                "failed to write consent store `{}`: {error}",
                self.path.display()
            )
        })?;
        if let Err(error) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "failed to replace consent store `{}`: {error}",
                self.path.display()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(instance_id: &str, digest_byte: char) -> ConsentRecord {
        serde_json::from_value(serde_json::json!({
            "instance_id": instance_id,
            "fingerprint": format!("sha256:{}", digest_byte.to_string().repeat(64)),
            "grants": [{
                "capability": "net",
                "permission": "egress",
                "scopes": ["https://example.com"],
                "optional": false,
                "reason": "Call the configured service"
            }],
            "approved_at": "2026-08-19T14:30:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn consent_record_round_trips_through_json_storage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let store = ConsentStore::new(&path);
        let record = record("example", '0');

        store.save(record.clone()).unwrap();

        assert_eq!(store.load("example"), Some(record.clone()));
        let persisted: BTreeMap<String, ConsentRecord> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted.get("example"), Some(&record));
        store.remove("example").unwrap();
        assert_eq!(store.load("example"), None);
        assert!(!path.exists());
    }

    #[test]
    fn save_merges_with_other_plugin_records() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConsentStore::new(directory.path().join("consent.json"));
        let first = record("first", '1');
        let second = record("second", '2');

        store.save(first.clone()).unwrap();
        store.save(second.clone()).unwrap();

        assert_eq!(store.load("first"), Some(first));
        assert_eq!(store.load("second"), Some(second));
    }

    #[test]
    fn corrupt_storage_is_preserved_before_saving_a_new_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let corrupt_path = directory.path().join("consent.json.corrupt");
        let store = ConsentStore::new(&path);
        let corrupt = b"{ definitely not valid JSON";
        fs::write(&path, corrupt).unwrap();

        let replacement = record("replacement", '3');
        store.save(replacement.clone()).unwrap();

        assert_eq!(fs::read(corrupt_path).unwrap(), corrupt);
        assert_eq!(store.load("replacement"), Some(replacement));
    }

    #[test]
    fn missing_or_unreadable_storage_has_no_approvals() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let store = ConsentStore::new(&path);

        assert_eq!(store.load("example"), None);
        fs::write(path, b"not JSON").unwrap();
        assert_eq!(store.load("example"), None);
    }
}
