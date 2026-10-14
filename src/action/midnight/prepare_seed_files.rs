use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::midnight::{seed_files, SEED_FILES};
use crate::action::{Action, ActionDescription, StatefulAction};

/** Write the bare secret phrases the node reads at start up

The node takes each seed as a file path in its environment, while the generated keys are
JSON. These files are derived from those, so unlike the keys themselves they can be deleted
and rebuilt, and revert does delete them.

They are not the keystore's own entries, although they are named like them: the node trims
a seed file but does not unquote it, and the keystore stores each phrase as a JSON string.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "prepare_seed_files")]
pub struct PrepareSeedFiles {
    keys_dir: PathBuf,
    seed_dir: PathBuf,
    user: String,
}

impl PrepareSeedFiles {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        keys_dir: impl Into<PathBuf>,
        seed_dir: impl Into<PathBuf>,
        user: impl Into<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            keys_dir: keys_dir.into(),
            seed_dir: seed_dir.into(),
            user: user.into(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "prepare_seed_files")]
impl Action for PrepareSeedFiles {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Write the validator seed files into `{}`",
            self.seed_dir.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "prepare_seed_files",
            seed_dir = tracing::field::display(self.seed_dir.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            SEED_FILES
                .iter()
                .map(|(variable, key_type, source)| {
                    format!("`{variable}`: `{key_type}` + public key, from `{source}`")
                })
                .collect(),
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            keys_dir,
            seed_dir,
            user,
        } = self;

        for (_, _, source) in SEED_FILES {
            let source_path = keys_dir.join(source);
            if !source_path.is_file() {
                anyhow::bail!(
                    "The validator key `{}` is missing, run the `midnight` step first",
                    source_path.display()
                );
            }
        }

        tokio::fs::create_dir_all(&seed_dir)
            .await
            .with_context(|| format!("Creating directory `{}`", seed_dir.display()))?;
        let uid = crate::util::uid_of(user)?;
        let gid = crate::util::gid_of(user).ok();
        nix::unistd::chown(seed_dir.as_path(), Some(uid), gid)
            .with_context(|| format!("Changing the owner of `{}`", seed_dir.display()))?;
        tokio::fs::set_permissions(
            &seed_dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .await
        .with_context(|| format!("Setting mode `{:#o}` on `{}`", 0o700, seed_dir.display()))?;

        for seed in seed_files(keys_dir, seed_dir).await? {
            crate::util::write_owned_file(
                &seed.path,
                &format!("{}\n", seed.phrase),
                0o600,
                Some(user),
            )
            .await?;
        }

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            String::from("Delete the validator seed files"),
            vec![String::from(
                "The keys they were derived from are left in place",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        // Named after the public keys, so without the keys there is nothing to find them by
        let seeds = match seed_files(&self.keys_dir, &self.seed_dir).await {
            Ok(seeds) => seeds,
            Err(e) => {
                tracing::warn!("Leaving the seed files in place: {e:#}");
                return Ok(());
            },
        };
        for seed in seeds {
            crate::util::remove_file_if_exists(&seed.path)
                .await
                .with_context(|| format!("Removing `{}`", seed.path.display()))?;
        }
        Ok(())
    }
}
