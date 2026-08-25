use chap_plugin::context::{Context, Segment};
use chap_plugin::{MetadataSource, Needs, Plugin};

mod settings;

use settings::Settings;

const PERSONA_SEGMENT_ID: &str = "persona";

struct Persona {
    settings: Settings,
}

chap_plugin::plugin!(Persona: Context);

impl Plugin for Persona {
    const ID: &'static str = "persona";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Persona");
    const DESCRIPTION: MetadataSource =
        MetadataSource::Explicit("Contributes configured persona instructions");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

impl Context for Persona {
    async fn segments(&self) -> Result<Vec<Segment>, String> {
        Ok(vec![Segment {
            id: PERSONA_SEGMENT_ID.to_owned(),
            content: self.settings.persona.clone(),
            priority: self.settings.priority,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contributes_the_configured_persona_segment() {
        let plugin = <Persona as Plugin>::new(Settings {
            persona: "You are a wizard. Answer in riddles.".to_owned(),
            priority: -4,
        });

        assert_eq!(
            futures::executor::block_on(<Persona as Context>::segments(&plugin)).unwrap(),
            [Segment {
                id: PERSONA_SEGMENT_ID.to_owned(),
                content: "You are a wizard. Answer in riddles.".to_owned(),
                priority: -4,
            }]
        );
    }
}
