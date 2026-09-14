use tracing::{span, Span};

use crate::action::postgres::psql_query_as_postgres;
use crate::action::{Action, ActionDescription, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::util::sql_escape;

/// Create the database `cardano-db-sync` populates, owned by the db-sync role
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "create_postgres_database")]
pub struct CreatePostgresDatabase {
    name: String,
    owner: String,
}

impl CreatePostgresDatabase {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(credentials: &DatabaseCredentials) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            name: credentials.name.clone(),
            owner: credentials.user.clone(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "create_postgres_database")]
impl Action for CreatePostgresDatabase {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Create the database `{name}` owned by `{owner}`",
            name = self.name,
            owner = self.owner
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "create_postgres_database",
            name = self.name,
            owner = self.owner,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(self.tracing_synopsis(), vec![])]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self { name, owner } = self;

        let exists = psql_query_as_postgres(&format!(
            "SELECT 1 FROM pg_database WHERE datname='{name}'",
            name = sql_escape(name)
        ))
        .await?
            == "1";

        if exists {
            tracing::info!("Database `{name}` already exists");
            return Ok(());
        }

        crate::execute_command(
            crate::command_as("postgres", "createdb")
                .arg("-O")
                .arg(&*owner)
                .arg(&*name),
        )
        .await?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Leave the database `{}` in place", self.name),
            vec![String::from(
                "It holds the synced chain, which would take days to rebuild",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            "Leaving the database `{}` in place; drop it by hand if that is really wanted",
            self.name
        );
        Ok(())
    }
}
