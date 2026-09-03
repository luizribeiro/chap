mod review;

use crate::render::render_consent_error;
use chap_core::{AgentBuilder, PluginId};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub(crate) struct Grants {
    #[command(subcommand)]
    pub(crate) command: GrantsCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum GrantsCommand {
    /// Approve the plugin's exact currently resolved permission manifest.
    Approve {
        /// Plugin instance to approve.
        plugin_id: PluginId,
    },
    /// Remove a plugin's stored approval.
    Deny {
        /// Plugin instance to deny.
        plugin_id: PluginId,
    },
    /// Review resolved permission requests without approving them.
    Review {
        /// Plugin instance to review; omit to review every configured plugin.
        plugin_id: Option<PluginId>,
    },
}

impl GrantsCommand {
    pub(crate) async fn run(self, builder: &AgentBuilder) -> Result<(), String> {
        match self {
            Self::Approve { plugin_id } => {
                builder
                    .approve_plugin(&plugin_id)
                    .await
                    .map_err(render_consent_error)?;
                println!(
                    "Approved `{plugin_id}` for its exact resolved manifest. Concrete scopes remain configured in chap.json."
                );
            }
            Self::Deny { plugin_id } => {
                builder
                    .deny_plugin(&plugin_id)
                    .map_err(render_consent_error)?;
                println!(
                    "Denied `{plugin_id}`. It will require approval before its next admission."
                );
            }
            Self::Review { plugin_id } => {
                print!(
                    "{}",
                    review::grants_review(builder, plugin_id.as_ref()).await?
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{args::Cli, commands::Command};
    use clap::Parser;

    #[test]
    fn parses_grants_review_approve_and_deny_commands() {
        let review = Cli::try_parse_from(["chap", "grants", "review"]).unwrap();
        let approve = Cli::try_parse_from(["chap", "grants", "approve", "openai"]).unwrap();
        let deny = Cli::try_parse_from(["chap", "grants", "deny", "openai"]).unwrap();

        assert!(matches!(
            review.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Review { plugin_id: None }
            }))
        ));
        assert!(matches!(
            approve.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Approve { plugin_id }
            })) if plugin_id == "openai".parse::<PluginId>().unwrap()
        ));
        assert!(matches!(
            deny.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Deny { plugin_id }
            })) if plugin_id == "openai".parse::<PluginId>().unwrap()
        ));
    }
}
