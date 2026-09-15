/*! The Midnight federated-node-operator (FNO) installer

This crate turns the FNO host setup — a Cardano relay bootstrapped from a Mithril snapshot,
PostgreSQL and `cardano-db-sync`, the Midnight node with its validator keys, and the
WireGuard identity — into the three concepts the [`installer`] framework supplies:

* [`Action`]: an executable, revertable step, possibly orchestrating sub-[`Action`]s.
* [`InstallPlan`]: a sequence of [`Action`]s plus metadata, which can be described, carried
  out, and reverted.
* [`Planner`](planner::Planner): something which produces an [`InstallPlan`] for one
  component of the host.

What lives here is what is specific to an FNO host: the planners, the actions they are built
from, the settings, and the `status` report. The machinery which runs them — actions,
plans, receipts, and the `plan` / `install` / `uninstall` CLI — comes from [`installer`].

Each component is its own planner, in the order the FNO runbook imposes, and
[`all`](planner::all::All) lays the six end to end as one plan. Nothing waits for the host to
catch up: db-sync follows a relay which is still syncing and the node follows a db-sync which
is still filling, so [`pre_install_check`](planner::Planner::pre_install_check) asks only what
must be true before anything runs, and how far along the host is belongs to [`status`].

Nothing recovers from an error: every failure is reported to the operator and stops the
stage, so errors are [`anyhow`] chains of context rather than typed variants.

```rust,no_run
use midnight_installer::{planner::Planner, InstallPlan};

# async fn wrapper() -> anyhow::Result<()> {
use midnight_installer::cli::APP;

let planner = midnight_installer::planner::cardano::Cardano::default().await?;
let mut plan = InstallPlan::plan(planner, APP.product, APP.version).await?;
match plan.install(None, APP.version).await {
    Ok(()) => tracing::info!("Done"),
    Err(e) => {
        tracing::error!("{e:#}");
        plan.uninstall(None, APP.version).await?;
    },
};
# Ok(())
# }
```
*/

pub mod action;
pub mod check;
pub mod cli;
pub mod credentials;
pub mod planner;
pub mod settings;
pub mod status;
mod util;

pub use installer::InstallPlan;
pub use planner::BuiltinPlanner;

pub use action::Action;

// The framework's command helpers, under the names the product actions already use
pub(crate) use installer::command::{
    command, command_as, command_succeeds, execute_command, execute_command_secret,
    execute_command_stdout, execute_command_with_stdin,
};
