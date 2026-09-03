use crate::render::render_start_error;
use chap_core::{AgentBuilder, PluginCheck};

pub(super) async fn plugin_check(builder: AgentBuilder) -> Result<(), String> {
    let checks = builder.check_plugins().await.map_err(render_start_error)?;
    print!("{}", render_plugin_checks(&checks));
    Ok(())
}

fn render_plugin_checks(checks: &[PluginCheck]) -> String {
    if checks.is_empty() {
        return super::super::NO_PLUGINS_CONFIGURED.to_owned();
    }
    let mut output = String::new();
    for check in checks {
        output.push_str(&format!("plugin `{}`: OK\n", check.plugin_id));
        for variable in &check.required_environment_variables {
            let presence = if variable.present { "set" } else { "unset" };
            output.push_str(&format!("  requires env {} ({presence})\n", variable.name));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chap_core::RequiredEnvironmentVariable;

    #[test]
    fn renders_short_per_plugin_results_and_environment_requirements() {
        assert_eq!(
            render_plugin_checks(&[
                PluginCheck {
                    plugin_id: "kagi".parse().unwrap(),
                    required_environment_variables: vec![RequiredEnvironmentVariable {
                        name: "KAGI_API_KEY".to_owned(),
                        present: false,
                    }],
                },
                PluginCheck {
                    plugin_id: "persona".parse().unwrap(),
                    required_environment_variables: Vec::new(),
                },
            ]),
            "plugin `kagi`: OK\n  requires env KAGI_API_KEY (unset)\nplugin `persona`: OK\n"
        );
    }
}
