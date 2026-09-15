use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::midnight::NodeInvocation;
use crate::action::{planned, Action, ActionDescription, ActionState, StatefulAction};
use crate::util::file_has_contents;

/// The signature schemes the Midnight validator keys use
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeyScheme {
    Sr25519,
    Ed25519,
    Ecdsa,
}

impl KeyScheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sr25519 => "sr25519",
            Self::Ed25519 => "ed25519",
            Self::Ecdsa => "ecdsa",
        }
    }
}

impl std::fmt::Display for KeyScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/** Generate one validator key, as JSON holding its public key and secret phrase

An existing key is never regenerated and never deleted on revert: a validator's identity is
the one thing on this host which cannot be recreated from a release archive or a snapshot.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "generate_validator_key")]
pub struct GenerateValidatorKey {
    label: String,
    scheme: KeyScheme,
    path: PathBuf,
    node: NodeInvocation,
}

impl GenerateValidatorKey {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        label: impl Into<String>,
        scheme: KeyScheme,
        path: impl Into<PathBuf>,
        node: NodeInvocation,
    ) -> anyhow::Result<StatefulAction<Self>> {
        let this = Self {
            label: label.into(),
            scheme,
            path: path.into(),
            node,
        };

        let state = if file_has_contents(&this.path).await {
            tracing::warn!(
                "{label} key already exists at `{path}`, it will not be regenerated",
                label = this.label,
                path = this.path.display()
            );
            ActionState::Skipped
        } else {
            ActionState::Uncompleted
        };

        Ok(planned(this, state))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "generate_validator_key")]
impl Action for GenerateValidatorKey {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Generate the {label} key ({scheme}) at `{path}`",
            label = self.label,
            scheme = self.scheme,
            path = self.path.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "generate_validator_key",
            label = self.label,
            scheme = self.scheme.as_str(),
            path = tracing::field::display(self.path.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!("Owned by `{}`, mode `600`", self.node.user),
                String::from("Back this up offline: it cannot be recovered"),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            scheme, path, node, ..
        } = self;

        // The key comes back on stdout, so it is written here with its mode already set
        // rather than by the node under whatever umask is in force. The runner which never
        // logs output is used, because the output is the secret phrase.
        let output = crate::execute_command_secret(
            node.command()
                .arg("key")
                .arg("generate")
                .arg("--scheme")
                .arg(scheme.as_str())
                .arg("--output-type")
                .arg("json"),
            "midnight-node key generate",
        )
        .await?;

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("Parsing JSON of `{}`", path.display()))?;
        if json.get("publicKey").is_none() || json.get("secretPhrase").is_none() {
            anyhow::bail!(
                "`{}` has an unexpected format: {}",
                path.display(),
                "expected `publicKey` and `secretPhrase`"
            );
        }

        let contents = String::from_utf8(output.stdout).context("Output was not valid UTF-8")?;
        crate::util::write_owned_file(path, &contents, 0o600, Some(&node.user)).await
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Leave the {} key in place", self.label),
            vec![String::from(
                "A validator key is never deleted by this installer",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::warn!(
            "Leaving the {label} key `{path}` in place; if this host is being torn down, destroy it deliberately",
            label = self.label,
            path = self.path.display()
        );
        Ok(())
    }
}

/// Generate the node's libp2p network identity, and report the resulting PeerID
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "generate_network_key")]
pub struct GenerateNetworkKey {
    path: PathBuf,
    node: NodeInvocation,
}

impl GenerateNetworkKey {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        path: impl Into<PathBuf>,
        node: NodeInvocation,
    ) -> anyhow::Result<StatefulAction<Self>> {
        let this = Self {
            path: path.into(),
            node,
        };

        let state = if file_has_contents(&this.path).await {
            tracing::warn!(
                "The network key already exists at `{}`, it will not be regenerated",
                this.path.display()
            );
            ActionState::Skipped
        } else {
            ActionState::Uncompleted
        };

        Ok(planned(this, state))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "generate_network_key")]
impl Action for GenerateNetworkKey {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Generate the Midnight network identity at `{}`",
            self.path.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "generate_network_key",
            path = tracing::field::display(self.path.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![String::from(
                "This is the node's PeerID; changing it makes the node a stranger to its peers",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self { path, node } = self;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Creating directory `{}`", parent.display()))?;
            let uid = crate::util::uid_of(&node.user)?;
            let gid = crate::util::gid_of(&node.user).ok();
            nix::unistd::chown(parent, Some(uid), gid)
                .with_context(|| format!("Changing the owner of `{}`", parent.display()))?;
            tokio::fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .await
                .with_context(|| {
                    format!("Setting mode `{:#o}` on `{}`", 0o700, parent.display())
                })?;
        }

        // Without `--file` the node prints the secret key to stdout and the PeerID to
        // stderr, so the key is written here with its mode already set
        let output = crate::execute_command_secret(
            node.command().arg("key").arg("generate-node-key"),
            "midnight-node key generate-node-key",
        )
        .await?;

        let secret = String::from_utf8(output.stdout).context("Output was not valid UTF-8")?;
        if secret.trim().is_empty() {
            anyhow::bail!(
                "`{}` has an unexpected format: {}",
                path.display(),
                "`generate-node-key` printed no key"
            );
        }
        crate::util::write_owned_file(path, secret.trim(), 0o600, Some(&node.user)).await?;

        tracing::info!(
            "Midnight PeerID: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            String::from("Leave the Midnight network identity in place"),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::warn!("Leaving the network key `{}` in place", self.path.display());
        Ok(())
    }
}
