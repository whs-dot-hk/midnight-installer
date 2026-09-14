use anyhow::Context;
use std::collections::HashMap;

use crate::action::base::{CreateSystemdUnit, StartSystemdUnit};
use crate::action::midnight::{CreateValidatorEnvFile, PrepareSeedFiles};
use crate::action::{Action, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::planner::{diff_from_default, require_root, units, Planner};
use crate::settings::{
    CommonSettings, MainChainParams, Secret, DEFAULT_DB_NAME, DEFAULT_DB_USER,
    MIDNIGHT_NODE_SERVICE,
};

/// The default block beneficiary, as the FNO runbook gives it
const DEFAULT_SIDECHAIN_BLOCK_BENEFICIARY: &str =
    "0000000000000000000000000000000000000000000000000000000000000002";

/** Run the Midnight node as a validator

The last stage, and the one with the most prerequisites: the node reads the chain through
db-sync, so it may not start until db-sync has caught up with the relay, which in turn may
not have started until the relay was synced.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct Validator {
    #[clap(flatten)]
    pub common: CommonSettings,

    /// The password for the PostgreSQL role
    ///
    /// Read from the credentials the db-sync stage saved if it is not given.
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

        Ok(vec![
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

        let paths = self.common.paths();
        let binary =
            crate::settings::user_bin_dir(&self.common.midnight_user)?.join("midnight-node");

        for (path, what) in [
            (binary, "the Midnight node binary"),
            (paths.midnight_chain_spec.clone(), "the chain spec"),
            (
                paths.midnight_network_dir.join("secret_ed25519"),
                "the Midnight network key",
            ),
        ] {
            if !path.exists() {
                anyhow::bail!(
                    "`{path}` is missing ({what}). Run the `midnight` step first.",
                    path = path.display(),
                );
            }
        }

        // Everything below the node has to be in place and caught up before it may sign
        crate::check::require_cardano_synced(&self.common).await?;

        if !crate::action::base::unit_is_active("postgresql").await {
            anyhow::bail!("PostgreSQL is not running. Run the `db-sync` step first.");
        }

        let credentials = self.credentials().await?;
        if !crate::check::postgres_reachable(&credentials).await {
            anyhow::bail!(
                "Could not log in to `{name}` as `{user}` with the password given. Run the `db-sync` step first, or pass the right `--postgres-password`.",
                name = credentials.name,
                user = credentials.user,
            );
        }

        crate::check::require_db_sync_near_tip(&self.common, &credentials).await?;

        Ok(())
    }
}
