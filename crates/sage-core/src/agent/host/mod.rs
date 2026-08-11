//! Host-owned capabilities exposed to admitted plugins.

use crate::config::Config;

mod settings;

pub(super) use settings::Settings;

pub(super) const SETTINGS_INTERFACE: &str = "sage:agent/settings@0.1.0";

pub(super) struct AppState {
    settings: Settings,
}

impl AppState {
    pub(super) fn from_config(config: &Config) -> Result<Self, String> {
        Ok(Self {
            settings: Settings::from_config(config)?,
        })
    }

    pub(super) fn settings(&self) -> &Settings {
        &self.settings
    }
}
