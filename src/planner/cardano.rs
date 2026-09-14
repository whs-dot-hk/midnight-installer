use std::collections::HashMap;

use crate::action::base::{CreateDirectory, CreateSystemdUnit, StartSystemdUnit};
use crate::action::cardano::{FetchMithrilSnapshot, InstallCardanoNode, InstallMithrilClient};
use crate::action::{Action, StatefulAction};
use crate::planner::{diff_from_default, require_root, require_user, units, Planner};
use crate::settings::{CommonSettings, CARDANO_NODE_SERVICE};

/** The Cardano relay: binaries, a Mithril bootstrap, and the service

The snapshot is what makes this practical: a relay which had to sync Preprod from genesis
would take days before db-sync could even start.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct Cardano {
    #[clap(flatten)]
    pub common: CommonSettings,
}

#[async_trait::async_trait]
#[typetag::serde(name = "cardano")]
impl Planner for Cardano {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            common: CommonSettings::default(),
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let paths = self.common.paths();
        let user = self.common.cardano_user.clone();
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        if self.common.install_base_packages {
            actions.push(base_packages().await?);
        }

        actions.push(
            CreateDirectory::plan(
                &paths.cardano_data,
                user.clone(),
                user.clone(),
                Some(0o755),
                false,
            )
            .await?
            .boxed(),
        );

        // The binaries and chain configuration live under the service user's home, so those
        // directories have to belong to the service user, not to the root running this
        for directory in user_directories(&user)? {
            actions.push(
                CreateDirectory::plan(directory, user.clone(), user.clone(), Some(0o755), false)
                    .await?
                    .boxed(),
            );
        }
        actions.push(
            CreateDirectory::plan(&paths.scratch_dir, None, None, Some(0o700), true)
                .await?
                .boxed(),
        );
        actions.push(InstallCardanoNode::plan(&self.common).await?.boxed());
        actions.push(InstallMithrilClient::plan(&self.common).await?.boxed());
        actions.push(FetchMithrilSnapshot::plan(&self.common).await?.boxed());
        actions.push(
            CreateSystemdUnit::plan(CARDANO_NODE_SERVICE, units::cardano_node(&self.common)?)
                .await?
                .boxed(),
        );
        actions.push(
            StartSystemdUnit::plan(CARDANO_NODE_SERVICE, true)
                .await?
                .boxed(),
        );

        Ok(actions)
    }

    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        self.common.settings()
    }

    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        diff_from_default(self).await
    }

    fn common_settings(&self) -> &CommonSettings {
        &self.common
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.common.cardano_user)
    }
}

/// `~user/.local`, and the `bin` and `share` directories inside it
pub(crate) fn user_directories(user: &str) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let home = crate::settings::user_home(user)?;

    Ok(vec![
        home.join(".local"),
        crate::settings::user_bin_dir(user)?,
        crate::settings::user_share_dir(user)?,
    ])
}

/// The packages every stage of the build-out assumes are present
pub(crate) async fn base_packages() -> anyhow::Result<StatefulAction<Box<dyn Action>>> {
    Ok(crate::action::base::AptInstall::plan([
        "curl",
        "wget",
        "ca-certificates",
        "jq",
        "tar",
        "gzip",
        "xz-utils",
        "rsync",
        "lsb-release",
        "gnupg",
        "coreutils",
        "sudo",
    ])
    .await?
    .boxed())
}
