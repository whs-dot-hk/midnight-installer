use std::process::ExitCode;

use clap::Parser;

use crate::{cli::CommandExecute, settings::CommonSettings};

/// Report where this host has got to
#[derive(Debug, Parser)]
pub struct Status {
    #[clap(flatten)]
    pub settings: CommonSettings,
}

#[async_trait::async_trait]
impl CommandExecute for Status {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        print!("{}", crate::status::report(&self.settings).await);
        Ok(ExitCode::SUCCESS)
    }
}
