use std::process::ExitCode;

use clap::Parser;

use crate::{
    cli::{ensure_root, CommandExecute},
    settings::CommonSettings,
};

/// Report where this host has got to
///
/// The saved database credentials are root-only and the relay is queried as its service
/// user, so this escalates with `sudo` like the other subcommands.
#[derive(Debug, Parser)]
pub struct Status {
    #[clap(flatten)]
    pub settings: CommonSettings,
}

#[async_trait::async_trait]
impl CommandExecute for Status {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        ensure_root()?;
        print!("{}", crate::status::report(&self.settings).await);
        Ok(ExitCode::SUCCESS)
    }
}
