use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::base::{FetchAndUnpackTarball, InstallBinary, InstallTree};
use crate::action::{Action, ActionDescription, StatefulAction};
use crate::settings::CommonSettings;

/// Install the `midnight-node` binary and the chain resources shipped alongside it
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_midnight_node")]
pub struct InstallMidnightNode {
    version: String,
    user: String,
    binary_path: PathBuf,
    fetch: StatefulAction<FetchAndUnpackTarball>,
    binary: StatefulAction<InstallBinary>,
    resources: StatefulAction<InstallTree>,
}

impl InstallMidnightNode {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();
        let user = settings.midnight_user.clone();
        let bin_dir = crate::settings::user_bin_dir(&user)?;
        let scratch = paths.scratch("midnight-node");
        let binary_path = bin_dir.join("midnight-node");

        let fetch = FetchAndUnpackTarball::plan(settings.midnight_archive()?, &scratch).await?;
        let binary =
            InstallBinary::plan(&scratch, "midnight-node", &binary_path, user.clone()).await?;
        let resources = InstallTree::plan(
            &scratch,
            "res",
            &paths.midnight_res_dir,
            user.clone(),
            user.clone(),
        )
        .await?;

        Ok(StatefulAction::uncompleted(Self {
            version: settings.midnight_version.clone(),
            user,
            binary_path,
            fetch,
            binary,
            resources,
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_midnight_node")]
impl Action for InstallMidnightNode {
    fn tracing_synopsis(&self) -> String {
        format!("Install midnight-node {}", self.version)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_midnight_node",
            version = self.version,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let explanation = [
            self.fetch.describe_execute(),
            self.binary.describe_execute(),
            self.resources.describe_execute(),
        ]
        .concat()
        .into_iter()
        .map(|description| description.description)
        .collect();

        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        self.fetch.try_execute().await?;
        self.binary.try_execute().await?;
        self.resources.try_execute().await?;

        // As the service user: a freshly downloaded binary has no business running as root
        let binary = self.binary_path.display().to_string();
        let version =
            crate::execute_command_stdout(crate::command_as(&self.user, &binary).arg("--version"))
                .await?;
        if let Some(line) = version.lines().next() {
            tracing::info!("{line}");
        }

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Remove midnight-node {}", self.version),
            vec![String::from(
                "The validator keys and chain data are not touched",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        let mut errors = vec![];

        for result in [
            self.resources.try_revert().await,
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
