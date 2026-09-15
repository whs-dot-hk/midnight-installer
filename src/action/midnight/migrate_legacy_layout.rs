use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::{planned, Action, ActionDescription, ActionState, StatefulAction};
use crate::settings::CommonSettings;
use crate::util::{chown_recursive, gid_of, uid_of};

const LEGACY_SEEDS: &[&str] = &["aura.seed", "grandpa.seed", "cross_chain.seed"];

/** Carry an older layout's chain identity and seeds into the current paths

Earlier revisions of this setup kept the Midnight chain data under `<data root>/midnight` and
the seeds in a `seeds` directory. Generating a fresh identity instead of moving the old one
would make the node a stranger to its peers, so anything found in the old places is copied
into the new ones (never the other way round, and never overwriting).
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "migrate_legacy_layout")]
pub struct MigrateLegacyLayout {
    user: String,
    legacy_chains: PathBuf,
    legacy_seeds: PathBuf,
    runtime_chains: PathBuf,
    network_key: PathBuf,
    legacy_network_key: PathBuf,
    keys_dir: PathBuf,
}

impl MigrateLegacyLayout {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(settings: &CommonSettings) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();
        let legacy_chains = paths.midnight_data.join("chains");
        let this = Self {
            user: settings.midnight_user.clone(),
            legacy_network_key: legacy_chains
                .join(format!("midnight_{}", settings.cardano_network))
                .join("network")
                .join("secret_ed25519"),
            legacy_chains,
            legacy_seeds: paths.midnight_node_data.join("seeds"),
            runtime_chains: paths.midnight_runtime_data.join("chains"),
            network_key: paths.midnight_network_dir.join("secret_ed25519"),
            keys_dir: paths.midnight_keys_dir,
        };

        let state = if this.legacy_chains.is_dir() || this.legacy_seeds.is_dir() {
            ActionState::Uncompleted
        } else {
            ActionState::Skipped
        };

        Ok(planned(this, state))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "migrate_legacy_layout")]
impl Action for MigrateLegacyLayout {
    fn tracing_synopsis(&self) -> String {
        String::from("Carry an earlier layout's Midnight identity into the current paths")
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "migrate_legacy_layout",
            legacy_chains = tracing::field::display(self.legacy_chains.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!(
                    "Copy `{from}` to `{to}` if the new path has none",
                    from = self.legacy_chains.display(),
                    to = self.runtime_chains.display()
                ),
                format!(
                    "Copy any seeds from `{}` into the keys directory",
                    self.legacy_seeds.display()
                ),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            user,
            legacy_chains,
            legacy_seeds,
            runtime_chains,
            network_key,
            legacy_network_key,
            keys_dir,
        } = self;

        let uid = uid_of(user)?;
        let gid = gid_of(user).ok();

        if legacy_chains.is_dir() {
            if !runtime_chains.exists() {
                tracing::info!(
                    "Moving the existing Midnight identity into `{}`",
                    runtime_chains.display()
                );
                if let Some(parent) = runtime_chains.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .with_context(|| format!("Creating directory `{}`", parent.display()))?;
                }
                crate::util::copy_dir_all(legacy_chains, runtime_chains).await?;
                chown_recursive(runtime_chains, Some(uid), gid)?;
            } else if legacy_network_key.is_file() && !network_key.exists() {
                tracing::info!("Preserving the existing Midnight network key");
                if let Some(parent) = network_key.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .with_context(|| format!("Creating directory `{}`", parent.display()))?;
                }
                tokio::fs::copy(&legacy_network_key, &network_key)
                    .await
                    .with_context(|| {
                        format!(
                            "Copying `{}` to `{}`",
                            legacy_network_key.display(),
                            network_key.display()
                        )
                    })?;
                nix::unistd::chown(network_key.as_path(), Some(uid), gid).with_context(|| {
                    format!("Changing the owner of `{}`", network_key.display())
                })?;
                tokio::fs::set_permissions(
                    &network_key,
                    std::os::unix::fs::PermissionsExt::from_mode(0o600),
                )
                .await
                .with_context(|| {
                    format!("Setting mode `{:#o}` on `{}`", 0o600, network_key.display())
                })?;
            }
        }

        if legacy_seeds.is_dir() {
            for seed in LEGACY_SEEDS {
                let from = legacy_seeds.join(seed);
                let to = keys_dir.join(seed);
                if !from.is_file() || to.exists() {
                    continue;
                }

                tracing::info!("Preserving the existing seed `{seed}`");
                tokio::fs::copy(&from, &to).await.with_context(|| {
                    format!("Copying `{}` to `{}`", from.display(), to.display())
                })?;
                nix::unistd::chown(to.as_path(), Some(uid), gid)
                    .with_context(|| format!("Changing the owner of `{}`", to.display()))?;
                tokio::fs::set_permissions(
                    &to,
                    std::os::unix::fs::PermissionsExt::from_mode(0o600),
                )
                .await
                .with_context(|| format!("Setting mode `{:#o}` on `{}`", 0o600, to.display()))?;
            }
        }

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            String::from("Leave the migrated identity in place"),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::debug!("Leaving the migrated Midnight identity in place");
        Ok(())
    }
}
