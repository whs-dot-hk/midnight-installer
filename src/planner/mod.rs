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

use crate::settings::CommonSettings;

pub use installer::planner::{diff_from_default, Planner};

// The gates every planner in this crate shares. The `Planner` trait's own defaults are
// no-ops, because the framework knows nothing about what a product's stages need; these are
// what every FNO stage needs, and every implementation below calls them.

/// This installer only builds `x86_64` Linux hosts
pub(crate) fn platform_check(planner: &str) -> anyhow::Result<()> {
    use std::env::consts::{ARCH, OS};
    if (ARCH, OS) != ("x86_64", "linux") {
        anyhow::bail!("The `{planner}` planner does not support {ARCH} {OS}");
    }
    Ok(())
}

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

/** Run `$body` with the concrete planner inside a [`BuiltinPlanner`]

The framework's `plan` / `install` / `uninstall` runners take a concrete `P: Planner`, so
that a planner keeps its own type all the way to its receipt. This is the one place the
stage chosen on the command line is turned back into that type.
*/
macro_rules! with_planner {
    ($planner:expr, |$bound:ident| $body:block) => {
        match $planner {
            $crate::planner::BuiltinPlanner::All($bound) => $body,
            $crate::planner::BuiltinPlanner::Directories($bound) => $body,
            $crate::planner::BuiltinPlanner::Cardano($bound) => $body,
            $crate::planner::BuiltinPlanner::DbSync($bound) => $body,
            $crate::planner::BuiltinPlanner::Midnight($bound) => $body,
            $crate::planner::BuiltinPlanner::Wireguard($bound) => $body,
            $crate::planner::BuiltinPlanner::Validator($bound) => $body,
        }
    };
}

pub(crate) use with_planner;
