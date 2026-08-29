use chap_plugin::tools::{ToolDefinition, Tools};
use chap_plugin::{MetadataSource, Needs, Plugin};
use schemars::JsonSchema;
use serde::Deserialize;

const GREET: &str = "greet";

struct MyPlugin {
    settings: Settings,
}

impl Plugin for MyPlugin {
    const ID: &'static str = "my-plugin";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("My plugin");
    const DESCRIPTION: MetadataSource = MetadataSource::Explicit("Greets a person by name");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = Settings;

    fn new(settings: Self::Settings) -> Self {
        Self { settings }
    }
}

impl Tools for MyPlugin {
    fn definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![ToolDefinition {
            name: GREET.to_owned(),
            description: "Greet a person by name.".to_owned(),
            parameters: r#"{
                "type":"object",
                "properties":{
                    "name":{"type":"string","description":"The name to greet."}
                },
                "required":["name"],
                "additionalProperties":false
            }"#
            .to_owned(),
        }])
    }

    async fn execute(&self, name: String, arguments: String) -> Result<String, String> {
        if name != GREET {
            return Err(format!("tool `{name}` is not provided by this plugin"));
        }
        let arguments: GreetArguments = serde_json::from_str(&arguments)
            .map_err(|error| format!("invalid `greet` arguments: {error}"))?;
        let greeting = self.settings.greeting.as_deref().unwrap_or("Hello");
        Ok(format!("{greeting}, {}!", arguments.name))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// Greeting placed before the person's name.
    greeting: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GreetArguments {
    name: String,
}

chap_plugin::plugin!(MyPlugin: Tools);

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(greeting: Option<&str>) -> MyPlugin {
        <MyPlugin as Plugin>::new(Settings {
            greeting: greeting.map(str::to_owned),
        })
    }

    #[test]
    fn greets_with_the_default_greeting() {
        let result = futures::executor::block_on(
            plugin(None).execute(GREET.to_owned(), r#"{"name":"Ada"}"#.to_owned()),
        );

        assert_eq!(result.unwrap(), "Hello, Ada!");
    }

    #[test]
    fn honors_a_configured_greeting() {
        let result = futures::executor::block_on(
            plugin(Some("Howdy")).execute(GREET.to_owned(), r#"{"name":"Ada"}"#.to_owned()),
        );

        assert_eq!(result.unwrap(), "Howdy, Ada!");
    }

    #[test]
    fn rejects_an_unknown_tool() {
        let result = futures::executor::block_on(
            plugin(None).execute("other".to_owned(), "{}".to_owned()),
        );

        assert_eq!(
            result.unwrap_err(),
            "tool `other` is not provided by this plugin"
        );
    }
}
