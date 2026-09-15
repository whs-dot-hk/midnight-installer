use std::path::PathBuf;

use tracing::{span, Span};
use url::Url;

use crate::action::{planned, Action, ActionDescription, ActionState, StatefulAction};
use crate::settings::CommonSettings;
use crate::util::{chown_recursive, directory_has_contents, gid_of, uid_of};

/** Bootstrap the relay's chain database from a signed Mithril snapshot

Skipped when the database directory already holds data: re-downloading over a live (or
partially synced) chain database is never what is wanted, and the relay can always catch up
from where it is.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "fetch_mithril_snapshot")]
pub struct FetchMithrilSnapshot {
    user: String,
    network: String,
    cardano_data: PathBuf,
    cardano_db: PathBuf,
    tools_dir: PathBuf,
    aggregator_endpoint: String,
    genesis_vkey_url: Url,
    ancillary_vkey_url: Url,
}

impl FetchMithrilSnapshot {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();
        let this = Self {
            user: settings.cardano_user.clone(),
            network: settings.cardano_network.clone(),
            cardano_data: paths.cardano_data.clone(),
            cardano_db: paths.cardano_db.clone(),
            tools_dir: paths.mithril_tools,
            aggregator_endpoint: settings.mithril_aggregator_endpoint(),
            genesis_vkey_url: settings.mithril_genesis_vkey_url()?,
            ancillary_vkey_url: settings.mithril_ancillary_vkey_url()?,
        };

        let state = if directory_has_contents(&this.cardano_db).await {
            tracing::warn!(
                "`{}` already contains data, the snapshot download will be skipped",
                this.cardano_db.display()
            );
            ActionState::Skipped
        } else {
            ActionState::Uncompleted
        };

        Ok(planned(this, state))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "fetch_mithril_snapshot")]
impl Action for FetchMithrilSnapshot {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Download the latest Mithril {network} snapshot into `{db}`",
            network = self.network,
            db = self.cardano_db.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "fetch_mithril_snapshot",
            network = self.network,
            aggregator = self.aggregator_endpoint,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!("Aggregator: {}", self.aggregator_endpoint),
                String::from(
                    "Verified against the genesis and ancillary verification keys, so the relay starts near the tip instead of syncing from genesis",
                ),
                String::from("This transfers tens of gigabytes and takes a while"),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            user,
            network,
            cardano_data,
            cardano_db: _,
            tools_dir,
            aggregator_endpoint,
            genesis_vkey_url,
            ancillary_vkey_url,
        } = self;

        let genesis_key = crate::util::fetch_string(genesis_vkey_url).await?;
        let ancillary_key = crate::util::fetch_string(ancillary_vkey_url).await?;

        if genesis_key.trim().is_empty() || ancillary_key.trim().is_empty() {
            anyhow::bail!("Could not download the Mithril verification keys",);
        }

        let client = tools_dir.join("mithril-client");

        for arguments in [
            ["cardano-db", "snapshot", "list"].as_slice(),
            ["cardano-db", "download", "--include-ancillary", "latest"].as_slice(),
        ] {
            // `sudo` resets the environment, so the client's settings go through `env`;
            // the download lands in the working directory, which `sudo` keeps
            let mut command = crate::command_as(user, "env");
            command
                .arg(format!("CARDANO_NETWORK={network}"))
                .arg(format!("AGGREGATOR_ENDPOINT={aggregator_endpoint}"))
                .arg(format!("GENESIS_VERIFICATION_KEY={}", genesis_key.trim()))
                .arg(format!(
                    "ANCILLARY_VERIFICATION_KEY={}",
                    ancillary_key.trim()
                ))
                .arg(&client)
                .args(arguments)
                .current_dir(&*cardano_data);

            let output = crate::execute_command_stdout(&mut command).await?;
            tracing::info!("{}", output.trim());
        }

        if !self.cardano_db.is_dir() {
            anyhow::bail!(
                "The Mithril download finished but `{}` was not created",
                self.cardano_db.display()
            );
        }

        let uid = uid_of(user)?;
        let gid = gid_of(user).ok();
        chown_recursive(cardano_data, Some(uid), gid)?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!(
                "Leave the chain database in `{}` in place",
                self.cardano_db.display()
            ),
            vec![String::from(
                "Deleting a downloaded (or since extended) chain database would mean syncing it all over again",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            "Leaving the chain database `{}` in place; remove it by hand if that is really wanted",
            self.cardano_db.display()
        );
        Ok(())
    }
}
