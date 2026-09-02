use crate::config::LoadError;
use lockgate::{ConsentManifest, ConsentRecord, DriftReport, PluginId};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConsentError {
    #[error("{0}")]
    StateLocation(#[source] LoadError),
    #[error("plugin `{plugin_id}` is not configured")]
    PluginNotConfigured { plugin_id: PluginId },
    #[error("failed to read plugin `{plugin_id}` from `{}`: {source}", path.display())]
    ReadPlugin {
        plugin_id: PluginId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to inspect plugin `{plugin_id}`: {source}")]
    InspectPlugin {
        plugin_id: PluginId,
        #[source]
        source: Box<lockgate::InspectError>,
    },
    #[error("failed to load plugin `{plugin_id}` from `{}`: {source}", path.display())]
    LoadPlugin {
        plugin_id: PluginId,
        path: PathBuf,
        #[source]
        source: Box<lockgate::AdmissionError>,
    },
    #[error(
        "plugin `{plugin_id}` from `{}` does not implement a supported role from `{role}`",
        path.display()
    )]
    UnsupportedRole {
        plugin_id: PluginId,
        path: PathBuf,
        role: String,
        exported_interfaces: Vec<String>,
    },
    #[error(
        "plugin `{plugin_id}` configures a `{role}` section, but its component does not export the {role} interface"
    )]
    RoleConfigInvalid { plugin_id: PluginId, role: String },
    #[error("failed to determine current working directory: {source}")]
    CurrentDirectoryUnavailable {
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    HostConfiguration(#[source] LoadError),
    #[cfg(feature = "vm")]
    #[error("failed to {operation}: {source}")]
    VmLifecycle {
        operation: &'static str,
        #[source]
        source: chap_vm::host::VmError,
    },
    #[error("failed to create Lockgate host: {source}")]
    HostConstruction {
        #[source]
        source: lockgate::HostConstructionError,
    },
    #[error("failed to configure a plugin call budget: {source}")]
    InvalidCallBudget {
        #[source]
        source: lockgate::InvalidCallBudget,
    },
    #[error("failed to register a host capability: {source}")]
    CapabilityRegistration {
        #[source]
        source: lockgate::CapabilityRegistrationError,
    },
    #[error("failed to clean up Lockgate host: {source}")]
    HostCleanup {
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("failed to clean up prepared plugin `{plugin_id}`: {source}")]
    PreparedPluginCleanup {
        plugin_id: PluginId,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("{source}; {cleanup}")]
    OperationAndCleanup {
        #[source]
        source: Box<ConsentError>,
        cleanup: Box<ConsentError>,
    },
    #[error("failed to remove consent store `{}`: {source}", path.display())]
    RemoveStore {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read consent store `{}`: {source}", path.display())]
    ReadStore {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to decode consent store `{}`: {source}", path.display())]
    DecodeStore {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "failed to preserve unreadable consent store `{}` as `{}`: {source}",
        path.display(),
        preserved.display()
    )]
    PreserveUnreadableStore {
        path: PathBuf,
        preserved: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to encode consent store `{}`: {source}", path.display())]
    EncodeStore {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "failed to create consent store directory `{}`: {source}",
        path.display()
    )]
    CreateStoreDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write consent store `{}`: {source}", path.display())]
    WriteStore {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to replace consent store `{}`: {source}", path.display())]
    ReplaceStore {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write config path breadcrumb `{}`: {source}", path.display())]
    WriteConfigPathBreadcrumb {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConsentReview {
    pub manifest: ConsentManifest,
    pub prior: Option<ConsentRecord>,
    pub drift: Option<DriftReport>,
}

#[derive(Clone, Debug)]
pub struct ConsentStore {
    path: PathBuf,
    config_path: PathBuf,
}

impl ConsentStore {
    pub fn new(path: impl Into<PathBuf>, config_path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            config_path: config_path.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, plugin_id: &PluginId) -> Option<ConsentRecord> {
        self.records()
            .and_then(|records| records.get(plugin_id).cloned())
            .filter(|record| record.plugin_id == *plugin_id)
    }

    pub fn save(&self, record: ConsentRecord) -> Result<(), ConsentError> {
        let mut records = match self.read_records() {
            Ok(records) => records.unwrap_or_default(),
            Err(read_error) => {
                if let Err(preserve_error) = self.preserve_unreadable() {
                    return Err(ConsentError::OperationAndCleanup {
                        source: Box::new(read_error),
                        cleanup: Box::new(preserve_error),
                    });
                }
                BTreeMap::new()
            }
        };
        records.insert(record.plugin_id.clone(), record);
        self.write(&records)
    }

    pub fn remove(&self, plugin_id: &PluginId) -> Result<(), ConsentError> {
        let Some(mut records) = self.records() else {
            return Ok(());
        };
        if records.remove(plugin_id).is_none() {
            return Ok(());
        }
        if records.is_empty() {
            return match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(ConsentError::RemoveStore {
                    path: self.path.clone(),
                    source,
                }),
            };
        }
        self.write(&records)
    }

    fn records(&self) -> Option<BTreeMap<PluginId, ConsentRecord>> {
        self.read_records().ok().flatten()
    }

    fn read_records(&self) -> Result<Option<BTreeMap<PluginId, ConsentRecord>>, ConsentError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConsentError::ReadStore {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| ConsentError::DecodeStore {
                path: self.path.clone(),
                source,
            })
    }

    fn preserve_unreadable(&self) -> Result<PathBuf, ConsentError> {
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
        fs::rename(&self.path, &preserved).map_err(|source| {
            ConsentError::PreserveUnreadableStore {
                path: self.path.clone(),
                preserved: preserved.clone(),
                source,
            }
        })?;
        Ok(preserved)
    }

    fn write(&self, records: &BTreeMap<PluginId, ConsentRecord>) -> Result<(), ConsentError> {
        let bytes =
            serde_json::to_vec_pretty(records).map_err(|source| ConsentError::EncodeStore {
                path: self.path.clone(),
                source,
            })?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| ConsentError::CreateStoreDirectory {
                path: parent.to_path_buf(),
                source,
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
        fs::write(&temporary, bytes).map_err(|source| ConsentError::WriteStore {
            path: self.path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(ConsentError::ReplaceStore {
                path: self.path.clone(),
                source,
            });
        }
        let breadcrumb = self.path.with_file_name("config-path");
        fs::write(&breadcrumb, self.config_path.as_os_str().as_encoded_bytes()).map_err(
            |source| ConsentError::WriteConfigPathBreadcrumb {
                path: breadcrumb,
                source,
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(directory: &Path) -> ConsentStore {
        ConsentStore::new(directory.join("consent.json"), directory.join("chap.json"))
    }

    fn record(plugin_id: &str, digest_byte: char) -> ConsentRecord {
        serde_json::from_value(serde_json::json!({
            "instance_id": plugin_id,
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

    fn plugin_id(id: &str) -> PluginId {
        PluginId::from(id)
    }

    #[test]
    fn consent_record_round_trips_through_json_storage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let store = store(directory.path());
        let record = record("example", '0');

        store.save(record.clone()).unwrap();

        assert_eq!(store.load(&plugin_id("example")), Some(record.clone()));
        let persisted: BTreeMap<String, ConsentRecord> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted.get("example"), Some(&record));
        store.remove(&plugin_id("example")).unwrap();
        assert_eq!(store.load(&plugin_id("example")), None);
        assert!(!path.exists());
    }

    #[test]
    fn save_merges_with_other_plugin_records() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        let first = record("first", '1');
        let second = record("second", '2');

        store.save(first.clone()).unwrap();
        store.save(second.clone()).unwrap();

        assert_eq!(store.load(&plugin_id("first")), Some(first));
        assert_eq!(store.load(&plugin_id("second")), Some(second));
    }

    #[test]
    fn corrupt_storage_is_preserved_before_saving_a_new_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let corrupt_path = directory.path().join("consent.json.corrupt");
        let store = store(directory.path());
        let corrupt = b"{ definitely not valid JSON";
        fs::write(&path, corrupt).unwrap();

        let replacement = record("replacement", '3');
        store.save(replacement.clone()).unwrap();

        assert_eq!(fs::read(corrupt_path).unwrap(), corrupt);
        assert_eq!(store.load(&plugin_id("replacement")), Some(replacement));
    }

    #[test]
    fn missing_or_unreadable_storage_has_no_approvals() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let store = store(directory.path());

        assert_eq!(store.load(&plugin_id("example")), None);
        fs::write(path, b"not JSON").unwrap();
        assert_eq!(store.load(&plugin_id("example")), None);
    }

    #[test]
    fn save_writes_the_config_path_breadcrumb() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("chap.json");
        fs::write(directory.path().join("config-path"), "stale").unwrap();
        let store = store(directory.path());

        store.save(record("example", '4')).unwrap();

        assert_eq!(
            fs::read(directory.path().join("config-path")).unwrap(),
            config_path.as_os_str().as_encoded_bytes()
        );
    }
}
