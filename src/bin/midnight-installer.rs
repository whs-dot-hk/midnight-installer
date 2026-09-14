use std::{io::IsTerminal, process::ExitCode};

use clap::Parser;
use midnight_installer::cli::{CommandExecute, MidnightInstallerCli};
use owo_colors::OwoColorize;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = MidnightInstallerCli::parse();

    let result = match cli.instrumentation.setup() {
        Ok(()) => {
            tracing::debug!("midnight-installer v{}", env!("CARGO_PKG_VERSION"));
            cli.execute().await
        },
        Err(err) => Err(err),
    };

    match result {
        Ok(code) => code,
        Err(err) => {
            // The chain of context is the whole story: what was being done, and why it failed
            let report = format!("Error: {err:?}");
            if std::io::stderr().is_terminal() {
                eprintln!("{}", report.red());
            } else {
                eprintln!("{report}");
            }
            ExitCode::FAILURE
        },
    }
}
