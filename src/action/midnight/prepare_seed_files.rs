use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::midnight::read_key_field;
use crate::action::{Action, ActionDescription, StatefulAction};

/// The generated key files, and the seed file the node's environment points at
const SEEDS: &[(&str, &str)] = &[
    ("aura.json", "aura.seed"),
    ("grandpa.json", "grandpa.seed"),
    ("cross_chain.json", "cross_chain.seed"),
];

/** Write the bare secret phrases the node reads at start up

The node takes each seed as a file path in its environment, while the generated keys are
JSON. These files are derived from those, so unlike the keys themselves they can be deleted
and rebuilt, and revert does delete them.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "prepare_seed_files")]
pub struct PrepareSeedFiles {
    keys_dir: PathBuf,
    user: String,
}

impl PrepareSeedFiles {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        keys_dir: impl Into<PathBuf>,
        user: impl Into<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            keys_dir: keys_dir.into(),
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
            self.keys_dir.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "prepare_seed_files",
            keys_dir = tracing::field::display(self.keys_dir.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            SEEDS
                .iter()
                .map(|(source, seed)| format!("`{seed}`, from `{source}`"))
                .collect(),
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self { keys_dir, user } = self;

        for (source, seed) in SEEDS {
            let source_path = keys_dir.join(source);
            if !source_path.is_file() {
                anyhow::bail!(
                    "The validator key `{}` is missing, run the `midnight` step first",
                    source_path.display()
                );
            }

            let phrase = read_key_field(&source_path, "secretPhrase").await?;
            crate::util::write_owned_file(
                &keys_dir.join(seed),
                &format!("{phrase}\n"),
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
        for (_, seed) in SEEDS {
            let path = self.keys_dir.join(seed);
            crate::util::remove_file_if_exists(&path)
                .await
                .with_context(|| format!("Removing `{}`", path.display()))?;
        }
        Ok(())
    }
}
