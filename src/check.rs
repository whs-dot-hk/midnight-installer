/*! Questions about how far the host has caught up

The FNO build-out used to be gated on these: db-sync could not be installed before the relay
had synced, nor the validator before db-sync had reached the tip. Nothing is gated any more,
because each service follows the one below it and retries, so these are now what
[`status`](crate::status) asks in order to tell the operator whether the host is *there yet*,
and how far off it is if not.
*/

use anyhow::Context;

use crate::credentials::DatabaseCredentials;
use crate::settings::CommonSettings;

/// How close db-sync must be to the relay's tip to count as caught up
pub const DB_SYNC_MAX_LAG: u64 = 20;

/// The point at which `cardano-cli query tip` is treated as fully synced
pub const SYNC_PROGRESS_COMPLETE: f64 = 99.99;

#[derive(Debug, Clone)]
pub struct CardanoTip {
    pub sync_progress: f64,
    pub block: u64,
}

/// Whether a service has caught up, or is still on its way
///
/// Only the two states a working service can be in. A service which is broken (not running,
/// not answering, refusing the password) is neither, and is reported as an error by whatever
/// asked, so that `status` never shows a failure as "still catching up".
#[derive(Debug, Clone, PartialEq)]
pub enum Readiness {
    Ready(String),
    Waiting(String),
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
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

/// Whether the relay has reached the tip of the chain
pub fn cardano_readiness(tip: &CardanoTip) -> Readiness {
    if tip.sync_progress < SYNC_PROGRESS_COMPLETE {
        Readiness::Waiting(format!(
            "The Cardano relay is at {progress}% and still syncing",
            progress = tip.sync_progress,
        ))
    } else {
        Readiness::Ready(format!(
            "The relay is synced ({progress}%, block {block})",
            progress = tip.sync_progress,
            block = tip.block,
        ))
    }
}

/// Whether db-sync, at `db_block`, has caught up with a relay at `tip`
pub fn db_sync_readiness(tip: &CardanoTip, db_block: u64) -> Readiness {
    let lag = tip.block.saturating_sub(db_block);

    if lag > DB_SYNC_MAX_LAG {
        Readiness::Waiting(format!(
            "`cardano-db-sync` is {lag} blocks behind the relay and still catching up (at most {DB_SYNC_MAX_LAG} counts as caught up)"
        ))
    } else {
        Readiness::Ready(format!("db-sync is at the tip ({lag} block(s) behind)"))
    }
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

/// The highest block `cardano-db-sync` has written
pub async fn db_sync_block(credentials: &DatabaseCredentials) -> anyhow::Result<u64> {
    let value = psql_query(credentials, "SELECT COALESCE(MAX(block_no),0) FROM block;").await?;

    value
        .parse::<u64>()
        .with_context(|| format!("`cardano-db-sync` returned `{value}` as its latest block"))
}

pub fn is_root() -> bool {
    nix::unistd::Uid::effective().is_root()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn a_relay_short_of_the_tip_is_waiting() {
        let tip = CardanoTip {
            sync_progress: 57.3,
            block: 1_000,
        };
        assert!(!cardano_readiness(&tip).is_ready());

        let tip = CardanoTip {
            sync_progress: 100.0,
            block: 1_000,
        };
        assert!(cardano_readiness(&tip).is_ready());
    }

    #[test]
    fn db_sync_is_ready_within_the_allowed_lag() {
        let tip = CardanoTip {
            sync_progress: 100.0,
            block: 1_000,
        };
        assert!(db_sync_readiness(&tip, 1_000 - DB_SYNC_MAX_LAG).is_ready());
        assert!(!db_sync_readiness(&tip, 1_000 - DB_SYNC_MAX_LAG - 1).is_ready());
        // db-sync a block ahead of a tip read a moment earlier is not "behind"
        assert!(db_sync_readiness(&tip, 1_001).is_ready());
    }
}
