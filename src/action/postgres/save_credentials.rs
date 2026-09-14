use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::util::pgpass_escape;

/** Keep one canonical, root-only copy of the database credentials

The db-sync stage asks for the password; the validator stage needs the same one. Rather than
prompting twice (and risking two different answers), it is saved here, mode `600`, and read
back later.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "save_database_credentials")]
pub struct SaveDatabaseCredentials {
    path: PathBuf,
    credentials: DatabaseCredentials,
}

impl SaveDatabaseCredentials {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        path: impl Into<PathBuf>,
        credentials: &DatabaseCredentials,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            path: path.into(),
            credentials: credentials.clone(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "save_database_credentials")]
impl Action for SaveDatabaseCredentials {
    fn tracing_synopsis(&self) -> String {
        format!("Save the database credentials to `{}`", self.path.display())
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "save_database_credentials",
            path = tracing::field::display(self.path.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![String::from(
                "Owned by `root`, mode `600`: it holds the PostgreSQL password the validator stage reuses",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        crate::util::write_owned_file(
            &self.path,
            &self.credentials.render_env_file(),
            0o600,
            Some("root"),
        )
        .await?;
        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Delete `{}`", self.path.display()),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        crate::util::remove_file_if_exists(&self.path)
            .await
            .with_context(|| format!("Removing `{}`", self.path.display()))?;
        Ok(())
    }
}

/// Write the service user's `~/.pgpass`, which is how `cardano-db-sync` authenticates
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "create_pgpass_file")]
pub struct CreatePgpassFile {
    path: PathBuf,
    user: String,
    host_entry: String,
    credentials: DatabaseCredentials,
}

impl CreatePgpassFile {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        user: impl Into<String>,
        credentials: &DatabaseCredentials,
    ) -> anyhow::Result<StatefulAction<Self>> {
        let user = user.into();
        let home = crate::settings::user_home(&user)?;

        Ok(StatefulAction::uncompleted(Self {
            path: home.join(".pgpass"),
            user,
            // db-sync connects over the Unix socket, so the `.pgpass` entry has to name the
            // socket directory, not `localhost`
            host_entry: String::from("/var/run/postgresql"),
            credentials: credentials.clone(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "create_pgpass_file")]
impl Action for CreatePgpassFile {
    fn tracing_synopsis(&self) -> String {
        format!("Write `{}`", self.path.display())
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "create_pgpass_file",
            path = tracing::field::display(self.path.display()),
            user = self.user,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![format!(
                "Owned by `{user}`, mode `600`: how `cardano-db-sync` authenticates over the socket",
                user = self.user
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            path,
            user,
            host_entry,
            credentials,
        } = self;

        let line = format!(
            "{host}:{port}:{name}:{user_field}:{password}\n",
            host = host_entry,
            port = credentials.port,
            name = pgpass_escape(&credentials.name),
            user_field = pgpass_escape(&credentials.user),
            password = pgpass_escape(credentials.password.expose()),
        );

        crate::util::write_owned_file(path, &line, 0o600, Some(user)).await?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Delete `{}`", self.path.display()),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        crate::util::remove_file_if_exists(&self.path)
            .await
            .with_context(|| format!("Removing `{}`", self.path.display()))?;
        Ok(())
    }
}
