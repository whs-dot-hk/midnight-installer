use std::process::ExitCode;

use clap::Parser;

use crate::{
    cli::{subcommand::install::resolve_secrets, CommandExecute, APP},
    planner::{with_planner, BuiltinPlanner},
};

/**
Describe what a stage would do

Planning inspects the machine, so it needs the same privileges an install does. It does not
check that the stage before this one has run: the plan is the same either way, and the
actions which need what an earlier stage produces check for it when they run.
*/
#[derive(Debug, Parser)]
pub struct Plan {
    /// Explain each action in the plan, not just name it
    #[clap(long, env = "MIDNIGHT_INSTALLER_EXPLAIN", global = true)]
    pub explain: bool,

    /// Print the plan as JSON, in the form its receipt takes
    #[clap(long, global = true)]
    pub json: bool,

    #[clap(subcommand)]
    pub planner: BuiltinPlanner,
}

#[async_trait::async_trait]
impl CommandExecute for Plan {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        let Self {
            explain,
            json,
            mut planner,
        } = self;

        resolve_secrets(&mut planner).await?;

        with_planner!(planner, |planner| {
            installer::cli::subcommand::plan::run(&APP, planner, explain, json).await
        })
    }
}
