use std::collections::HashMap;
use std::path::PathBuf;

use crate::action::base::{CreateDirectory, CreateSymlink};
use crate::action::midnight::{
    ConfigureKeystore, CreateRegistrationFile, GenerateNetworkKey, GenerateValidatorKey,
    InstallMidnightNode, KeyScheme, MigrateLegacyLayout, NodeInvocation,
};
use crate::action::{Action, StatefulAction};
use crate::planner::{
    cardano::base_packages, diff_from_default, platform_check, require_root, require_user, Planner,
};
use crate::settings::CommonSettings;

/// The validator keys, in the order the registration file wants them
const VALIDATOR_KEYS: &[(&str, KeyScheme, &str)] = &[
    ("AURA", KeyScheme::Sr25519, "aura.json"),
    ("GRANDPA", KeyScheme::Ed25519, "grandpa.json"),
    ("Cross-Chain", KeyScheme::Ecdsa, "cross_chain.json"),
];

/** The Midnight node binary and this host's validator identity

Nothing here starts the node: running as a validator is its own stage, which needs the
database credentials this one does not, and which an operator may want to redo (a new node
name, a new beneficiary) without touching the keys.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct Midnight {
    #[clap(flatten)]
    pub common: CommonSettings,
}

#[async_trait::async_trait]
#[typetag::serde(name = "midnight")]
impl Planner for Midnight {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            common: CommonSettings::default(),
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let paths = self.common.paths();
        let user = self.common.midnight_user.clone();
        let home = crate::settings::user_home(&user)?;
        let node = NodeInvocation {
            user: user.clone(),
            binary: crate::settings::user_bin_dir(&user)?.join("midnight-node"),
            work_dir: paths.midnight_node_data.clone(),
            network: self.common.cardano_network.clone(),
        };
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        if self.common.install_base_packages {
            actions.push(base_packages().await?);
        }

        for (directory, mode) in [
            (&paths.midnight_data, 0o755),
            (&paths.midnight_node_data, 0o755),
            (&paths.midnight_runtime_data, 0o700),
            (&paths.midnight_keys_dir, 0o700),
        ] {
            actions.push(
                CreateDirectory::plan(directory, user.clone(), user.clone(), Some(mode), false)
                    .await?
                    .boxed(),
            );
        }
        actions.push(
            CreateDirectory::plan(&paths.scratch_dir, None, None, Some(0o700), true)
                .await?
                .boxed(),
        );
        for directory in crate::planner::cardano::user_directories(&user)? {
            actions.push(
                CreateDirectory::plan(directory, user.clone(), user.clone(), Some(0o755), false)
                    .await?
                    .boxed(),
            );
        }

        // Before anything generates a key: an identity from an earlier layout is the one
        // this host should keep
        actions.push(MigrateLegacyLayout::plan(&self.common).await?.boxed());
        actions.push(InstallMidnightNode::plan(&self.common).await?.boxed());
        actions.push(
            CreateSymlink::plan(home.join("res"), &paths.midnight_res_dir, user.clone())
                .await?
                .boxed(),
        );

        for (label, scheme, file) in VALIDATOR_KEYS {
            actions.push(
                GenerateValidatorKey::plan(
                    *label,
                    *scheme,
                    paths.midnight_keys_dir.join(file),
                    node.clone(),
                )
                .await?
                .boxed(),
            );
        }

        actions.push(
            GenerateNetworkKey::plan(
                paths.midnight_network_dir.join("secret_ed25519"),
                node.clone(),
            )
            .await?
            .boxed(),
        );
        actions.push(
            ConfigureKeystore::plan(&paths.midnight_keystore_dir, &paths.midnight_keys_dir, node)
                .await?
                .boxed(),
        );
        actions.push(
            CreateRegistrationFile::plan(
                &paths.midnight_keys_dir,
                &paths.midnight_registration_file,
                &user,
            )
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

    fn receipt_path(&self) -> PathBuf {
        self.common.paths().receipt(self.typetag_name())
    }

    async fn platform_check(&self) -> anyhow::Result<()> {
        platform_check(self.typetag_name())
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.common.midnight_user)
    }
}
