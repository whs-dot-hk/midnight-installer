/*! A read-only report of where this host has got to

Nothing here changes the machine: it is the "what does this host look like right now" view
that makes the staged build-out navigable. Installing does not wait for the host to catch up,
so the `READINESS` section at the end is where an operator finds out whether it has, and
whether anything is broken rather than merely behind.

It reads root-only files and queries the relay as its service user, so it runs as root.
*/

use std::fmt::Write as _;

use crate::check::{CardanoTip, Readiness};
use crate::credentials::DatabaseCredentials;
use crate::settings::{
    CommonSettings, CARDANO_DB_SYNC_SERVICE, CARDANO_NODE_SERVICE, MIDNIGHT_NODE_SERVICE,
};

/// Build the status report
pub async fn report(settings: &CommonSettings) -> String {
    let paths = settings.paths();
    let mut buf = String::new();

    let _ = writeln!(buf, "===== DIRECTORY LAYOUT =====");
    for directory in paths.base_directories() {
        let mark = if directory.is_dir() {
            "[OK]  "
        } else {
            "[MISS]"
        };
        let _ = writeln!(buf, "{mark} {}", directory.display());
    }

    // Each fact about the machine is fetched once, here, and the readiness at the end is
    // derived from what was fetched: the tip query alone can block for twenty seconds on a
    // relay which is up but not answering
    let _ = writeln!(buf, "\n===== CARDANO RELAY =====");
    let relay = Service::inspect(CARDANO_NODE_SERVICE).await;
    let tip = if relay.installed {
        let _ = writeln!(buf, "{relay}");
        let tip = crate::check::cardano_tip(settings).await;
        match &tip {
            Ok(tip) => {
                let _ = writeln!(
                    buf,
                    "Sync progress: {progress}% (block {block})",
                    progress = tip.sync_progress,
                    block = tip.block
                );
            },
            Err(e) => {
                let _ = writeln!(buf, "Tip unavailable: {e:#}");
            },
        }
        Some(tip)
    } else {
        let _ = writeln!(buf, "{CARDANO_NODE_SERVICE} is not installed");
        None
    };

    let _ = writeln!(buf, "\n===== POSTGRESQL =====");
    match command_output("psql", &["--version"]).await {
        Some(version) => {
            let _ = writeln!(buf, "{}", version.trim());
            let ready = crate::command_succeeds(
                crate::command("pg_isready")
                    .arg("-h")
                    .arg("/var/run/postgresql")
                    .arg("-p")
                    .arg("5432"),
            )
            .await;
            let _ = writeln!(
                buf,
                "Accepting connections: {}",
                if ready { "yes" } else { "no" }
            );

            if let Ok(data_directory) =
                crate::action::postgres::psql_query_as_postgres("SHOW data_directory;").await
            {
                let _ = writeln!(buf, "Data directory: {}", data_directory.trim());
            }
        },
        None => {
            let _ = writeln!(buf, "PostgreSQL is not installed");
        },
    }

    let _ = writeln!(buf, "\n===== CARDANO DB SYNC =====");
    let db_sync = Service::inspect(CARDANO_DB_SYNC_SERVICE).await;
    let credentials = DatabaseCredentials::load(&paths.postgres_credentials_file).await;
    let db_block = if db_sync.installed {
        let _ = writeln!(buf, "{db_sync}");
        match &credentials {
            Ok(credentials) => {
                let db_block = crate::check::db_sync_block(credentials).await;
                match &db_block {
                    Ok(block) => {
                        let _ = writeln!(buf, "Latest block written: {block}");
                    },
                    Err(e) => {
                        let _ = writeln!(buf, "Database unavailable: {e:#}");
                    },
                }
                Some(db_block)
            },
            Err(_) => None,
        }
    } else {
        let _ = writeln!(buf, "{CARDANO_DB_SYNC_SERVICE} is not installed");
        None
    };

    let _ = writeln!(buf, "\n===== MIDNIGHT =====");
    match crate::settings::user_bin_dir(&settings.midnight_user) {
        Ok(bin_dir) => {
            let binary = bin_dir.join("midnight-node");
            if binary.is_file() {
                if let Some(version) =
                    command_output(&binary.display().to_string(), &["--version"]).await
                {
                    let _ = writeln!(buf, "{}", version.lines().next().unwrap_or("").trim());
                }
            } else {
                let _ = writeln!(buf, "midnight-node is not installed");
            }
        },
        Err(e) => {
            let _ = writeln!(buf, "{e}");
        },
    }

    for path in [
        paths.midnight_keys_dir.join("aura.json"),
        paths.midnight_keys_dir.join("grandpa.json"),
        paths.midnight_keys_dir.join("cross_chain.json"),
        paths.midnight_network_dir.join("secret_ed25519"),
        paths.midnight_registration_file.clone(),
    ] {
        let mark = if path.is_file() { "[OK]  " } else { "[MISS]" };
        let _ = writeln!(buf, "{mark} {}", path.display());
    }

    let node = Service::inspect(MIDNIGHT_NODE_SERVICE).await;
    if node.installed {
        let _ = writeln!(buf, "{node}");
    } else {
        let _ = writeln!(buf, "{MIDNIGHT_NODE_SERVICE} is not installed");
    }

    let _ = writeln!(buf, "\n===== WIREGUARD =====");
    match crate::action::wireguard::installed_version().await {
        Some(version) => {
            let _ = writeln!(buf, "wireguard-tools {version}");
        },
        None => {
            let _ = writeln!(buf, "wg is not installed");
        },
    }
    let public_key = paths.wireguard_data.join("publickey");
    match tokio::fs::read_to_string(&public_key).await {
        Ok(key) if !key.trim().is_empty() => {
            let _ = writeln!(buf, "Public key: {}", key.trim());
        },
        _ => {
            let _ = writeln!(buf, "No WireGuard identity has been generated yet");
        },
    }

    let _ = writeln!(buf, "\n===== READINESS =====");
    let relay_verdict = relay_readiness(&relay, tip.as_ref());
    let _ = writeln!(buf, "{relay_verdict}");
    let _ = writeln!(
        buf,
        "{}",
        db_sync_readiness(
            &db_sync,
            &credentials,
            db_block.as_ref(),
            tip.as_ref(),
            &paths.postgres_credentials_file,
        )
    );

    let _ = writeln!(buf, "\n===== RECEIPTS =====");
    match tokio::fs::read_dir(&paths.receipt_dir).await {
        Ok(mut entries) => {
            let mut found = false;
            while let Ok(Some(entry)) = entries.next_entry().await {
                found = true;
                let _ = writeln!(buf, "{}", entry.path().display());
            }
            if !found {
                let _ = writeln!(buf, "No steps have been applied on this host yet");
            }
        },
        Err(_) => {
            let _ = writeln!(buf, "No steps have been applied on this host yet");
        },
    }

    buf
}

