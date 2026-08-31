mod review;

use crate::render::render_consent_error;
use chap_core::AgentBuilder;
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
        instance_id: String,
    },
    /// Remove a plugin's stored approval.
    Deny {
        /// Plugin instance to deny.
        instance_id: String,
    },
    /// Review resolved permission requests without approving them.
    Review {
        /// Plugin instance to review; omit to review every configured plugin.
        instance_id: Option<String>,
    },
}

impl GrantsCommand {
    pub(crate) async fn run(self, builder: &AgentBuilder) -> Result<(), String> {
        match self {
            Self::Approve { instance_id } => {
                builder
                    .approve_plugin(&instance_id)
                    .await
                    .map_err(render_consent_error)?;
                println!(
                    "Approved `{instance_id}` for its exact resolved manifest. Concrete scopes remain configured in chap.json."
                );
            }
            Self::Deny { instance_id } => {
                builder
                    .deny_plugin(&instance_id)
                    .map_err(render_consent_error)?;
                println!(
                    "Denied `{instance_id}`. It will require approval before its next admission."
                );
            }
            Self::Review { instance_id } => {
                print!(
                    "{}",
                    review::grants_review(builder, instance_id.as_deref()).await?
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
                command: GrantsCommand::Review { instance_id: None }
            }))
        ));
        assert!(matches!(
            approve.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Approve { instance_id }
            })) if instance_id == "openai"
        ));
        assert!(matches!(
            deny.command,
            Some(Command::Grants(Grants {
                command: GrantsCommand::Deny { instance_id }
            })) if instance_id == "openai"
        ));
    }
}
