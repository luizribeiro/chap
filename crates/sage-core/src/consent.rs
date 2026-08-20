use lockgate::{ConsentManifest, ConsentRecord, DriftChange, DriftKind, DriftReport, GrantReview};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConsentReview {
    pub manifest: ConsentManifest,
    pub prior: Option<ConsentRecord>,
    pub drift: Option<DriftReport>,
}

pub(crate) fn consent_drift(before: &[GrantReview], after: &[GrantReview]) -> DriftReport {
    let before = indexed(before);
    let after = indexed(after);
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let changes = keys
        .into_iter()
        .filter_map(|(capability, permission)| {
            let before = before
                .get(&(capability.clone(), permission.clone()))
                .copied();
            let after = after
                .get(&(capability.clone(), permission.clone()))
                .copied();
            let kind = match (before, after) {
                (None, Some(_)) => DriftKind::NewGrant,
                (Some(_), None) => DriftKind::RemovedGrant,
                (Some(before), Some(after)) if before.optional && !after.optional => {
                    DriftKind::BecameRequired
                }
                (Some(before), Some(after)) => match scope_change(before, after) {
                    Some(DriftKind::ScopeWidened) => DriftKind::ScopeWidened,
                    _ if before.optional != after.optional => DriftKind::BecameOptional,
                    Some(DriftKind::ScopeNarrowed) => DriftKind::ScopeNarrowed,
                    None => return None,
                    Some(_) => unreachable!("scope changes only have scope drift kinds"),
                },
                (None, None) => return None,
            };
            Some(DriftChange {
                capability,
                permission,
                kind,
                before: before.map(|grant| grant.scopes.clone()),
                after: after.map(|grant| grant.scopes.clone()),
            })
        })
        .collect::<Vec<_>>();
    let blocks_admission = changes.iter().any(|change| {
        matches!(
            change.kind,
            DriftKind::NewGrant | DriftKind::ScopeWidened | DriftKind::BecameRequired
        )
    });
    DriftReport {
        changes,
        blocks_admission,
    }
}

fn indexed(grants: &[GrantReview]) -> BTreeMap<(String, String), &GrantReview> {
    grants
        .iter()
        .map(|grant| ((grant.capability.clone(), grant.permission.clone()), grant))
        .collect()
}

fn scope_change(before: &GrantReview, after: &GrantReview) -> Option<DriftKind> {
    let before = before.scopes.iter().collect::<BTreeSet<_>>();
    let after = after.scopes.iter().collect::<BTreeSet<_>>();
    if before == after {
        None
    } else if after.difference(&before).next().is_some() {
        Some(DriftKind::ScopeWidened)
    } else {
        Some(DriftKind::ScopeNarrowed)
    }
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

    #[test]
    fn consent_record_round_trips_through_json_storage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let store = ConsentStore::new(&path);
        let record: ConsentRecord = serde_json::from_value(serde_json::json!({
            "instance_id": "example",
            "fingerprint": format!("sha256:{}", "0".repeat(64)),
            "grants": [{
                "capability": "http",
                "permission": "egress",
                "scopes": ["https://example.com"],
                "optional": false,
                "reason": "Call the configured service"
            }],
            "approved_at": "2026-08-19T14:30:00Z"
        }))
        .unwrap();

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
    fn missing_or_unreadable_storage_has_no_approvals() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("consent.json");
        let store = ConsentStore::new(&path);

        assert_eq!(store.load("example"), None);
        fs::write(path, b"not JSON").unwrap();
        assert_eq!(store.load("example"), None);
    }
}