/// A systemd unit as the host has it: installed or not, and what `systemctl` says of it
struct Service {
    unit: &'static str,
    installed: bool,
    active: String,
    enabled: String,
}

impl Service {
    async fn inspect(unit: &'static str) -> Self {
        let installed = crate::action::base::unit_exists(unit).await;
        let active = command_output("systemctl", &["is-active", unit])
            .await
            .unwrap_or_default();
        let enabled = command_output("systemctl", &["is-enabled", unit])
            .await
            .unwrap_or_default();

        Self {
            unit,
            installed,
            active: active.trim().to_string(),
            enabled: enabled.trim().to_string(),
        }
    }

    fn is_active(&self) -> bool {
        self.active == "active"
    }
}

impl std::fmt::Display for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{unit}: {active}, {enabled}",
            unit = self.unit,
            active = blank_as_unknown(&self.active),
            enabled = blank_as_unknown(&self.enabled),
        )
    }
}

/// One line of the `READINESS` section
///
/// `Missing` is a stage which has not been installed, `Failed` one which is installed but
/// broken, and the other two are the states of a service which is working.
enum Verdict {
    Ready(String),
    Waiting(String),
    Missing(String),
    Failed(String),
}

impl From<Readiness> for Verdict {
    fn from(readiness: Readiness) -> Self {
        match readiness {
            Readiness::Ready(why) => Self::Ready(why),
            Readiness::Waiting(why) => Self::Waiting(why),
        }
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready(why) => write!(f, "[OK]   {why}"),
            Self::Waiting(why) => write!(f, "[WAIT] {why}"),
            Self::Missing(why) => write!(f, "[MISS] {why}"),
            Self::Failed(why) => write!(f, "[FAIL] {why}"),
        }
    }
}

