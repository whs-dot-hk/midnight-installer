/*! The gates between the stages of the build-out

The FNO runbook is a sequence with hard ordering: `cardano-db-sync` may not start before the
relay has fully synced, and the Midnight node may not run as a validator before db-sync has
caught up to the tip. These checks are what
[`pre_install_check`](crate::planner::Planner::pre_install_check) calls, so an out of order
step is refused while planning rather than half applied.
*/

use anyhow::Context;

use crate::credentials::DatabaseCredentials;
use crate::settings::CommonSettings;

/// How close db-sync must be to the relay's tip before the validator may start
pub const DB_SYNC_MAX_LAG: u64 = 20;

/// The point at which `cardano-cli query tip` is treated as fully synced
pub const SYNC_PROGRESS_COMPLETE: f64 = 99.99;

#[derive(Debug, Clone)]
pub struct CardanoTip {
    pub sync_progress: f64,
    pub block: u64,
}

/// Ask the relay where it is, via its socket
#[tracing::instrument(level = "debug", skip_all)]
pub async fn cardano_tip(settings: &CommonSettings) -> anyhow::Result<CardanoTip> {
    let paths = settings.paths();
    let user = &settings.cardano_user;
    let cli = crate::settings::user_bin_dir(user)?.join("cardano-cli");

    if !cli.is_file() {
        anyhow::bail!(
            "`{}` is not installed, run the `cardano` step first",
            cli.display()
        );
    }
    if !paths.cardano_socket.exists() {
        anyhow::bail!(
            "The Cardano node socket `{}` does not exist, the relay is not running",
            paths.cardano_socket.display()
        );
    }

    let mut command = crate::command_as(user, "env");
    command
        .arg(format!(
            "CARDANO_NODE_SOCKET_PATH={}",
            paths.cardano_socket.display()
        ))
        .arg("timeout")
        .arg("20")
        .arg(cli)
        .arg("query")
        .arg("tip")
        .arg("--testnet-magic")
        .arg(settings.cardano_magic.to_string());

    let stdout = crate::execute_command_stdout(&mut command)
        .await
        .context("Could not query the Cardano tip")?;
    let json: serde_json::Value =
        serde_json::from_str(&stdout).context("Could not parse the Cardano tip")?;

    let sync_progress = json
        .get("syncProgress")
        .and_then(|value| match value {
            serde_json::Value::String(progress) => progress.parse::<f64>().ok(),
            serde_json::Value::Number(progress) => progress.as_f64(),
            _ => None,
        })
        .context("`cardano-cli` returned no `syncProgress`")?;
    let block = json
        .get("block")
        .and_then(|value| value.as_u64())
        .context("`cardano-cli` returned no `block`")?;

    Ok(CardanoTip {
        sync_progress,
        block,
    })
}

/// Refuse to continue unless the relay has reached the tip
#[tracing::instrument(level = "debug", skip_all)]
pub async fn require_cardano_synced(settings: &CommonSettings) -> anyhow::Result<CardanoTip> {
    if !crate::action::base::unit_is_active(crate::settings::CARDANO_NODE_SERVICE).await {
        anyhow::bail!(
            "`{}` is not running, so the relay cannot be synced",
            crate::settings::CARDANO_NODE_SERVICE
        );
    }

    let tip = cardano_tip(settings).await?;
    if tip.sync_progress < SYNC_PROGRESS_COMPLETE {
        anyhow::bail!(
            "The Cardano relay is at {progress}%. Wait for it to reach 100%, then run this step again.",
            progress = tip.sync_progress,
        );
    }

    tracing::debug!("Cardano node is synced ({}%)", tip.sync_progress);
    Ok(tip)
}

/// Run a single statement and return its unaligned, untitled output
///
/// The password travels in `PGPASSWORD`, which the command's debug rendering would include,
/// so this goes through the runner which never logs the command line.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn psql_query(credentials: &DatabaseCredentials, sql: &str) -> anyhow::Result<String> {
    let mut command = crate::command("psql");
    command
        .env("PGPASSWORD", credentials.password.expose())
        .arg("-h")
        .arg(&credentials.host)
        .arg("-p")
        .arg(credentials.port.to_string())
        .arg("-U")
        .arg(&credentials.user)
        .arg("-d")
        .arg(&credentials.name)
        .arg("-v")
        .arg("ON_ERROR_STOP=1")
        .arg("-tAc")
        .arg(sql);

    let output = crate::execute_command_secret(&mut command, "psql")
        .await
        .with_context(|| {
            format!(
                "Could not query `{db}` as `{user}`",
                db = credentials.name,
                user = credentials.user
            )
        })?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether these credentials can actually open the database
pub async fn postgres_reachable(credentials: &DatabaseCredentials) -> bool {
    matches!(psql_query(credentials, "SELECT 1;").await, Ok(value) if value == "1")
}

/// The highest block `cardano-db-sync` has written
pub async fn db_sync_block(credentials: &DatabaseCredentials) -> anyhow::Result<u64> {
    let value = psql_query(credentials, "SELECT COALESCE(MAX(block_no),0) FROM block;").await?;

    value
        .parse::<u64>()
        .with_context(|| format!("`cardano-db-sync` returned `{value}` as its latest block"))
}

/// Refuse to continue unless db-sync has caught up with the relay
#[tracing::instrument(level = "debug", skip_all)]
pub async fn require_db_sync_near_tip(
    settings: &CommonSettings,
    credentials: &DatabaseCredentials,
) -> anyhow::Result<u64> {
    if !crate::action::base::unit_is_active(crate::settings::CARDANO_DB_SYNC_SERVICE).await {
        anyhow::bail!(
            "`{}` is not running",
            crate::settings::CARDANO_DB_SYNC_SERVICE
        );
    }

    let tip = cardano_tip(settings).await?;
    let db_block = db_sync_block(credentials).await?;
    let lag = tip.block.saturating_sub(db_block);

    tracing::info!(
        "Cardano tip block: {tip_block}, db-sync block: {db_block}, lag: {lag} block(s)",
        tip_block = tip.block
    );

    if lag > DB_SYNC_MAX_LAG {
        anyhow::bail!(
            "`cardano-db-sync` is {lag} blocks behind the relay. Wait for it to catch up (at most {DB_SYNC_MAX_LAG} blocks), then run this step again."
        );
    }
    Ok(lag)
}

pub fn is_root() -> bool {
    nix::unistd::Uid::effective().is_root()
}
