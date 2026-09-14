use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::base::{FetchAndUnpackTarball, InstallBinary, InstallTree};
use crate::action::{Action, ActionDescription, StatefulAction};
use crate::settings::CommonSettings;

/** Install `cardano-node`, `cardano-cli`, and the network's configuration

The release archive carries the topology and config for each network under
`share/<network>`, so the binaries and the configuration they are run with always come from
the same release.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_cardano_node")]
pub struct InstallCardanoNode {
    version: String,
    user: String,
    binary_path: PathBuf,
    fetch: StatefulAction<FetchAndUnpackTarball>,
    node_binary: StatefulAction<InstallBinary>,
    cli_binary: StatefulAction<InstallBinary>,
    configuration: StatefulAction<InstallTree>,
}

impl InstallCardanoNode {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();
        let user = settings.cardano_user.clone();
        let bin_dir = crate::settings::user_bin_dir(&user)?;
        let share_dir = crate::settings::user_share_dir(&user)?;
        let scratch = paths.scratch("cardano-node");
        let binary_path = bin_dir.join("cardano-node");

        let fetch = FetchAndUnpackTarball::plan(settings.cardano_node_archive()?, &scratch).await?;
        let node_binary =
            InstallBinary::plan(&scratch, "cardano-node", &binary_path, user.clone()).await?;
        let cli_binary = InstallBinary::plan(
            &scratch,
            "cardano-cli",
            bin_dir.join("cardano-cli"),
            user.clone(),
        )
        .await?;
        let configuration = InstallTree::plan(
            &scratch,
            format!("share/{network}", network = settings.cardano_network),
            share_dir.join(&settings.cardano_network),
            user.clone(),
            user.clone(),
        )
        .await?;

        Ok(StatefulAction::uncompleted(Self {
            version: settings.cardano_node_version.clone(),
            user,
            binary_path,
            fetch,
            node_binary,
            cli_binary,
            configuration,
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_cardano_node")]
impl Action for InstallCardanoNode {
    fn tracing_synopsis(&self) -> String {
        format!("Install cardano-node {}", self.version)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_cardano_node",
            version = self.version,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let explanation = [
            self.fetch.describe_execute(),
            self.node_binary.describe_execute(),
            self.cli_binary.describe_execute(),
            self.configuration.describe_execute(),
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
        self.node_binary.try_execute().await?;
        self.cli_binary.try_execute().await?;
        self.configuration.try_execute().await?;

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
        let explanation = [
            self.configuration.describe_revert(),
            self.cli_binary.describe_revert(),
            self.node_binary.describe_revert(),
            self.fetch.describe_revert(),
        ]
        .concat()
        .into_iter()
        .map(|description| description.description)
        .collect();

        vec![ActionDescription::new(
            format!("Remove cardano-node {}", self.version),
            explanation,
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        let mut errors = vec![];

        for result in [
            self.configuration.try_revert().await,
            self.cli_binary.try_revert().await,
            self.node_binary.try_revert().await,
            self.fetch.try_revert().await,
        ] {
            if let Err(e) = result {
                errors.push(e);
            }
        }

        crate::action::fold_errors(errors)
    }
}
