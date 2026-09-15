use std::collections::HashMap;

use crate::action::base::CreateDirectory;
use crate::action::{Action, StatefulAction};
use crate::planner::{diff_from_default, require_root, require_user, Planner};
use crate::settings::CommonSettings;

/** The `/data` layout every later stage writes into

Each directory is created exactly as the stage which fills it would create it — the same
owner, the same mode — so that stage finds it already in place rather than owned by someone
else, and so the whole-host plan can tell the two are the same action.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct Directories {
    #[clap(flatten)]
    pub common: CommonSettings,
}

#[async_trait::async_trait]
#[typetag::serde(name = "directories")]
impl Planner for Directories {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            common: CommonSettings::default(),
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let paths = self.common.paths();
        let cardano = &self.common.cardano_user;
        let midnight = &self.common.midnight_user;
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        for (directory, owner, mode) in [
            (&paths.cardano_data, Some(cardano), 0o755),
            (&paths.midnight_data, Some(midnight), 0o755),
            (&paths.midnight_node_data, Some(midnight), 0o755),
            (&paths.postgres_data, None, 0o755),
            (&paths.wireguard_data, None, 0o700),
        ] {
            actions.push(
                CreateDirectory::plan(directory, owner.cloned(), owner.cloned(), Some(mode), false)
                    .await?
                    .boxed(),
            );
        }

        // The installer's own state: receipts, and the scratch space release archives are
        // unpacked into
        actions.push(
            CreateDirectory::plan(&paths.state_dir, None, None, Some(0o700), false)
                .await?
                .boxed(),
        );
        actions.push(
            CreateDirectory::plan(&paths.receipt_dir, None, None, Some(0o700), false)
                .await?
                .boxed(),
        );
        actions.push(
            CreateDirectory::plan(&paths.scratch_dir, None, None, Some(0o700), true)
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
        require_user(&self.common.cardano_user)?;
        require_user(&self.common.midnight_user)
    }
}
