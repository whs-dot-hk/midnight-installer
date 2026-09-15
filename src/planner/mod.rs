/*! [`Planner`]s: what produces an [`InstallPlan`](crate::InstallPlan) for one component

Each stage of the FNO build-out is its own planner, in the order the runbook imposes:

1. [`directories`](crate::planner::directories::Directories) — the `/data` layout
2. [`cardano`](crate::planner::cardano::Cardano) — the relay, bootstrapped from Mithril
3. [`db_sync`](crate::planner::db_sync::DbSync) — PostgreSQL and `cardano-db-sync`
4. [`midnight`](crate::planner::midnight::Midnight) — the node binary and validator keys
5. [`wireguard`](crate::planner::wireguard::Wireguard) — tunnel tooling and identity
6. [`validator`](crate::planner::validator::Validator) — the node in validator mode

[`all`](crate::planner::all::All) is all six at once, which is the usual way to build a host;
the individual stages are for redoing one part of one.

[`pre_install_check`](Planner::pre_install_check) is for what must be true of the machine
before anything runs: root, and the users the services will own files as. Whether the host
has *caught up* — the relay synced, db-sync near the tip — is not a precondition for
installing, because each service follows the one below it and retries. That question belongs
to [`status`](crate::status).
*/

pub mod all;
pub mod cardano;
pub mod db_sync;
pub mod directories;
pub mod midnight;
pub mod units;
pub mod validator;
pub mod wireguard;

use std::collections::HashMap;

use crate::{action::StatefulAction, settings::CommonSettings, Action, InstallPlan};

/// Something which can produce an [`InstallPlan`](crate::InstallPlan)
#[async_trait::async_trait]
#[typetag::serde(tag = "planner")]
pub trait Planner: std::fmt::Debug + Send + Sync + dyn_clone::DynClone {
    /// Instantiate the planner with default settings, if possible
    async fn default() -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Plan the [`Action`]s this component needs
    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>>;

    /// Every setting in force, for `--explain` and for the receipt
    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>>;

    /// Only the settings which differ from the defaults, for the plan description
    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>>;

    /// The settings every planner shares, which is where the data root (and so the receipt
    /// location) comes from
    fn common_settings(&self) -> &CommonSettings;

    fn boxed(self) -> Box<dyn Planner>
    where
        Self: Sized + 'static,
    {
        Box::new(self)
    }

    /// Whether this planner can run on this machine at all
    async fn platform_check(&self) -> anyhow::Result<()> {
        use std::env::consts::{ARCH, OS};
        if (ARCH, OS) != ("x86_64", "linux") {
            anyhow::bail!(
                "The `{}` planner does not support {ARCH} {OS}",
                self.typetag_name()
            );
        }
        Ok(())
    }

    /// The gates which must hold before this component may be installed
    ///
    /// Every stage changes the system, so every stage needs root; planners with further
    /// gates call [`require_root`] first.
    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()
    }

    async fn pre_uninstall_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

dyn_clone::clone_trait_object!(Planner);

/// Planning inspects the machine the way an install would, so it needs the same privileges
pub(crate) fn require_root() -> anyhow::Result<()> {
    if !crate::check::is_root() {
        anyhow::bail!("This step must be run as root: re-run it with `sudo`");
    }
    Ok(())
}

/// The service user has to exist before anything can be owned by it
pub(crate) fn require_user(user: &str) -> anyhow::Result<()> {
    if !crate::settings::user_exists(user) {
        anyhow::bail!("No user `{user}` on this system, create it first");
    }
    Ok(())
}

/** The stages this installer knows how to plan

Each is a subcommand of `install`, `plan` and `uninstall`, in the order the runbook runs
them.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Subcommand)]
pub enum BuiltinPlanner {
    /// The whole host in one plan: every stage below, in order
    All(all::All),
    /// The `/data` directory layout every later stage writes into
    Directories(directories::Directories),
    /// The Cardano relay, bootstrapped from a Mithril snapshot
    Cardano(cardano::Cardano),
    /// PostgreSQL and `cardano-db-sync`, which follows the relay
    DbSync(db_sync::DbSync),
    /// The Midnight node binary and this host's validator keys
    Midnight(midnight::Midnight),
    /// WireGuard tooling and this host's tunnel identity
    Wireguard(wireguard::Wireguard),
    /// The Midnight node in validator mode (needs the `midnight` stage)
    Validator(validator::Validator),
}

impl BuiltinPlanner {
    pub async fn plan(self) -> anyhow::Result<InstallPlan> {
        match self {
            Self::All(planner) => InstallPlan::plan(planner).await,
            Self::Directories(planner) => InstallPlan::plan(planner).await,
            Self::Cardano(planner) => InstallPlan::plan(planner).await,
            Self::DbSync(planner) => InstallPlan::plan(planner).await,
            Self::Midnight(planner) => InstallPlan::plan(planner).await,
            Self::Wireguard(planner) => InstallPlan::plan(planner).await,
            Self::Validator(planner) => InstallPlan::plan(planner).await,
        }
    }

    pub fn common_settings(&self) -> &CommonSettings {
        match self {
            Self::All(planner) => planner.common(),
            Self::Directories(planner) => &planner.common,
            Self::Cardano(planner) => &planner.common,
            Self::DbSync(planner) => &planner.common,
            Self::Midnight(planner) => &planner.common,
            Self::Wireguard(planner) => &planner.common,
            Self::Validator(planner) => &planner.common,
        }
    }

    /// The name this planner's receipt is written under
    pub fn typetag_name(&self) -> &'static str {
        match self {
            Self::All(planner) => planner.typetag_name(),
            Self::Directories(planner) => planner.typetag_name(),
            Self::Cardano(planner) => planner.typetag_name(),
            Self::DbSync(planner) => planner.typetag_name(),
            Self::Midnight(planner) => planner.typetag_name(),
            Self::Wireguard(planner) => planner.typetag_name(),
            Self::Validator(planner) => planner.typetag_name(),
        }
    }
}

/// Only the settings which differ from this planner type's defaults
pub(crate) async fn diff_from_default<P>(
    planner: &P,
) -> anyhow::Result<HashMap<String, serde_json::Value>>
where
    P: Planner + Sized,
{
    let default = P::default().await?.settings()?;
    let configured = planner.settings()?;

    let mut settings = HashMap::new();
    for (key, value) in configured.into_iter() {
        if default.get(&key) != Some(&value) {
            settings.insert(key, value);
        }
    }

    Ok(settings)
}
