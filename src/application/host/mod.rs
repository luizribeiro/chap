use crate::config::Config;

mod http;
mod settings;

pub(super) const SETTINGS_INTERFACE: &str = "sage:agent/settings@0.1.0";
pub(super) const HTTP_CLIENT_INTERFACE: &str = "sage:agent/http-client@0.1.0";

pub(super) struct AppState {
    settings: settings::Settings,
    http: http::Client,
}

impl AppState {
    pub(super) fn from_config(config: &Config) -> Result<Self, String> {
        Ok(Self {
            settings: settings::Settings::from_config(config)?,
            http: http::Client::new()?,
        })
    }
}
