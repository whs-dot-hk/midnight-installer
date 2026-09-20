/*! The systemd units for the three services an FNO host runs

They are rendered from settings rather than kept as files, so a unit can never drift from
the paths and user the rest of the plan uses.
*/

use crate::credentials::DatabaseCredentials;
use crate::settings::{CommonSettings, TELEMETRY_URL};

pub fn cardano_node(settings: &CommonSettings) -> anyhow::Result<String> {
    let paths = settings.paths();
    let user = &settings.cardano_user;
    let bin_dir = crate::settings::user_bin_dir(user)?;
    let config_dir = crate::settings::user_share_dir(user)?.join(&settings.cardano_network);

    Ok(format!(
        "\
[Unit]
Description=Cardano Relay Node ({network})
Wants=network-online.target
After=network-online.target

[Service]
User={user}
Type=simple
WorkingDirectory={working_directory}
ExecStart={bin_dir}/cardano-node run \\
    --topology {config_dir}/topology.json \\
    --database-path {database_path} \\
    --socket-path {socket_path} \\
    --host-addr 0.0.0.0 \\
    --port 3001 \\
    --config {config_dir}/config.json
KillSignal=SIGINT
Restart=always
RestartSec=5
LimitNOFILE=32768

[Install]
WantedBy=multi-user.target
",
        network = settings.cardano_network,
        working_directory = paths.cardano_data.display(),
        bin_dir = bin_dir.display(),
        config_dir = config_dir.display(),
        database_path = paths.cardano_db.display(),
        socket_path = paths.cardano_socket.display(),
    ))
}

pub fn cardano_db_sync(
    settings: &CommonSettings,
    credentials: &DatabaseCredentials,
) -> anyhow::Result<String> {
    let paths = settings.paths();
    let user = &settings.cardano_user;
    let bin_dir = crate::settings::user_bin_dir(user)?;
    let pgpass = &paths.postgres_pgpass_file;

    Ok(format!(
        "\
[Unit]
Description=Cardano DB Sync ({network})
After=cardano-node.service postgresql.service
Requires=cardano-node.service postgresql.service

[Service]
User={user}
Type=simple
Environment=\"PGPASSFILE={pgpass}\"
Environment=\"PGHOST=/var/run/postgresql\"
Environment=\"PGPORT=5432\"
Environment=\"PGUSER={db_user}\"
Environment=\"PGDATABASE={db_name}\"
WorkingDirectory={working_directory}
ExecStart={bin_dir}/cardano-db-sync \\
    --config {config} \\
    --socket-path {socket_path} \\
    --schema-dir {schema_dir} \\
    --state-dir {state_dir}
KillSignal=SIGINT
Restart=always
RestartSec=10
LimitNOFILE=32768

[Install]
WantedBy=multi-user.target
",
        network = settings.cardano_network,
        pgpass = pgpass.display(),
        db_user = credentials.user,
        db_name = credentials.name,
        working_directory = paths.cardano_data.display(),
        bin_dir = bin_dir.display(),
        config = paths.cardano_db_sync_config.display(),
        socket_path = paths.cardano_socket.display(),
        schema_dir = paths.cardano_schema.display(),
        state_dir = paths.cardano_db_sync_state.display(),
    ))
}

pub fn midnight_node(settings: &CommonSettings) -> anyhow::Result<String> {
    let paths = settings.paths();
    let user = &settings.midnight_user;
    let bin_dir = crate::settings::user_bin_dir(user)?;

    Ok(format!(
        "\
[Unit]
Description=Midnight Protocol Node ({network} FNO)
After=network-online.target postgresql.service cardano-db-sync.service
Wants=network-online.target postgresql.service cardano-db-sync.service
# The node is started as soon as it is installed, which may be long before db-sync has
# filled the database it reads. It fails and retries until then, so systemd must not give
# up on it for restarting too often.
StartLimitIntervalSec=0

[Service]
User={user}
Group={user}
Type=simple
WorkingDirectory={working_directory}
EnvironmentFile={env_file}
ExecStart={bin_dir}/midnight-node \\
    --chain {chain_spec} \\
    --base-path {base_path} \\
    --node-key-file {node_key_file} \\
    --keystore-path {keystore_path} \\
    --telemetry-url '{telemetry_url}' \\
    --validator \\
    --pool-limit 35 \\
    --name ${{NODE_NAME}} \\
    --rpc-port 9933
Restart=on-failure
RestartSec=10
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
",
        network = settings.cardano_network,
        working_directory = paths.midnight_node_data.display(),
        env_file = paths.midnight_env_file.display(),
        bin_dir = bin_dir.display(),
        chain_spec = paths.midnight_chain_spec.display(),
        base_path = paths.midnight_runtime_data.display(),
        // The node derives both of these from `--base-path` unless it is told otherwise.
        // They live under the secret root, so they have to be named: without them the node
        // silently falls back and writes a keystore of real private keys under the base
        // path. `Validator` checks after starting that they really did reach the process.
        node_key_file = paths.midnight_network_dir.join("secret_ed25519").display(),
        keystore_path = paths.midnight_keystore_dir.display(),
        telemetry_url = TELEMETRY_URL,
    ))
}

#[cfg(test)]
mod test {
    use super::*;

    /** The two flags which stop the node falling back to `--base-path`

    Without them `midnight-node` derives its keystore and network key from `--base-path` and
    writes private keys there, outside the secret root. They have been dropped once already,
    by a systemd drop-in holding a stale copy of `ExecStart`, and a host ran that way for
    three days. `VerifySecretFlags` catches the drop-in; this catches the unit.
    */
    /// `cardano-db-sync` cannot find a `.pgpass` outside `$HOME` on its own
    #[test]
    fn the_db_sync_unit_points_pgpassfile_at_the_secret_root() {
        let settings = CommonSettings {
            cardano_user: String::from("root"),
            secret_root: std::path::PathBuf::from("/secret"),
            ..CommonSettings::default()
        };

        let credentials = crate::credentials::DatabaseCredentials::new(
            "midnight",
            "cexplorer",
            crate::settings::Secret::new("unused"),
        );

        let unit = cardano_db_sync(&settings, &credentials).unwrap();

        assert!(unit.contains("PGPASSFILE=/secret/pgpass"), "{unit}");
    }

    #[test]
    fn the_validator_unit_names_the_secret_root_paths() {
        let settings = CommonSettings {
            midnight_user: String::from("root"),
            secret_root: std::path::PathBuf::from("/secret"),
            ..CommonSettings::default()
        };

        let unit = midnight_node(&settings).unwrap();

        assert!(
            unit.contains("--node-key-file /secret/node/secret_ed25519"),
            "{unit}"
        );
        assert!(unit.contains("--keystore-path /secret/keystore"), "{unit}");
    }
}
