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
    let pgpass = crate::settings::user_home(user)?.join(".pgpass");

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
        telemetry_url = TELEMETRY_URL,
    ))
}
