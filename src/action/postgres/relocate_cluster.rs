use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};
use crate::settings::CommonSettings;
use crate::util::{chown_recursive, gid_of, uid_of};

/// The tuning the FNO programme recommends for a Preprod sized db-sync workload
const TUNING: &[(&str, &str)] = &[
    ("shared_buffers", "4GB"),
    ("maintenance_work_mem", "1GB"),
    ("max_parallel_maintenance_workers", "2"),
    ("effective_cache_size", "12GB"),
    ("join_collapse_limit", "1"),
];

/** Move the PostgreSQL cluster onto the data disk, and tune it for db-sync

The packaged cluster lives on the root filesystem, which on an FNO host is far too small for
a Cardano chain database. This copies it under the data root, points `data_directory` at the
copy, and applies the tuning.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "relocate_postgres_cluster")]
pub struct RelocatePostgresCluster {
    version: String,
    source: PathBuf,
    target: PathBuf,
    config_path: PathBuf,
}

impl RelocatePostgresCluster {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let version = settings.postgres_version.clone();
        let paths = settings.paths();

        Ok(StatefulAction::uncompleted(Self {
            source: PathBuf::from(format!("/var/lib/postgresql/{version}/main")),
            target: paths.postgres_cluster(&version),
            config_path: PathBuf::from(format!("/etc/postgresql/{version}/main/postgresql.conf")),
            version,
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "relocate_postgres_cluster")]
impl Action for RelocatePostgresCluster {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Move the PostgreSQL {version} cluster to `{target}` and tune it",
            version = self.version,
            target = self.target.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "relocate_postgres_cluster",
            version = self.version,
            target = tracing::field::display(self.target.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let mut explanation = vec![
            format!("Stop PostgreSQL {}", self.version),
            format!(
                "Copy `{source}` to `{target}` if the target is empty",
                source = self.source.display(),
                target = self.target.display()
            ),
            format!("Point `data_directory` at `{}`", self.target.display()),
        ];
        for (key, value) in TUNING {
            explanation.push(format!("Set `{key} = {value}`"));
        }
        explanation.push(String::from("Start PostgreSQL again"));

        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            version,
            source,
            target,
            config_path,
        } = self;

        if !config_path.is_file() {
            anyhow::bail!(
                "The PostgreSQL configuration `{}` does not exist",
                config_path.display()
            );
        }

        // Both, because which one is in charge depends on how the cluster was started
        let _ = crate::command_succeeds(crate::command("systemctl").arg("stop").arg("postgresql"))
            .await;
        let _ = crate::command_succeeds(
            crate::command("pg_ctlcluster")
                .arg(&*version)
                .arg("main")
                .arg("stop"),
        )
        .await;

        tokio::fs::create_dir_all(&target)
            .await
            .with_context(|| format!("Creating directory `{}`", target.display()))?;

        if !target.join("PG_VERSION").is_file() {
            if !source.join("PG_VERSION").is_file() {
                anyhow::bail!(
                    "The packaged PostgreSQL cluster `{}` is missing",
                    source.display()
                );
            }

            tracing::info!("Copying the PostgreSQL cluster to `{}`", target.display());
            crate::execute_command(
                crate::command("rsync")
                    .arg("-aHAX")
                    .arg(format!("{}/", source.display()))
                    .arg(format!("{}/", target.display())),
            )
            .await?;
        } else {
            tracing::info!("PostgreSQL data already exists in `{}`", target.display());
        }

        // The cluster and its version directory, not the whole PostgreSQL data root: the
        // root-only credentials file lives there too
        let uid = uid_of("postgres")?;
        let gid = gid_of("postgres").ok();
        chown_recursive(target.parent().unwrap_or(target), Some(uid), gid)?;
        tokio::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .await
            .with_context(|| format!("Setting mode `{:#o}` on `{}`", 0o700, target.display()))?;

        let config = tokio::fs::read_to_string(&config_path)
            .await
            .with_context(|| format!("Reading `{}`", config_path.display()))?;

        let mut updated = set_config_key(
            &config,
            "data_directory",
            &format!("'{}'", target.display()),
        );
        for (key, value) in TUNING {
            updated = set_config_key(&updated, key, value);
        }

        if updated != config {
            crate::util::write_owned_file(config_path, &updated, 0o644, Some("postgres")).await?;
        }

        crate::execute_command(crate::command("systemctl").arg("start").arg("postgresql")).await?;

        crate::execute_command(
            crate::command("pg_isready")
                .arg("-h")
                .arg("/var/run/postgresql")
                .arg("-p")
                .arg("5432"),
        )
        .await?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!(
                "Leave the PostgreSQL cluster in `{}`",
                self.target.display()
            ),
            vec![String::from(
                "Moving a cluster back would risk the database it holds; do it by hand if it is really wanted",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            "Leaving the PostgreSQL cluster in `{}`",
            self.target.display()
        );
        Ok(())
    }
}

/// Set `key = value` in a `postgresql.conf`, replacing the existing (or commented out)
/// setting in place, or appending it when there is none
fn set_config_key(config: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut out = String::with_capacity(config.len());

    for line in config.lines() {
        if is_setting_for(line, key) {
            found = true;
            out.push_str(&format!("{key} = {value}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    if !found {
        out.push_str(&format!("{key} = {value}\n"));
    }

    out
}

fn is_setting_for(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start().trim_start_matches('#').trim_start();
    match trimmed.strip_prefix(key) {
        Some(rest) => rest.trim_start().starts_with('='),
        None => false,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn replaces_an_existing_setting() {
        let config = "# comment\nshared_buffers = 128MB\nwork_mem = 4MB\n";
        let updated = set_config_key(config, "shared_buffers", "4GB");
        assert_eq!(updated, "# comment\nshared_buffers = 4GB\nwork_mem = 4MB\n");
    }

    #[test]
    fn replaces_a_commented_out_setting() {
        let config = "#data_directory = '/var/lib/postgresql/17/main'\n";
        let updated = set_config_key(config, "data_directory", "'/data/postgresql/17/main'");
        assert_eq!(updated, "data_directory = '/data/postgresql/17/main'\n");
    }

    #[test]
    fn appends_a_missing_setting() {
        let updated = set_config_key("work_mem = 4MB\n", "join_collapse_limit", "1");
        assert_eq!(updated, "work_mem = 4MB\njoin_collapse_limit = 1\n");
    }

    #[test]
    fn does_not_match_a_different_setting_with_the_same_prefix() {
        let config = "shared_buffers_extra = 1\n";
        let updated = set_config_key(config, "shared_buffers", "4GB");
        assert_eq!(updated, "shared_buffers_extra = 1\nshared_buffers = 4GB\n");
    }
}
