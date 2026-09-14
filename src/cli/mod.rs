/*! The command line: `plan`, `install`, `uninstall`, `status` */

pub(crate) mod arg;
pub(crate) mod interaction;
pub(crate) mod subcommand;

use std::{ffi::CString, process::ExitCode};

use anyhow::Context;
use clap::Parser;
use owo_colors::OwoColorize;
use tokio::sync::broadcast::{Receiver, Sender};

use self::subcommand::MidnightInstallerSubcommand;

#[async_trait::async_trait]
pub trait CommandExecute {
    async fn execute(self) -> anyhow::Result<ExitCode>;
}

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

/// A channel which fires when the operator interrupts us, so an install can stop between
/// actions rather than in the middle of one
pub(crate) fn signal_channel() -> anyhow::Result<(Sender<()>, Receiver<()>)> {
    let (sender, receiver) = tokio::sync::broadcast::channel(100);

    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Installing the SIGINT handler")?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Installing the SIGTERM handler")?;

    let sender_cloned = sender.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(()) = interrupt.recv() => {
                    tracing::warn!("Got SIGINT, stopping after this step");
                    sender_cloned.send(()).ok();
                },
                Some(()) = terminate.recv() => {
                    tracing::warn!("Got SIGTERM, stopping after this step");
                    sender_cloned.send(()).ok();
                },
            }
        }
    });

    Ok((sender, receiver))
}

/// Re-run ourselves under `sudo` if we are not already root
pub fn ensure_root() -> anyhow::Result<()> {
    if crate::check::is_root() {
        return Ok(());
    }

    eprintln!(
        "{}",
        "`midnight-installer` needs to run as `root`, escalating with `sudo`..."
            .yellow()
            .dimmed()
    );

    let mut arguments = vec![
        CString::new("sudo").context("Building the `sudo` argument")?,
        CString::new("--set-home").context("Building the `--set-home` argument")?,
    ];

    // `sudo` drops the environment, so the few variables which configure this installer are
    // carried across deliberately. Only their names are named here: a value passed as
    // `env KEY=VALUE` would be visible in the process list and in sudo's own log, and one of
    // them may be the database password.
    let preserved: Vec<String> = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| {
            matches!(key.as_str(), "RUST_LOG" | "RUST_BACKTRACE" | "SHELL")
                || key.starts_with("MIDNIGHT_INSTALLER")
                || key.starts_with("http_proxy")
                || key.starts_with("https_proxy")
                || key.starts_with("HTTP_PROXY")
                || key.starts_with("HTTPS_PROXY")
        })
        .collect();

    if !preserved.is_empty() {
        arguments.push(
            CString::new(format!("--preserve-env={}", preserved.join(",")))
                .context("Building the `--preserve-env` argument")?,
        );
    }

    for argument in std::env::args() {
        arguments.push(CString::new(argument).context("Building an argument")?);
    }

    let sudo = CString::new("sudo").context("Building the `sudo` program name")?;
    tracing::trace!("Executing `{sudo:?}` with `{arguments:?}`");
    nix::unistd::execvp(&sudo, &arguments).context("Re-running this installer under `sudo`")?;

    Ok(())
}
