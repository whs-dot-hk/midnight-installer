use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgAction, Parser, Subcommand};

use crate::cli::{interaction, CommandExecute};
use crate::devnet::{config::DevnetConfig, controller};

/**
Run a devnet of your own over SSH: one host or many

The devnet is described in a TOML file: the hosts, how to reach them (`gcloud compute ssh`,
through an IAP tunnel, or plain `ssh`) and the validators and full nodes on each. Nothing is
installed on this machine; it needs only to reach the hosts, and to be able to `sudo` on them
without a password.
*/
#[derive(Debug, Parser)]
pub struct Devnet {
    #[clap(subcommand)]
    pub command: DevnetCommand,
}

#[derive(Debug, Subcommand)]
pub enum DevnetCommand {
    /// Bring the devnet up, or in line with its file; safe to run again
    Up {
        /// The devnet file
        config: PathBuf,

        /// The installer to run on the hosts: a static build of this one. Defaults to this
        /// binary, which only works on the hosts if it is static
        #[clap(long, env = "MIDNIGHT_INSTALLER_DEVNET_BINARY")]
        binary: Option<PathBuf>,
    },
    /// Show every node: its service, its best and finalized block, its peers
    Status {
        /// The devnet file
        config: PathBuf,
    },
    /// Stop every node, and delete the devnet with its keys and chain data from every host
    Down {
        /// The devnet file
        config: PathBuf,

        /// Run without asking for confirmation
        #[clap(
            long,
            env = "MIDNIGHT_INSTALLER_NO_CONFIRM",
            action(ArgAction::SetTrue)
        )]
        no_confirm: bool,
    },
}

#[async_trait::async_trait]
impl CommandExecute for Devnet {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        match self.command {
            DevnetCommand::Up { config, binary } => {
                let config = DevnetConfig::load(&config)?;
                let binary = match binary {
                    Some(binary) => binary,
                    None => std::env::current_exe()?,
                };
                controller::up(&config, binary).await?;
            },
            DevnetCommand::Status { config } => {
                controller::status(&DevnetConfig::load(&config)?).await?;
            },
            DevnetCommand::Down { config, no_confirm } => {
                let config = DevnetConfig::load(&config)?;
                if !no_confirm {
                    let hosts = config
                        .hosts
                        .iter()
                        .map(|h| h.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let question = format!(
                        "Stop devnet `{}` and delete its keys and chain data from {hosts}.",
                        config.chain
                    );
                    if interaction::prompt(question, interaction::PromptChoice::No, true)?
                        != interaction::PromptChoice::Yes
                    {
                        return Ok(ExitCode::FAILURE);
                    }
                }
                controller::down(&config).await?;
            },
        }
        Ok(ExitCode::SUCCESS)
    }
}
