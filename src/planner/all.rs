/*! The whole FNO host, as one plan

Every other planner covers one component. This one covers the host: it asks each of them for
its actions and lays them end to end, so the build-out is a single plan to read, a single
confirmation, and a single receipt which unwinds the lot in reverse.

That is possible because no stage waits for another to have *caught up*, only for it to have
*run*. The services below each other retry: db-sync follows a relay which is still syncing,
and the node follows a db-sync which is still filling. Within one plan each stage's inputs
are produced by actions a few steps earlier, which is why the checks for them happen when
they run rather than when they are planned.
*/

use std::collections::{HashMap, HashSet};

use crate::action::{Action, StatefulAction};
use crate::planner::{
    cardano::Cardano, db_sync::DbSync, diff_from_default, directories::Directories,
    midnight::Midnight, require_root, require_user, validator::Validator,
    validator::DEFAULT_SIDECHAIN_BLOCK_BENEFICIARY, wireguard::Wireguard, Planner,
};
use crate::settings::{CommonSettings, Secret, DEFAULT_DB_NAME, DEFAULT_DB_USER};

/// Everything, in the order the runbook imposes
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct All {
    #[clap(flatten)]
    pub common: CommonSettings,

    /// The password for the PostgreSQL role
    ///
    /// Prompted for if it is not given. It is saved, root-only, so a later run of a single
    /// stage does not have to ask for it again.
    #[clap(long, env = "MIDNIGHT_INSTALLER_POSTGRES_PASSWORD")]
    pub postgres_password: Option<Secret>,

    /// The PostgreSQL role db-sync and the node log in as
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

impl All {
    fn directories(&self) -> Directories {
        Directories {
            common: self.common.clone(),
        }
    }

    fn cardano(&self) -> Cardano {
        Cardano {
            common: self.common.clone(),
        }
    }

    fn db_sync(&self) -> DbSync {
        DbSync {
            common: self.common.clone(),
            postgres_password: self.postgres_password.clone(),
            database_user: self.database_user.clone(),
            database_name: self.database_name.clone(),
        }
    }

    fn midnight(&self) -> Midnight {
        Midnight {
            common: self.common.clone(),
        }
    }

    fn wireguard(&self) -> Wireguard {
        Wireguard {
            common: self.common.clone(),
        }
    }

    fn validator(&self) -> Validator {
        Validator {
            common: self.common.clone(),
            postgres_password: self.postgres_password.clone(),
            database_user: self.database_user.clone(),
            database_name: self.database_name.clone(),
            node_name: self.node_name.clone(),
            sidechain_block_beneficiary: self.sidechain_block_beneficiary.clone(),
        }
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "all")]
impl Planner for All {
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
        let stages = [
            self.directories().plan().await?,
            self.cardano().plan().await?,
            self.db_sync().plan().await?,
            self.midnight().plan().await?,
            self.wireguard().plan().await?,
            self.validator().plan().await?,
        ];

        // The stages overlap: several of them install the same base packages and create the
        // same directories. An action's synopsis names what it does and what it does it to,
        // so the same synopsis twice is the same work twice, and the first one wins.
        let mut seen = HashSet::new();
        let mut actions = vec![];
        for stage in stages {
            for action in stage {
                if seen.insert(action.tracing_synopsis()) {
                    actions.push(action);
                }
            }
        }

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
        require_user(&self.common.cardano_user)?;
        require_user(&self.common.midnight_user)
    }
}