fn relay_readiness(relay: &Service, tip: Option<&anyhow::Result<CardanoTip>>) -> Verdict {
    if !relay.installed {
        return Verdict::Missing(format!(
            "{CARDANO_NODE_SERVICE} is not installed, run the `cardano` step"
        ));
    }
    if !relay.is_active() {
        return Verdict::Failed(format!(
            "{CARDANO_NODE_SERVICE} is {state}, so the relay cannot sync; see `journalctl -u {CARDANO_NODE_SERVICE}`",
            state = blank_as_unknown(&relay.active),
        ));
    }
    match tip {
        Some(Ok(tip)) => crate::check::cardano_readiness(tip).into(),
        Some(Err(e)) => Verdict::Failed(format!(
            "The relay is running but its tip could not be read: {e:#}"
        )),
        None => Verdict::Failed(String::from("The relay's tip was not queried")),
    }
}

fn db_sync_readiness(
    db_sync: &Service,
    credentials: &anyhow::Result<DatabaseCredentials>,
    db_block: Option<&anyhow::Result<u64>>,
    tip: Option<&anyhow::Result<CardanoTip>>,
    credentials_file: &std::path::Path,
) -> Verdict {
    if !db_sync.installed {
        return Verdict::Missing(format!(
            "{CARDANO_DB_SYNC_SERVICE} is not installed, run the `db-sync` step"
        ));
    }
    if let Err(e) = credentials {
        // The file is root-only, so a permission error is about who is asking, not about
        // whether the stage ran
        return match e.downcast_ref::<std::io::Error>().map(|io| io.kind()) {
            Some(std::io::ErrorKind::NotFound) => Verdict::Missing(format!(
                "The database credentials `{}` have not been saved, so db-sync cannot be checked; run the `db-sync` step",
                credentials_file.display(),
            )),
            Some(std::io::ErrorKind::PermissionDenied) => Verdict::Failed(format!(
                "The database credentials `{}` are root-only; run `status` with `sudo`",
                credentials_file.display(),
            )),
            _ => Verdict::Failed(format!("The database credentials could not be read: {e:#}")),
        };
    }
    if !db_sync.is_active() {
        return Verdict::Failed(format!(
            "{CARDANO_DB_SYNC_SERVICE} is {state}, so db-sync cannot catch up; see `journalctl -u {CARDANO_DB_SYNC_SERVICE}`",
            state = blank_as_unknown(&db_sync.active),
        ));
    }
    let db_block = match db_block {
        Some(Ok(block)) => *block,
        Some(Err(e)) => {
            return Verdict::Failed(format!(
                "db-sync is running but its database could not be queried: {e:#}"
            ))
        },
        None => return Verdict::Failed(String::from("The db-sync database was not queried")),
    };
    match tip {
        Some(Ok(tip)) => crate::check::db_sync_readiness(tip, db_block).into(),
        _ => Verdict::Waiting(format!(
            "db-sync has written block {db_block}, but without the relay's tip it cannot be told how far behind that is"
        )),
    }
}

fn blank_as_unknown(value: &str) -> &str {
    if value.is_empty() {
        "unknown"
    } else {
        value
    }
}

/// The stdout of a command which is only being asked a question, or `None` if it could not
/// be asked
async fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = crate::command(program).args(args).output().await.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if stdout.trim().is_empty() {
        // `systemctl is-active` reports through its exit status and stdout both; other
        // commands may say nothing at all on failure
        None
    } else {
        Some(stdout)
    }
}
