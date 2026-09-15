/*! Actions for the PostgreSQL instance `cardano-db-sync` and the Midnight node share */

pub(crate) mod check_credentials;
pub(crate) mod create_database;
pub(crate) mod create_role;
pub(crate) mod install_postgresql;
pub(crate) mod relocate_cluster;
pub(crate) mod save_credentials;

use anyhow::Context;

pub use check_credentials::CheckDatabaseCredentials;
pub use create_database::CreatePostgresDatabase;
pub use create_role::CreatePostgresRole;
pub use install_postgresql::InstallPostgresql;
pub use relocate_cluster::RelocatePostgresCluster;
pub use save_credentials::{CreatePgpassFile, SaveDatabaseCredentials};

/// Run SQL as the `postgres` superuser, with the statement on stdin rather than in `argv`
/// where `ps` would show it (and where a failure would log it)
pub(crate) async fn psql_as_postgres(sql: &str) -> anyhow::Result<String> {
    let output = crate::execute_command_with_stdin(
        crate::command_as("postgres", "psql")
            .arg("-v")
            .arg("ON_ERROR_STOP=1"),
        sql.as_bytes(),
        "psql (statement on stdin)",
    )
    .await?;
    String::from_utf8(output.stdout).context("Output was not valid UTF-8")
}

/// Run a read-only query as the `postgres` superuser
pub(crate) async fn psql_query_as_postgres(sql: &str) -> anyhow::Result<String> {
    let stdout =
        crate::execute_command_stdout(crate::command_as("postgres", "psql").arg("-tAc").arg(sql))
            .await?;
    Ok(stdout.trim().to_string())
}
