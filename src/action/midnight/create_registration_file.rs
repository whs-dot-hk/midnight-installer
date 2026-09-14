use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::midnight::read_key_field;
use crate::action::{Action, ActionDescription, StatefulAction};

/** Write the public keys the Foundation needs in order to register this validator

Only public material, so unlike the keys themselves this file is disposable — it can be
rebuilt from the key files at any time.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "create_registration_file")]
pub struct CreateRegistrationFile {
    keys_dir: PathBuf,
    path: PathBuf,
    user: String,
}

impl CreateRegistrationFile {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        keys_dir: impl Into<PathBuf>,
        path: impl Into<PathBuf>,
        user: impl Into<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            keys_dir: keys_dir.into(),
            path: path.into(),
            user: user.into(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "create_registration_file")]
impl Action for CreateRegistrationFile {
    fn tracing_synopsis(&self) -> String {
        format!("Write the registration file `{}`", self.path.display())
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "create_registration_file",
            path = tracing::field::display(self.path.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![String::from(
                "The partner chains key and the `aura`, `crch` and `gran` public keys, which is what registration asks for",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            keys_dir,
            path,
            user,
        } = self;

        let aura = read_key_field(&keys_dir.join("aura.json"), "publicKey").await?;
        let grandpa = read_key_field(&keys_dir.join("grandpa.json"), "publicKey").await?;
        let cross_chain = read_key_field(&keys_dir.join("cross_chain.json"), "publicKey").await?;

        let registration = serde_json::json!({
            "partner_chains_key": cross_chain,
            "keys": {
                "aura": aura,
                "crch": cross_chain,
                "gran": grandpa,
            }
        });
        let rendered = serde_json::to_string_pretty(&registration).context("Serializing JSON")?;

        crate::util::write_owned_file(path, &format!("{rendered}\n"), 0o600, Some(user)).await?;

        tracing::info!("Validator registration file:\n{rendered}");

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Delete `{}`", self.path.display()),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        crate::util::remove_file_if_exists(&self.path)
            .await
            .with_context(|| format!("Removing `{}`", self.path.display()))?;
        Ok(())
    }
}
