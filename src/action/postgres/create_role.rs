use tracing::{span, Span};

use crate::action::postgres::{psql_as_postgres, psql_query_as_postgres};
use crate::action::{Action, ActionDescription, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::settings::Secret;
use crate::util::sql_escape;

/** Create (or re-password) the PostgreSQL role db-sync and the node log in as

The role is a superuser because `cardano-db-sync` creates and migrates its own schema.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "create_postgres_role")]
pub struct CreatePostgresRole {
    role: String,
    password: Secret,
}

impl CreatePostgresRole {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(credentials: &DatabaseCredentials) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            role: credentials.user.clone(),
            password: credentials.password.clone(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "create_postgres_role")]
impl Action for CreatePostgresRole {
    fn tracing_synopsis(&self) -> String {
        format!("Create the PostgreSQL role `{}`", self.role)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "create_postgres_role",
            role = self.role
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![String::from(
                "`LOGIN SUPERUSER CREATEDB`, because db-sync creates and migrates its own schema",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self { role, password } = self;

        let exists = psql_query_as_postgres(&format!(
            "SELECT 1 FROM pg_roles WHERE rolname='{role}'",
            role = sql_escape(role)
        ))
        .await?
            == "1";

        let verb = if exists { "ALTER" } else { "CREATE" };
        if exists {
            tracing::info!("PostgreSQL role `{role}` exists, updating its password");
        }

        psql_as_postgres(&format!(
            "{verb} ROLE \"{role}\" WITH LOGIN SUPERUSER CREATEDB PASSWORD '{password}';",
            role = role.replace('"', "\"\""),
            password = sql_escape(password.expose()),
        ))
        .await?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Drop the PostgreSQL role `{}`", self.role),
            vec![String::from(
                "Left alone if it still owns objects, such as the db-sync database",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        let statement = format!(
            "DROP ROLE IF EXISTS \"{role}\";",
            role = self.role.replace('"', "\"\"")
        );

        if let Err(e) = psql_as_postgres(&statement).await {
            // A role which still owns the database cannot be dropped, and taking the
            // database with it is not something a revert should do
            tracing::warn!(
                "Could not drop the PostgreSQL role `{role}`, leaving it: {e}",
                role = self.role
            );
        }

        Ok(())
    }
}
