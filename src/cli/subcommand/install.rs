use std::process::ExitCode;

use clap::{ArgAction, Parser};
use owo_colors::OwoColorize;

use crate::{
    cli::{
        ensure_root,
        interaction::{self, PromptChoice},
        signal_channel, CommandExecute,
    },
    settings::Secret,
    BuiltinPlanner, InstallPlan,
};

/**
Carry out a stage of the FNO build-out

The stage is shown as a plan and confirmed before anything is changed. If a step fails
part way through, the plan that was applied so far is offered for reverting, and either way
a receipt is written so the stage can be undone later.
*/
#[derive(Debug, Parser)]
pub struct Install {
    /// Run without asking for confirmation
    #[clap(
        long,
        env = "MIDNIGHT_INSTALLER_NO_CONFIRM",
        action(ArgAction::SetTrue),
        global = true
    )]
    pub no_confirm: bool,

    /// Explain each action in the plan, not just name it
    #[clap(
        long,
        env = "MIDNIGHT_INSTALLER_EXPLAIN",
        action(ArgAction::SetTrue),
        global = true
    )]
    pub explain: bool,

    #[clap(subcommand)]
    pub planner: BuiltinPlanner,
}

#[async_trait::async_trait]
impl CommandExecute for Install {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        let Self {
            no_confirm,
            explain,
            mut planner,
        } = self;

        ensure_root()?;
        resolve_secrets(&mut planner).await?;

        let stage = planner.typetag_name();
        let mut plan = planner.plan().await?;

        if !no_confirm {
            let mut currently_explaining = explain;
            loop {
                match interaction::prompt(
                    plan.describe_install(currently_explaining).await?,
                    PromptChoice::Yes,
                    currently_explaining,
                )? {
                    PromptChoice::Yes => break,
                    PromptChoice::Explain => currently_explaining = true,
                    PromptChoice::No => {
                        interaction::clean_exit_with_message("Nothing was changed. Bye!")
                    },
                }
            }
        }

        let (tx, rx) = signal_channel()?;

        match plan.install(rx).await {
            Ok(()) => {
                println!("{}", format!("The `{stage}` stage is done.").bold());
                println!("Receipt: {}", plan.receipt_path().display());
                for note in next_steps(stage) {
                    println!("{note}");
                }
                Ok(ExitCode::SUCCESS)
            },
            Err(err) => handle_failure(&mut plan, err, no_confirm, explain, tx).await,
        }
    }
}

/// Offer to undo what was applied, the way the stage would have been undone later
async fn handle_failure(
    plan: &mut InstallPlan,
    err: anyhow::Error,
    no_confirm: bool,
    explain: bool,
    tx: tokio::sync::broadcast::Sender<()>,
) -> anyhow::Result<ExitCode> {
    eprintln!("{}", format!("{err:?}").red());

    if no_confirm {
        return Ok(ExitCode::FAILURE);
    }

    eprintln!("{}", "The stage did not finish; it can be reverted.".red());

    let mut currently_explaining = explain;
    loop {
        match interaction::prompt(
            plan.describe_uninstall(currently_explaining).await?,
            PromptChoice::Yes,
            currently_explaining,
        )? {
            PromptChoice::Yes => break,
            PromptChoice::Explain => currently_explaining = true,
            PromptChoice::No => interaction::clean_exit_with_message(
                "Leaving what was applied in place. Its receipt is written, so `uninstall` can still undo it.",
            ),
        }
    }

    plan.uninstall(tx.subscribe()).await?;
    println!("{}", "What was applied has been reverted.".bold());
    Ok(ExitCode::FAILURE)
}

/** Ask for what only a person can supply

The planners are pure: they take a password, they do not go looking for one. Finding it —
from a flag, from the credentials an earlier stage saved, or from whoever is at the terminal
— belongs here.
*/
pub(crate) async fn resolve_secrets(planner: &mut BuiltinPlanner) -> anyhow::Result<()> {
    match planner {
        BuiltinPlanner::All(all) if all.postgres_password.is_none() => {
            let saved = all.common.paths().postgres_credentials_file;

            if let Ok(credentials) = crate::credentials::DatabaseCredentials::load(&saved).await {
                tracing::info!("Using the database password saved in `{}`", saved.display());
                all.postgres_password = Some(credentials.password);
                return Ok(());
            }

            // Asked once, here, and handed to both the stage which creates the role and the
            // stage which logs in as it — neither goes looking for a file which the same
            // plan has not written yet
            let password = interaction::prompt_new_secret(&format!(
                "Password for the PostgreSQL role `{}`",
                all.database_user
            ))?;
            all.postgres_password = Some(Secret::new(password));
        },
        BuiltinPlanner::DbSync(db_sync) if db_sync.postgres_password.is_none() => {
            let saved = db_sync.common.paths().postgres_credentials_file;

            // A second run of this stage should reuse the password the first one set, or it
            // would reset the role's password to something the running services do not know
            if let Ok(credentials) = crate::credentials::DatabaseCredentials::load(&saved).await {
                tracing::info!("Using the database password saved in `{}`", saved.display());
                db_sync.postgres_password = Some(credentials.password);
                return Ok(());
            }

            let password = interaction::prompt_new_secret(&format!(
                "Password for the PostgreSQL role `{}`",
                db_sync.database_user
            ))?;
            db_sync.postgres_password = Some(Secret::new(password));
        },
        BuiltinPlanner::Validator(validator) if validator.postgres_password.is_none() => {
            let saved = validator.common.paths().postgres_credentials_file;

            if saved.exists() {
                // The planner reads it: leaving it there keeps the password out of this
                // process for as long as possible
                return Ok(());
            }

            let password = interaction::prompt_secret(&format!(
                "PostgreSQL password for `{}`",
                validator.database_user
            ))?;
            validator.postgres_password = Some(Secret::new(password));
        },
        _ => (),
    }

    Ok(())
}

/// What the operator has to do next, which this installer cannot do for them
fn next_steps(stage: &str) -> Vec<String> {
    match stage {
        "all" => vec![
            String::from("Back up the validator keys and the network key offline; they cannot be recovered."),
            String::from("Send the registration file and the WireGuard public key to the Midnight Foundation."),
            String::from("The relay, db-sync and the node are now catching up, in that order, which takes a while. Watch it with `midnight-installer status`; the node restarts until db-sync has the chain it reads."),
        ],
        "cardano" => vec![String::from(
            "The relay is syncing. Watch it with `midnight-installer status`; db-sync cannot start until it reaches 100%.",
        )],
        "midnight" => vec![
            String::from("Back up the validator keys and the network key offline; they cannot be recovered."),
            String::from("Send the registration file to the Midnight Foundation."),
        ],
        "wireguard" => vec![String::from(
            "Send the WireGuard public key to the Midnight Foundation; they return the peer configuration.",
        )],
        "validator" => vec![String::from(
            "The node will not produce blocks until the validator set is activated for it.",
        )],
        _ => vec![],
    }
}
