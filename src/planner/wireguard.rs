use std::collections::HashMap;

use crate::action::base::CreateDirectory;
use crate::action::wireguard::{GenerateWireguardKeys, InstallWireguardTools};
use crate::action::{Action, StatefulAction};
use crate::planner::{diff_from_default, Planner};
use crate::settings::CommonSettings;

/** The WireGuard tooling and this host's tunnel identity

Only the identity: the peer and interface configuration comes from the Foundation once they
have this host's public key, and is not this installer's to invent.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct Wireguard {
    #[clap(flatten)]
    pub common: CommonSettings,
}

#[async_trait::async_trait]
#[typetag::serde(name = "wireguard")]
impl Planner for Wireguard {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            common: CommonSettings::default(),
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let paths = self.common.paths();

        Ok(vec![
            CreateDirectory::plan(&paths.wireguard_data, None, None, Some(0o700), false)
                .await?
                .boxed(),
            InstallWireguardTools::plan(&self.common.wireguard_tools_version)
                .await?
                .boxed(),
            GenerateWireguardKeys::plan(&paths.wireguard_data)
                .await?
                .boxed(),
        ])
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
