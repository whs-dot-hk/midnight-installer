use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, ActionState, StatefulAction};
use crate::util::file_has_contents;

/** Generate this host's WireGuard identity, and report the public half

The private key is the host's identity on the Foundation's tunnel: it is never regenerated
over an existing one, and a public key without its private counterpart is an error rather
than a reason to make a new pair — that would silently change the identity the Foundation
has already been given.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "generate_wireguard_keys")]
pub struct GenerateWireguardKeys {
    private_key: PathBuf,
    public_key: PathBuf,
}

impl GenerateWireguardKeys {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(wireguard_data: impl Into<PathBuf>) -> anyhow::Result<StatefulAction<Self>> {
        let wireguard_data = wireguard_data.into();
        let this = Self {
            private_key: wireguard_data.join("privatekey"),
            public_key: wireguard_data.join("publickey"),
        };

        let has_private = file_has_contents(&this.private_key).await;
        let has_public = file_has_contents(&this.public_key).await;

        let state = match (has_private, has_public) {
            (true, true) => {
                tracing::warn!("A WireGuard keypair already exists, it will not be regenerated");
                ActionState::Skipped
            },
            (false, true) => {
                anyhow::bail!(
                    "`{public}` exists but `{private}` does not. Refusing to generate a mismatched pair: move the public key aside if a new identity is really wanted.",
                    public = this.public_key.display(),
                    private = this.private_key.display(),
                )
            },
            // A private key with no public key is recoverable: the public key derives from it
            (true, false) | (false, false) => ActionState::Uncompleted,
        };

        Ok(StatefulAction {
            action: this,
            state,
        })
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "generate_wireguard_keys")]
impl Action for GenerateWireguardKeys {
    fn tracing_synopsis(&self) -> String {
        String::from("Generate the WireGuard identity keypair")
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "generate_wireguard_keys",
            private_key = tracing::field::display(self.private_key.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!("Private key `{}`, mode `600`", self.private_key.display()),
                format!("Public key `{}`", self.public_key.display()),
                String::from(
                    "The public key is what the Midnight Foundation needs; they peer with it and this host's static IP, so make sure that address is configured",
                ),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            private_key,
            public_key,
        } = self;

        if !file_has_contents(private_key).await {
            let generated =
                crate::execute_command_stdout(crate::command("wg").arg("genkey")).await?;
            crate::util::write_owned_file(private_key, generated.trim_end(), 0o600, Some("root"))
                .await?;
        } else {
            tracing::info!("Rebuilding the missing public key from the existing private key");
        }

        let private = tokio::fs::read_to_string(&private_key)
            .await
            .with_context(|| format!("Reading `{}`", private_key.display()))?;

        let public = derive_public_key(private.trim()).await?;
        crate::util::write_owned_file(public_key, &public, 0o600, Some("root")).await?;

        tracing::info!(
            "WireGuard public key (send this to the Midnight Foundation): {}",
            public.trim()
        );

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            String::from("Leave the WireGuard identity in place"),
            vec![String::from(
                "The Foundation may already have peered with this public key",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::warn!(
            "Leaving the WireGuard identity `{}` in place",
            self.private_key.display()
        );
        Ok(())
    }
}

/// `wg pubkey` reads the private key on stdin, so it never reaches a command line
async fn derive_public_key(private: &str) -> anyhow::Result<String> {
    let output = crate::execute_command_with_stdin(
        crate::command("wg").arg("pubkey"),
        format!("{private}\n").as_bytes(),
        "wg pubkey",
    )
    .await?;
    String::from_utf8(output.stdout).context("Output was not valid UTF-8")
}
