use chap_plugin::context::{Context, Segment};
use chap_plugin::{MetadataSource, Needs, Plugin};
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    segments: Vec<ConfiguredSegment>,
    #[serde(default)]
    error: bool,
    #[serde(default)]
    hang: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConfiguredSegment {
    id: String,
    content: String,
    priority: i32,
}

struct ContextFixture {
    settings: Settings,
}

impl Plugin for ContextFixture {
    const ID: &'static str = "sdk-context-fixture";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

impl Context for ContextFixture {
    async fn segments(&self) -> Result<Vec<Segment>, String> {
        if self.settings.hang {
            std::thread::sleep(Duration::from_secs(60));
        }
        if self.settings.error {
            return Err("configured context failure".to_owned());
        }
        Ok(self
            .settings
            .segments
            .iter()
            .map(|segment| Segment {
                id: segment.id.clone(),
                content: segment.content.clone(),
                priority: segment.priority,
            })
            .collect())
    }
}

chap_plugin::plugin!(ContextFixture: Context);
