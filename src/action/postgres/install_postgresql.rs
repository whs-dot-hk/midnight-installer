use anyhow::Context;

use tracing::{span, Span};
use url::Url;

use crate::action::base::{package_installed, AptInstall, ConfigureAptRepository};
use crate::action::{planned, Action, ActionDescription, ActionState, StatefulAction};
use crate::settings::CommonSettings;

const PGDG_KEY_URL: &str = "https://www.postgresql.org/media/keys/ACCC4CF8.asc";
const PGDG_KEY_PATH: &str = "/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc";
const PGDG_LIST_PATH: &str = "/etc/apt/sources.list.d/pgdg.list";

/** Install PostgreSQL from the PostgreSQL project's own APT repository

The FNO programme pins a PostgreSQL major version which is usually newer than the one the
distribution ships, so the packages come from PGDG.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_postgresql")]
pub struct InstallPostgresql {
    version: String,
    repository: StatefulAction<ConfigureAptRepository>,
    packages: StatefulAction<AptInstall>,
}

impl InstallPostgresql {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let version = settings.postgres_version.clone();
        let codename = crate::util::os_release_codename().await?;

        let key_url = Url::parse(PGDG_KEY_URL).context("Parsing the PGDG key URL")?;

        let repository = ConfigureAptRepository::plan(
            "PostgreSQL (PGDG)",
            key_url,
            PGDG_KEY_PATH,
            PGDG_LIST_PATH,
            format!("https://apt.postgresql.org/pub/repos/apt {codename}-pgdg main"),
        )
        .await?;

        let packages = AptInstall::plan([
            format!("postgresql-{version}"),
            format!("postgresql-server-dev-{version}"),
        ])
        .await?;

        let state = if package_installed(&format!("postgresql-{version}")).await {
            tracing::debug!("PostgreSQL {version} is already installed");
            ActionState::Skipped
        } else {
            ActionState::Uncompleted
        };

        Ok(planned(
            Self {
                version,
                repository,
                packages,
            },
            state,
        ))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_postgresql")]
impl Action for InstallPostgresql {
    fn tracing_synopsis(&self) -> String {
        format!("Install PostgreSQL {}", self.version)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_postgresql",
            version = self.version,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let explanation = [
            self.repository.describe_execute(),
            self.packages.describe_execute(),
        ]
        .concat()
        .into_iter()
        .map(|description| description.description)
        .collect();

        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        self.repository.try_execute().await?;
        self.packages.try_execute().await?;
        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Leave PostgreSQL {} installed", self.version),
            vec![String::from(
                "Removing a database server, and with it the cluster it manages, is never a safe way to undo a failed step",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::info!("Leaving PostgreSQL {} installed", self.version);
        Ok(())
    }
}
