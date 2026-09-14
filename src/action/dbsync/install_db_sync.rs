use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};
use url::Url;

use crate::action::base::{FetchAndUnpackTarball, InstallBinary, InstallTree};
use crate::action::{Action, ActionDescription, StatefulAction};
use crate::settings::CommonSettings;

/** Install `cardano-db-sync`, its SQL schema, and its configuration

The published configuration points `NodeConfigFile` at wherever the packager's node config
lives; it is rewritten to the config this host's relay actually runs with, so db-sync and the
relay can never disagree about the network.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_cardano_db_sync")]
pub struct InstallCardanoDbSync {
    version: String,
    user: String,
    fetch: StatefulAction<FetchAndUnpackTarball>,
    binary: StatefulAction<InstallBinary>,
    schema: StatefulAction<InstallTree>,
    config_url: Url,
    config_path: PathBuf,
    node_config_path: PathBuf,
}

impl InstallCardanoDbSync {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();
        let user = settings.cardano_user.clone();
        let bin_dir = crate::settings::user_bin_dir(&user)?;
        let share_dir = crate::settings::user_share_dir(&user)?;
        let scratch = paths.scratch("cardano-db-sync");

        let fetch = FetchAndUnpackTarball::plan(settings.db_sync_archive()?, &scratch).await?;
        let binary = InstallBinary::plan(
            &scratch,
            "cardano-db-sync",
            bin_dir.join("cardano-db-sync"),
            user.clone(),
        )
        .await?;
        let schema = InstallTree::plan(
            &scratch,
            "schema",
            &paths.cardano_schema,
            user.clone(),
            user.clone(),
        )
        .await?;

        Ok(StatefulAction::uncompleted(Self {
            version: settings.db_sync_version.clone(),
            user,
            fetch,
            binary,
            schema,
            config_url: settings.db_sync_config_url()?,
            config_path: paths.cardano_db_sync_config,
            node_config_path: share_dir
                .join(&settings.cardano_network)
                .join("config.json"),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_cardano_db_sync")]
impl Action for InstallCardanoDbSync {
    fn tracing_synopsis(&self) -> String {
        format!("Install cardano-db-sync {}", self.version)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_cardano_db_sync",
            version = self.version,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let mut explanation: Vec<String> = [
            self.fetch.describe_execute(),
            self.binary.describe_execute(),
            self.schema.describe_execute(),
        ]
        .concat()
        .into_iter()
        .map(|description| description.description)
        .collect();
        explanation.push(format!(
            "Fetch `{url}` into `{path}`, pointing `NodeConfigFile` at `{node_config}`",
            url = self.config_url,
            path = self.config_path.display(),
            node_config = self.node_config_path.display(),
        ));

        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        self.fetch.try_execute().await?;
        self.binary.try_execute().await?;
        self.schema.try_execute().await?;

        let config = crate::util::fetch_string(&self.config_url).await?;
        let mut config: serde_json::Value = serde_json::from_str(&config)
            .with_context(|| format!("Parsing JSON of `{}`", self.config_path.display()))?;

        config["NodeConfigFile"] =
            serde_json::Value::String(self.node_config_path.display().to_string());

        let rendered = serde_json::to_string_pretty(&config).context("Serializing JSON")?;
        crate::util::write_owned_file(
            &self.config_path,
            &format!("{rendered}\n"),
            0o644,
            Some(&self.user),
        )
        .await?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Remove cardano-db-sync {}", self.version),
            vec![
                format!("Delete `{}`", self.config_path.display()),
                String::from("Remove the binary and the SQL schema"),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        let mut errors = vec![];

        if let Err(e) = crate::util::remove_file_if_exists(&self.config_path)
            .await
            .with_context(|| format!("Removing `{}`", self.config_path.display()))
        {
            errors.push(e);
        }

        for result in [
            self.schema.try_revert().await,
            self.binary.try_revert().await,
            self.fetch.try_revert().await,
        ] {
            if let Err(e) = result {
                errors.push(e);
            }
        }

        crate::action::fold_errors(errors)
    }
}
