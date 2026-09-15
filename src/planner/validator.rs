use anyhow::Context;
use std::collections::HashMap;

use crate::action::base::{CreateSystemdUnit, RequirePaths, StartSystemdUnit};
use crate::action::midnight::{CreateValidatorEnvFile, PrepareSeedFiles};
use crate::action::postgres::CheckDatabaseCredentials;
use crate::action::{Action, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::planner::{diff_from_default, require_root, require_user, units, Planner};
use crate::settings::{
    CommonSettings, MainChainParams, Secret, DEFAULT_DB_NAME, DEFAULT_DB_USER,
    MIDNIGHT_NODE_SERVICE,
};

/// The default block beneficiary, as the FNO runbook gives it
pub(crate) const DEFAULT_SIDECHAIN_BLOCK_BENEFICIARY: &str =
    "0000000000000000000000000000000000000000000000000000000000000002";

/** Run the Midnight node as a validator

The last stage, because it depends on what every other stage produces: the keys, the
environment, and a database to read the main chain from.

It does not wait for any of them to have caught up. The node follows db-sync, which follows
the relay, and each of them retries until the one below it has what it needs, so starting
the validator early costs a while of restarts rather than a broken install. Whether the host
has actually caught up is a question for `status`, not a reason to refuse to install.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct Validator {
    #[clap(flatten)]
    pub common: CommonSettings,

    /// The password for the PostgreSQL role
    ///
    /// Read from the credentials the db-sync stage saved if it is not given, else asked for
    /// at the terminal. Whatever it is, it is checked against PostgreSQL before the node is
    /// given it.
    #[clap(long, env = "MIDNIGHT_INSTALLER_POSTGRES_PASSWORD")]
    pub postgres_password: Option<Secret>,

    /// The PostgreSQL role the node logs in as
    #[clap(long, default_value = DEFAULT_DB_USER, env = "MIDNIGHT_INSTALLER_DATABASE_USER")]
    pub database_user: String,

    /// The database db-sync populates
    #[clap(long, default_value = DEFAULT_DB_NAME, env = "MIDNIGHT_INSTALLER_DATABASE_NAME")]
    pub database_name: String,

    /// The name this validator reports to telemetry (default: this host's name)
    #[clap(long, env = "MIDNIGHT_INSTALLER_NODE_NAME")]
    pub node_name: Option<String>,

    /// The address blocks produced by this validator pay out to
    #[clap(
        long,
        default_value = DEFAULT_SIDECHAIN_BLOCK_BENEFICIARY,
        env = "MIDNIGHT_INSTALLER_SIDECHAIN_BLOCK_BENEFICIARY"
    )]
    pub sidechain_block_beneficiary: String,
}

impl Validator {
    /// The password given on the command line, else the one the db-sync stage saved
    pub async fn credentials(&self) -> anyhow::Result<DatabaseCredentials> {
        if let Some(password) = &self.postgres_password {
            return Ok(DatabaseCredentials::new(
                &self.database_user,
                &self.database_name,
                password.clone(),
            ));
        }

        let path = self.common.paths().postgres_credentials_file;
        DatabaseCredentials::load(&path).await.with_context(|| format!(
                "No PostgreSQL password: `{path}` could not be read. Run the `db-sync` step first, or pass `--postgres-password`.",
                path = path.display(),
            ))
    }

    pub fn node_name(&self) -> anyhow::Result<String> {
        let name = match &self.node_name {
            Some(name) => name.clone(),
            None => hostname()?,
        };

        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            anyhow::bail!(
                "`{name}` is not a usable node name: letters, numbers, `.`, `_` and `-` only"
            );
        }

        Ok(name)
    }

    fn main_chain_params(&self) -> anyhow::Result<MainChainParams> {
        MainChainParams::for_network(&self.common.cardano_network).with_context(|| format!(
                "The main chain parameters for `{network}` are not known to this installer, so the node's environment cannot be written. Only `preprod` is known.",
                network = self.common.cardano_network,
            ))
    }
}

fn hostname() -> anyhow::Result<String> {
    let hostname = nix::unistd::gethostname().context("Could not read this host's name")?;
    let hostname = hostname.to_string_lossy().to_string();

    // The short name, as `hostname -s` reports it
    Ok(hostname.split('.').next().unwrap_or(&hostname).to_string())
}

#[async_trait::async_trait]
#[typetag::serde(name = "validator")]
impl Planner for Validator {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            common: CommonSettings::default(),
            postgres_password: None,
            database_user: DEFAULT_DB_USER.into(),
            database_name: DEFAULT_DB_NAME.into(),
            node_name: None,
            sidechain_block_beneficiary: DEFAULT_SIDECHAIN_BLOCK_BENEFICIARY.into(),
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let paths = self.common.paths();
        let credentials = self.credentials().await?;
        let main_chain = self.main_chain_params()?;
        let node_name = self.node_name()?;
        let binary =
            crate::settings::user_bin_dir(&self.common.midnight_user)?.join("midnight-node");

        Ok(vec![
            // Checked here rather than while planning: in a whole-host plan the `midnight`
            // stage produces these a few actions earlier, so they do not exist yet when this
            // is planned, only by the time it runs
            RequirePaths::plan(
                "what the validator needs",
                vec![
                    (binary, String::from("the Midnight node binary")),
                    (
                        paths.midnight_chain_spec.clone(),
                        String::from("the chain spec"),
                    ),
                    (
                        paths.midnight_network_dir.join("secret_ed25519"),
                        String::from("the Midnight network key"),
                    ),
                ],
                "Run the `midnight` step first.",
            )
            .await?
            .boxed(),
            // Likewise: a wrong password would not fail here, it would fail in the node's
            // journal every ten seconds for ever while this stage reported success
            CheckDatabaseCredentials::plan(&credentials).await?.boxed(),
            PrepareSeedFiles::plan(&paths.midnight_keys_dir, &self.common.midnight_user)
                .await?
                .boxed(),
            CreateValidatorEnvFile::plan(
                &self.common,
                node_name,
                &credentials,
                main_chain,
                &self.sidechain_block_beneficiary,
            )
            .await?
            .boxed(),
            CreateSystemdUnit::plan(MIDNIGHT_NODE_SERVICE, units::midnight_node(&self.common)?)
                .await?
                .boxed(),
            // Restart rather than start: the environment file was just rewritten, and a node
            // still running with the old one would not be the node this plan describes
            StartSystemdUnit::plan_restart(MIDNIGHT_NODE_SERVICE)
                .await?
                .boxed(),
        ])
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
        settings.insert("node_name".into(), serde_json::to_value(&self.node_name)?);
        settings.insert(
            "sidechain_block_beneficiary".into(),
            serde_json::to_value(&self.sidechain_block_beneficiary)?,
        );
        Ok(settings)
    }

    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        diff_from_default(self).await
    }

    fn common_settings(&self) -> &CommonSettings {
        &self.common
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.common.midnight_user)
    }
}
