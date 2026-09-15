use std::process::ExitCode;

use clap::Parser;
use midnight_installer::cli::{MidnightInstallerCli, APP};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = MidnightInstallerCli::parse();

    if let Err(err) = cli.instrumentation.setup() {
        eprintln!("Error: {err:?}");
        return ExitCode::FAILURE;
    }
    tracing::debug!("{} v{}", APP.binary_name, APP.version);

    installer::cli::run(cli).await
}
