use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::midnight::{read_key_field, NodeInvocation};
use crate::action::{Action, ActionDescription, StatefulAction};

use super::generate_keys::KeyScheme;

/// The keystore entries the node expects: key type, the scheme it was generated with, and
/// the file the secret phrase comes from
const KEYSTORE_ENTRIES: &[(&str, KeyScheme, &str)] = &[
    ("aura", KeyScheme::Sr25519, "aura.json"),
    ("gran", KeyScheme::Ed25519, "grandpa.json"),
    ("beef", KeyScheme::Ecdsa, "cross_chain.json"),
];

/// Insert the validator keys into the node's keystore, which is what lets it sign
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "configure_keystore")]
pub struct ConfigureKeystore {
    keystore_dir: PathBuf,
    keys_dir: PathBuf,
    node: NodeInvocation,
}

impl ConfigureKeystore {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        keystore_dir: impl Into<PathBuf>,
        keys_dir: impl Into<PathBuf>,
        node: NodeInvocation,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            keystore_dir: keystore_dir.into(),
            keys_dir: keys_dir.into(),
            node,
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "configure_keystore")]
impl Action for ConfigureKeystore {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Insert the validator keys into the keystore `{}`",
            self.keystore_dir.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "configure_keystore",
            keystore_dir = tracing::field::display(self.keystore_dir.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            KEYSTORE_ENTRIES
                .iter()
                .map(|(key_type, scheme, source)| {
                    format!("`{key_type}` ({scheme}) from `{source}`")
                })
                .collect(),
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            keystore_dir,
            keys_dir,
            node,
        } = self;

        tokio::fs::create_dir_all(&keystore_dir)
            .await
            .with_context(|| format!("Creating directory `{}`", keystore_dir.display()))?;
        let uid = crate::util::uid_of(&node.user)?;
        let gid = crate::util::gid_of(&node.user).ok();
        crate::util::chown_recursive(keystore_dir, Some(uid), gid)?;
        tokio::fs::set_permissions(
            &keystore_dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .await
        .with_context(|| {
            format!(
                "Setting mode `{:#o}` on `{}`",
                0o700,
                keystore_dir.display()
            )
        })?;

        // `--suri` reads the phrase from a file when given a path, so it never appears in
        // the process list. The file lives only for the length of the insert.
        let suri_file = keys_dir.join(".suri");
        for (key_type, scheme, source) in KEYSTORE_ENTRIES {
            let secret = read_key_field(&keys_dir.join(source), "secretPhrase").await?;
            crate::util::write_owned_file(&suri_file, &secret, 0o600, Some(&node.user)).await?;

            tracing::info!("Inserting the `{key_type}` key into the keystore");
            let inserted = crate::execute_command(
                node.command()
                    .arg("key")
                    .arg("insert")
                    .arg("--keystore-path")
                    .arg(&*keystore_dir)
                    .arg("--scheme")
                    .arg(scheme.as_str())
                    .arg("--key-type")
                    .arg(key_type)
                    .arg("--suri")
                    .arg(&suri_file),
            )
            .await;

            crate::util::remove_file_if_exists(&suri_file)
                .await
                .with_context(|| format!("Removing `{}`", suri_file.display()))?;
            inserted?;
        }

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!(
                "Leave the keystore `{}` in place",
                self.keystore_dir.display()
            ),
            vec![String::from("It holds validator key material")],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::warn!(
            "Leaving the keystore `{}` in place",
            self.keystore_dir.display()
        );
        Ok(())
    }
}
