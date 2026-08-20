use crate::{MetadataSource, Needs, SettingsPolicy, schemars, serde};

/// Common authoring contract for every SAGE plugin.
pub trait Plugin: Sized {
    /// Stable plugin identity.
    const ID: &'static str;
    /// Human-facing name shown by hosts.
    const DISPLAY_NAME: MetadataSource = MetadataSource::Cargo;
    /// Plugin version reported to hosts.
    const VERSION: MetadataSource = MetadataSource::Cargo;
    /// Human-facing plugin description.
    const DESCRIPTION: MetadataSource = MetadataSource::Cargo;
    /// Plugin license metadata.
    const LICENSE: MetadataSource = MetadataSource::Cargo;
    /// Plugin source repository metadata.
    const REPOSITORY: MetadataSource = MetadataSource::Cargo;
    /// Plugin homepage metadata.
    const HOMEPAGE: MetadataSource = MetadataSource::Cargo;
    /// Maximum permissions this plugin may request.
    const NEEDS: Needs;
    /// Controls whether unknown top-level settings are accepted.
    const SETTINGS_POLICY: SettingsPolicy = SettingsPolicy::Closed;

    /// Validated settings used to construct one invocation object.
    type Settings: serde::de::DeserializeOwned + schemars::JsonSchema;

    /// Constructs fresh plugin state for one role invocation.
    fn new(settings: Self::Settings) -> Self;
}
