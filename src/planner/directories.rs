use std::collections::HashMap;

use crate::action::base::CreateDirectory;
use crate::action::{Action, StatefulAction};
use crate::planner::{diff_from_default, Planner};
use crate::settings::CommonSettings;

/// The `/data` layout every later stage writes into
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
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        for directory in paths.base_directories() {
            actions.push(
                CreateDirectory::plan(directory, None, None, Some(0o755), false)
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
}
