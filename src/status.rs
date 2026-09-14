/*! A read-only report of where this host has got to

Nothing here changes the machine: it is the "what does this host look like right now" view
that makes the staged build-out navigable.
*/

use std::fmt::Write as _;

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

    let _ = writeln!(buf, "\n===== CARDANO RELAY =====");
    if crate::action::base::unit_exists(CARDANO_NODE_SERVICE).await {
        let _ = writeln!(buf, "{}", unit_state(CARDANO_NODE_SERVICE).await);
        match crate::check::cardano_tip(settings).await {
            Ok(tip) => {
                let _ = writeln!(
                    buf,
                    "Sync progress: {progress}% (block {block})",
                    progress = tip.sync_progress,
                    block = tip.block
                );
            },
            Err(e) => {
                let _ = writeln!(buf, "Tip unavailable: {e}");
            },
        }
    } else {
        let _ = writeln!(buf, "{CARDANO_NODE_SERVICE} is not installed");
    }

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
    if crate::action::base::unit_exists(CARDANO_DB_SYNC_SERVICE).await {
        let _ = writeln!(buf, "{}", unit_state(CARDANO_DB_SYNC_SERVICE).await);
        if let Ok(credentials) =
            crate::credentials::DatabaseCredentials::load(&paths.postgres_credentials_file).await
        {
            match crate::check::db_sync_block(&credentials).await {
                Ok(block) => {
                    let _ = writeln!(buf, "Latest block written: {block}");
                },
                Err(e) => {
                    let _ = writeln!(buf, "Database unavailable: {e}");
                },
            }
        }
    } else {
        let _ = writeln!(buf, "{CARDANO_DB_SYNC_SERVICE} is not installed");
    }

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

    if crate::action::base::unit_exists(MIDNIGHT_NODE_SERVICE).await {
        let _ = writeln!(buf, "{}", unit_state(MIDNIGHT_NODE_SERVICE).await);
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

async fn unit_state(unit: &str) -> String {
    let active = command_output("systemctl", &["is-active", unit])
        .await
        .unwrap_or_default();
    let enabled = command_output("systemctl", &["is-enabled", unit])
        .await
        .unwrap_or_default();

    format!(
        "{unit}: {active}, {enabled}",
        active = blank_as_unknown(active.trim()),
        enabled = blank_as_unknown(enabled.trim()),
    )
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
