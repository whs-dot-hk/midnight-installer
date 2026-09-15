/*! The command line: `plan`, `install`, `uninstall`, `status`

The framework owns the machinery — confirmation, `sudo`, signal handling, receipts — and
this module owns what is specific to this product: the clap tree whose subcommands are the
FNO stages, the secrets those stages need asked for, and the `status` report the framework
knows nothing about.
*/

pub(crate) mod arg;
pub(crate) mod subcommand;

use std::process::ExitCode;

use clap::Parser;

use self::subcommand::MidnightInstallerSubcommand;

pub use installer::cli::{interaction, App, CommandExecute};

/// Who this installer is, for receipts, `sudo`, and the header of every plan
pub const APP: App = App {
    product: "Midnight FNO",
    binary_name: "midnight-installer",
    version: env!("CARGO_PKG_VERSION"),
    env_prefix: "MIDNIGHT_INSTALLER",
};

/**
The Midnight federated-node-operator installer

Each stage of the FNO build-out is a subcommand of `install`, and every stage can be
described with `plan` before it is applied, or undone with `uninstall`.
*/
#[derive(Debug, Parser)]
#[clap(version)]
pub struct MidnightInstallerCli {
    #[clap(flatten)]
    pub instrumentation: arg::Instrumentation,

    #[clap(subcommand)]
    pub subcommand: MidnightInstallerSubcommand,
}

#[async_trait::async_trait]
impl CommandExecute for MidnightInstallerCli {
    #[tracing::instrument(level = "trace", skip_all)]
    async fn execute(self) -> anyhow::Result<ExitCode> {
        match self.subcommand {
            MidnightInstallerSubcommand::Plan(plan) => plan.execute().await,
            MidnightInstallerSubcommand::Install(install) => install.execute().await,
            MidnightInstallerSubcommand::Uninstall(uninstall) => uninstall.execute().await,
            MidnightInstallerSubcommand::Status(status) => status.execute().await,
        }
    }
}

/// Re-run ourselves under `sudo` if we are not already root
pub fn ensure_root() -> anyhow::Result<()> {
    installer::cli::ensure_root(&APP)
}
