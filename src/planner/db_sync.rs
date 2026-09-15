use anyhow::Context;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::action::base::{CreateDirectory, CreateSystemdUnit, RequirePaths};
use crate::action::dbsync::InstallCardanoDbSync;
use crate::action::postgres::{
    CreatePgpassFile, CreatePostgresDatabase, CreatePostgresRole, InstallPostgresql,
    RelocatePostgresCluster, SaveDatabaseCredentials,
};
use crate::action::{Action, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::planner::{
    cardano::{base_packages, installed_version_matches, start_or_restart, unit_differs},
    diff_from_default, platform_check, require_root, require_user, units, Planner,
};
use crate::settings::{
    CommonSettings, Secret, CARDANO_DB_SYNC_SERVICE, CARDANO_NODE_SERVICE, DEFAULT_DB_NAME,
    DEFAULT_DB_USER,
};

/** PostgreSQL and `cardano-db-sync`

db-sync follows the relay rather than requiring it to have finished: started against a relay
which is still catching up, it simply follows along behind it. So this stage installs
whenever it is asked to, and how far along the pair have got is a question for `status`.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct DbSync {
    #[clap(flatten)]
    pub common: CommonSettings,

    /// The password for the PostgreSQL role
    ///
    /// Prompted for if it is not given. It is saved, root-only, so the validator stage does
    /// not have to ask for it a second time.
    #[clap(long, env = "MIDNIGHT_INSTALLER_POSTGRES_PASSWORD")]
    pub postgres_password: Option<Secret>,

    /// The PostgreSQL role db-sync and the node log in as
    #[clap(long, default_value = DEFAULT_DB_USER, env = "MIDNIGHT_INSTALLER_DATABASE_USER")]
    pub database_user: String,

    /// The database db-sync populates
    #[clap(long, default_value = DEFAULT_DB_NAME, env = "MIDNIGHT_INSTALLER_DATABASE_NAME")]
    pub database_name: String,
}

impl DbSync {
    pub fn credentials(&self) -> anyhow::Result<DatabaseCredentials> {
        let password = self.postgres_password.clone().context("No PostgreSQL password was given. Pass `--postgres-password`, set `MIDNIGHT_INSTALLER_POSTGRES_PASSWORD`, or run this from a terminal so it can be asked for.")?;

        Ok(DatabaseCredentials::new(
            &self.database_user,
            &self.database_name,
            password,
        ))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "db-sync")]
impl Planner for DbSync {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            common: CommonSettings::default(),
            postgres_password: None,
            database_user: DEFAULT_DB_USER.into(),
            database_name: DEFAULT_DB_NAME.into(),
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let paths = self.common.paths();
        let user = self.common.cardano_user.clone();
        let credentials = self.credentials()?;
        let bin_dir = crate::settings::user_bin_dir(&user)?;
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        // First, before PostgreSQL is installed and its cluster moved: db-sync's unit
        // `Requires=` the relay's, so without the `cardano` stage the last action here would
        // fail after everything before it had changed the host. Checked when it runs rather
        // than while planning, because in a whole-host plan the `cardano` stage is a few
        // actions earlier in the same plan.
        actions.push(
            RequirePaths::plan(
                "the Cardano relay db-sync follows",
                vec![
                    (
                        crate::action::base::unit_path(CARDANO_NODE_SERVICE),
                        format!("the `{CARDANO_NODE_SERVICE}` unit"),
                    ),
                    (
                        bin_dir.join("cardano-node"),
                        String::from("the `cardano-node` binary"),
                    ),
                ],
                "Run the `cardano` step first.",
            )
            .await?
            .boxed(),
        );

        if self.common.install_base_packages {
            actions.push(base_packages().await?);
        }

        actions.push(
            CreateDirectory::plan(&paths.postgres_data, None, None, Some(0o755), false)
                .await?
                .boxed(),
        );
        actions.push(InstallPostgresql::plan(&self.common).await?.boxed());
        actions.push(RelocatePostgresCluster::plan(&self.common).await?.boxed());
        actions.push(CreatePostgresRole::plan(&credentials).await?.boxed());
        actions.push(CreatePostgresDatabase::plan(&credentials).await?.boxed());
        actions.push(CreatePgpassFile::plan(&user, &credentials).await?.boxed());
        actions.push(
            SaveDatabaseCredentials::plan(&paths.postgres_credentials_file, &credentials)
                .await?
                .boxed(),
        );

        actions.push(
            CreateDirectory::plan(
                &paths.cardano_db_sync_state,
                user.clone(),
                user.clone(),
                Some(0o755),
                false,
            )
            .await?
            .boxed(),
        );
        actions.push(InstallCardanoDbSync::plan(&self.common).await?.boxed());

        // Restarted only when this plan changes what it runs: its unit, or its binary.
        // db-sync rolls back and revalidates after a restart, which a re-run that changed
        // nothing should not cost the host.
        let unit = units::cardano_db_sync(&self.common, &credentials)?;
        let changed = unit_differs(CARDANO_DB_SYNC_SERVICE, &unit).await
            || !installed_version_matches(
                &bin_dir.join("cardano-db-sync"),
                &self.common.db_sync_version,
            )
            .await;
        actions.push(
            CreateSystemdUnit::plan(CARDANO_DB_SYNC_SERVICE, unit)
                .await?
                .boxed(),
        );
        actions.push(
            start_or_restart(CARDANO_DB_SYNC_SERVICE, changed)
                .await?
                .boxed(),
        );

        Ok(actions)
    }

    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        let mut settings = self.common.settings()?;
        settings.insert(
            "database_user".into(),
            serde_json::to_value(&self.database_user)?,
        );
        settings.insert(
            "database_name".into(),
            serde_json::to_value(&self.database_name)?,
        );
        Ok(settings)
    }

    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        diff_from_default(self).await
    }

    fn receipt_path(&self) -> PathBuf {
        self.common.paths().receipt(self.typetag_name())
    }

    async fn platform_check(&self) -> anyhow::Result<()> {
        platform_check(self.typetag_name())
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.common.cardano_user)
    }
}
