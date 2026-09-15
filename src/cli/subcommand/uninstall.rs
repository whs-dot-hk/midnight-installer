use std::{path::PathBuf, process::ExitCode};

use clap::{ArgAction, Parser};

use crate::{
    cli::{CommandExecute, APP},
    planner::BuiltinPlanner,
};

/**
Undo a stage, following the receipt it wrote

What this does not undo is deliberate: chain data, the database, validator keys and the
WireGuard identity are left in place. Each action says so in the plan, which is worth reading
before confirming.
*/
#[derive(Debug, Parser)]
pub struct Uninstall {
    /// Run without asking for confirmation
    #[clap(
        long,
        env = "MIDNIGHT_INSTALLER_NO_CONFIRM",
        action(ArgAction::SetTrue),
        global = true
    )]
    pub no_confirm: bool,

    /// Explain each action, not just name it
    #[clap(
        long,
        env = "MIDNIGHT_INSTALLER_EXPLAIN",
        action(ArgAction::SetTrue),
        global = true
    )]
    pub explain: bool,

    /// The receipt to follow, instead of the one for the stage under the data root
    #[clap(long, global = true)]
    pub receipt: Option<PathBuf>,

    /// The stage to undo
    #[clap(subcommand)]
    pub stage: Option<BuiltinPlanner>,
}

#[async_trait::async_trait]
impl CommandExecute for Uninstall {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        let Self {
            no_confirm,
            explain,
            receipt,
            stage,
        } = self;

        let receipt_path = match (receipt, stage) {
            (Some(receipt), _) => receipt,
            (None, Some(stage)) => {
                let path = stage
                    .common_settings()
                    .paths()
                    .receipt(stage.typetag_name());
                // The framework reports a missing receipt too, but it cannot know the one
                // thing an operator here most often has wrong: the stage was installed as
                // part of `all`, whose receipt is the one to follow
                if !path.exists() {
                    anyhow::bail!(
                        "No receipt `{path}`. Nothing was installed from here, it was installed with a different data root, or it was installed as part of `all`, whose receipt is `all.json` and which `uninstall all` follows.",
                        path = path.display()
                    );
                }
                path
            },
            (None, None) => {
                anyhow::bail!("Name the stage to undo, or pass `--receipt` with the path to one")
            },
        };

        installer::cli::subcommand::uninstall::run(&APP, receipt_path, no_confirm, explain).await
    }
}
