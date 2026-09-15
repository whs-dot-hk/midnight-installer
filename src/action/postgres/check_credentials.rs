use anyhow::Context;

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};
use crate::credentials::DatabaseCredentials;

/** Check, at execute time, that these credentials open the database

The validator's environment file carries the password, and a node started with the wrong one
fails to authenticate every ten seconds for ever, while the install which wrote it reports
success. Checked here, before the file is written, a wrong `--postgres-password` (or a
PostgreSQL which is not running) stops the stage with the reason instead.

Within one whole-host plan the role and database were created a few actions earlier, so
this always passes there; it earns its keep when the validator stage is run on its own.

It changes nothing, so reverting it does nothing.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "check_database_credentials")]
pub struct CheckDatabaseCredentials {
    credentials: DatabaseCredentials,
}

impl CheckDatabaseCredentials {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(credentials: &DatabaseCredentials) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            credentials: credentials.clone(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "check_database_credentials")]
impl Action for CheckDatabaseCredentials {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Check that `{user}` can open the database `{db}`",
            user = self.credentials.user,
            db = self.credentials.name,
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "check_database_credentials",
            user = self.credentials.user,
            database = self.credentials.name,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![format!(
                "Log in to PostgreSQL at `{host}:{port}` with the password the node will be given",
                host = self.credentials.host,
                port = self.credentials.port,
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let answer = crate::check::psql_query(&self.credentials, "SELECT 1;")
            .await
            .with_context(|| format!(
                "The database `{db}` could not be opened as `{user}` with the password given. Check `--postgres-password` against the one the `db-sync` step saved, and that PostgreSQL is running.",
                db = self.credentials.name,
                user = self.credentials.user,
            ))?;

        if answer != "1" {
            anyhow::bail!(
                "PostgreSQL answered `{answer}` to `SELECT 1`, which is not the `1` a working connection returns"
            );
        }

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
