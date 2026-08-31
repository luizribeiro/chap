pub(crate) mod grants;
pub(crate) mod plugins;

use chap_core::AgentBuilder;
use clap::Subcommand;

pub(crate) const NO_PLUGINS_CONFIGURED: &str = "No plugins are configured.\n";

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Review and manage plugin permission grants.
    Grants(grants::Grants),
    /// Inspect configured plugins.
    Plugins(plugins::Plugins),
}

impl Command {
    pub(crate) async fn run(self, builder: AgentBuilder) -> Result<(), String> {
        match self {
            Self::Grants(grants) => grants.command.run(&builder).await,
            Self::Plugins(plugins) => plugins.command.run(builder).await,
        }
    }
}
