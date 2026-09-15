use std::process::ExitCode;

use clap::{ArgAction, Parser};

use crate::{
    cli::{ensure_root, interaction, CommandExecute, APP},
    planner::{with_planner, BuiltinPlanner},
    settings::Secret,
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

        // Before the password is asked for, not after: escalating replaces this process, and
        // would take anything typed at the terminal with it
        ensure_root()?;
        resolve_secrets(&mut planner).await?;

        let next_steps = next_steps(planner.typetag_name());
        with_planner!(planner, |planner| {
            installer::cli::subcommand::install::run(
                &APP,
                planner,
                no_confirm,
                explain,
                &next_steps,
            )
            .await
        })
    }
}

/** Ask for what only a person can supply

The planners are pure: they take a password, they do not go looking for one. Finding it —
from a flag, from the credentials an earlier stage saved, or from whoever is at the terminal
— belongs here.
*/
pub(crate) async fn resolve_secrets(planner: &mut BuiltinPlanner) -> anyhow::Result<()> {
    match planner {
        // Asked once, here, and handed to both the stage which creates the role and the
        // stage which logs in as it — neither goes looking for a file which the same plan
        // has not written yet
        BuiltinPlanner::All(all) if all.validator.postgres_password.is_none() => {
            let saved = all.common().paths().postgres_credentials_file;
            all.validator.postgres_password =
                Some(saved_or_new_password(&saved, &all.validator.database_user).await?);
        },
        BuiltinPlanner::DbSync(db_sync) if db_sync.postgres_password.is_none() => {
            let saved = db_sync.common.paths().postgres_credentials_file;
            db_sync.postgres_password =
                Some(saved_or_new_password(&saved, &db_sync.database_user).await?);
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

/// The password the db-sync stage saved, else a new one asked for at the terminal
///
/// A second run should reuse the password the first one set, or it would reset the role's
/// password to something the running services do not know.
async fn saved_or_new_password(
    saved: &std::path::Path,
    database_user: &str,
) -> anyhow::Result<Secret> {
    if let Ok(credentials) = crate::credentials::DatabaseCredentials::load(saved).await {
        tracing::info!("Using the database password saved in `{}`", saved.display());
        return Ok(credentials.password);
    }

    let password = interaction::prompt_new_secret(&format!(
        "Password for the PostgreSQL role `{database_user}`"
    ))?;
    Ok(Secret::new(password))
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
            "The relay is syncing, which takes a while. Watch it with `midnight-installer status`; the `db-sync` step can be run now and follows along behind it.",
        )],
        "db-sync" => vec![String::from(
            "db-sync is filling the database behind the relay. Watch it with `midnight-installer status`; the `validator` step can be run now and the node restarts until the database has what it reads.",
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
