use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::action::base::{CreateDirectory, CreateSystemdUnit, StartSystemdUnit};
use crate::action::cardano::{FetchMithrilSnapshot, InstallCardanoNode, InstallMithrilClient};
use crate::action::{Action, StatefulAction};
use crate::planner::{
    diff_from_default, platform_check, require_root, require_user, units, Planner,
};
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
        let bin_dir = crate::settings::user_bin_dir(&user)?;
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

        // Restarted only when this plan changes what it runs: its unit, or its binary. A
        // relay restarted for nothing replays its ledger for a good while before it serves
        // a block again, and takes db-sync (which `Requires=` it) down with it.
        let unit = units::cardano_node(&self.common)?;
        let changed = unit_differs(CARDANO_NODE_SERVICE, &unit).await
            || !installed_version_matches(
                &bin_dir.join("cardano-node"),
                &self.common.cardano_node_version,
            )
            .await;
        actions.push(
            CreateSystemdUnit::plan(CARDANO_NODE_SERVICE, unit)
                .await?
                .boxed(),
        );
        actions.push(
            start_or_restart(CARDANO_NODE_SERVICE, changed)
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

/// Whether the binary at `path` reports itself as `version`
///
/// The release archives are re-fetched on every run, so this is how a plan tells a refresh
/// which changes the binary (and so has to restart the service) from one which does not.
/// A binary which is missing or will not answer counts as not matching: restarting is the
/// safe answer when nothing better is known.
pub(crate) async fn installed_version_matches(path: &Path, version: &str) -> bool {
    if !path.is_file() {
        return false;
    }

    match crate::command(&path.display().to_string())
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).contains(version)
        },
        _ => false,
    }
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

/// Whether applying `rendered` would leave a different unit file than the one on disk
pub(crate) async fn unit_differs(unit: &str, rendered: &str) -> bool {
    match tokio::fs::read_to_string(crate::action::base::unit_path(unit)).await {
        Ok(existing) => existing != rendered,
        Err(_) => true,
    }
}

/** Restart the unit if this plan changes what it runs, else only enable and start it

For a service which is expensive to bounce: a relay restarted for no reason replays its
ledger for a good while before it serves a block again, so a re-run which changed neither
its unit nor its binary leaves it alone.
*/
pub(crate) async fn start_or_restart(
    unit: &str,
    changed: bool,
) -> anyhow::Result<StatefulAction<StartSystemdUnit>> {
    if changed {
        StartSystemdUnit::plan_restart(unit).await
    } else {
        StartSystemdUnit::plan(unit, true).await
    }
}
