use lockgate::ConsentRecord;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

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
        let mut records = self.records().unwrap_or_default();
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
        let bytes = fs::read(&self.path).ok()?;
        serde_json::from_slice(&bytes).ok()
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
