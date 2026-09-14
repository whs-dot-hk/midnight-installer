use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::base::{FetchAndUnpackTarball, InstallBinary};
use crate::action::{Action, ActionDescription, StatefulAction};
use crate::settings::CommonSettings;

/** Install the Mithril client, which fetches and verifies Cardano database snapshots

It comes from a pinned, checksummed Mithril distribution archive, like every other binary
here, rather than from the upstream `curl | sh` installer, whose script and the "unstable"
build it fetches change under it.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_mithril_client")]
pub struct InstallMithrilClient {
    version: String,
    client_path: PathBuf,
    fetch: StatefulAction<FetchAndUnpackTarball>,
    binary: StatefulAction<InstallBinary>,
}

impl InstallMithrilClient {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();
        let scratch = paths.scratch("mithril");
        let client_path = paths.mithril_tools.join("mithril-client");

        let fetch = FetchAndUnpackTarball::plan(settings.mithril_archive()?, &scratch).await?;
        let binary = InstallBinary::plan(
            &scratch,
            "mithril-client",
            &client_path,
            settings.cardano_user.clone(),
        )
        .await?;

        Ok(StatefulAction::uncompleted(Self {
            version: settings.mithril_version.clone(),
            client_path,
            fetch,
            binary,
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_mithril_client")]
impl Action for InstallMithrilClient {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Install the Mithril {version} client to `{path}`",
            version = self.version,
            path = self.client_path.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_mithril_client",
            version = self.version,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let explanation = [
            self.fetch.describe_execute(),
            self.binary.describe_execute(),
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
        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Remove `{}`", self.client_path.display()),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        let mut errors = vec![];

        for result in [
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
