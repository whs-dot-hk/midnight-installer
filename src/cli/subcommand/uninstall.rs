use std::{path::PathBuf, process::ExitCode};

use anyhow::Context;
use clap::{ArgAction, Parser};
use owo_colors::OwoColorize;

use crate::{
    cli::{
        ensure_root,
        interaction::{self, PromptChoice},
        signal_channel, CommandExecute,
    },
    BuiltinPlanner, InstallPlan,
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

        ensure_root()?;

        let receipt_path = match (receipt, stage) {
            (Some(receipt), _) => receipt,
            (None, Some(stage)) => stage
                .common_settings()
                .paths()
                .receipt(stage.typetag_name()),
            (None, None) => {
                anyhow::bail!("Name the stage to undo, or pass `--receipt` with the path to one")
            },
        };

        let contents = tokio::fs::read_to_string(&receipt_path)
            .await
            .with_context(|| {
                format!(
                    "Reading the receipt `{path}`. Nothing was installed from here, it was installed with a different data root, or it was installed as part of `all`, whose receipt is `all.json` and which `uninstall all` follows.",
                    path = receipt_path.display()
                )
            })?;
        let mut plan: InstallPlan = serde_json::from_str(&contents).with_context(|| {
            format!(
                "Parsing the receipt `{path}`; it may have been written by an incompatible version of this installer",
                path = receipt_path.display()
            )
        })?;

        if !no_confirm {
            let mut currently_explaining = explain;
            loop {
                match interaction::prompt(
                    plan.describe_uninstall(currently_explaining).await?,
                    PromptChoice::No,
                    currently_explaining,
                )? {
                    PromptChoice::Yes => break,
                    PromptChoice::Explain => currently_explaining = true,
                    PromptChoice::No => {
                        interaction::clean_exit_with_message("Nothing was undone. Bye!")
                    },
                }
            }
        }

        let (_tx, rx) = signal_channel()?;
        plan.uninstall(rx).await?;

        println!("{}", "Undone.".bold());
        Ok(ExitCode::SUCCESS)
    }
}
