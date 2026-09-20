use anyhow::Context;
use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};
use crate::credentials::DatabaseCredentials;
use crate::settings::{CommonSettings, MainChainParams};
use crate::util::systemd_env_quote;

/** Write the environment file the Midnight node's service reads

It holds the database connection string, so it is mode `600` and owned by the service user.
The values which describe the Cardano network (its epoch geometry) come from
[`MainChainParams`], not from this host.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "create_validator_env_file")]
pub struct CreateValidatorEnvFile {
    path: PathBuf,
    user: String,
    node_name: String,
    network: String,
    credentials: DatabaseCredentials,
    main_chain: MainChainParams,
    node_key_file: PathBuf,
    aura_seed_file: PathBuf,
    grandpa_seed_file: PathBuf,
    cross_chain_seed_file: PathBuf,
    prometheus_push_endpoint: String,
    sidechain_block_beneficiary: String,
}

impl CreateValidatorEnvFile {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        settings: &CommonSettings,
        node_name: impl Into<String>,
        credentials: &DatabaseCredentials,
        main_chain: MainChainParams,
        sidechain_block_beneficiary: impl Into<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        let paths = settings.paths();

        Ok(StatefulAction::uncompleted(Self {
            path: paths.midnight_env_file,
            user: settings.midnight_user.clone(),
            node_name: node_name.into(),
            network: settings.cardano_network.clone(),
            credentials: credentials.clone(),
            main_chain,
            node_key_file: paths.midnight_network_dir.join("secret_ed25519"),
            aura_seed_file: paths.midnight_keys_dir.join("aura.seed"),
            grandpa_seed_file: paths.midnight_keys_dir.join("grandpa.seed"),
            cross_chain_seed_file: paths.midnight_keys_dir.join("cross_chain.seed"),
            prometheus_push_endpoint: crate::settings::PROMETHEUS_PUSH_ENDPOINT.to_string(),
            sidechain_block_beneficiary: sidechain_block_beneficiary.into(),
        }))
    }

    fn render(&self) -> String {
        let Self {
            user: _,
            path: _,
            node_name,
            network,
            credentials,
            main_chain,
            node_key_file,
            aura_seed_file,
            grandpa_seed_file,
            cross_chain_seed_file,
            prometheus_push_endpoint,
            sidechain_block_beneficiary,
        } = self;

        let mut buf = String::new();
        let mut push = |key: &str, value: String| buf.push_str(&format!("{key}={value}\n"));

        push("POSTGRES_HOST", systemd_env_quote(&credentials.host));
        push("POSTGRES_DB", systemd_env_quote(&credentials.name));
        push("POSTGRES_PORT", credentials.port.to_string());
        push("POSTGRES_USER", systemd_env_quote(&credentials.user));
        push(
            "POSTGRES_PASSWORD",
            systemd_env_quote(credentials.password.expose()),
        );
        push(
            "DB_SYNC_POSTGRES_CONNECTION_STRING",
            systemd_env_quote(&credentials.connection_string()),
        );
        push(
            "PROMETHEUS_PUSH_ENDPOINT",
            systemd_env_quote(prometheus_push_endpoint),
        );
        push("CFG_PRESET", systemd_env_quote(network));
        push("NODE_NAME", systemd_env_quote(node_name));
        push(
            "NODE_KEY_FILE",
            systemd_env_quote(&node_key_file.display().to_string()),
        );
        push(
            "AURA_SEED_FILE",
            systemd_env_quote(&aura_seed_file.display().to_string()),
        );
        push(
            "GRANDPA_SEED_FILE",
            systemd_env_quote(&grandpa_seed_file.display().to_string()),
        );
        push(
            "CROSS_CHAIN_SEED_FILE",
            systemd_env_quote(&cross_chain_seed_file.display().to_string()),
        );
        push(
            "SIDECHAIN_BLOCK_BENEFICIARY",
            systemd_env_quote(sidechain_block_beneficiary),
        );
        push(
            "CARDANO_SECURITY_PARAMETER",
            main_chain.security_parameter.to_string(),
        );
        push(
            "CARDANO_ACTIVE_SLOTS_COEFF",
            main_chain.active_slots_coeff.to_string(),
        );
        push(
            "BLOCK_STABILITY_MARGIN",
            main_chain.block_stability_margin.to_string(),
        );
        push(
            "MC__FIRST_EPOCH_TIMESTAMP_MILLIS",
            main_chain.first_epoch_timestamp_millis.to_string(),
        );
        push(
            "MC__FIRST_EPOCH_NUMBER",
            main_chain.first_epoch_number.to_string(),
        );
        push(
            "MC__EPOCH_DURATION_MILLIS",
            main_chain.epoch_duration_millis.to_string(),
        );
        push(
            "MC__FIRST_SLOT_NUMBER",
            main_chain.first_slot_number.to_string(),
        );
        push(
            "MC__SLOT_DURATION_MILLIS",
            main_chain.slot_duration_millis.to_string(),
        );
        push("ALLOW_NON_SSL", String::from("true"));

        buf
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "create_validator_env_file")]
impl Action for CreateValidatorEnvFile {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Write the validator environment file `{}`",
            self.path.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "create_validator_env_file",
            path = tracing::field::display(self.path.display()),
            node_name = self.node_name,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!("Node name `{}`", self.node_name),
                format!(
                    "Owned by `{}`, mode `600`: it holds the database password",
                    self.user
                ),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        crate::util::write_owned_file(&self.path, &self.render(), 0o600, Some(&self.user)).await?;
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::settings::Secret;

    fn example() -> CreateValidatorEnvFile {
        CreateValidatorEnvFile {
            path: PathBuf::from("/secret/.env"),
            user: String::from("ubuntu"),
            node_name: String::from("fno-1"),
            network: String::from("preprod"),
            credentials: DatabaseCredentials::new(
                "midnight",
                "cexplorer",
                Secret::new("p@ss word"),
            ),
            main_chain: MainChainParams::preprod(),
            node_key_file: PathBuf::from("/secret/node/secret_ed25519"),
            aura_seed_file: PathBuf::from("/secret/keys/aura.seed"),
            grandpa_seed_file: PathBuf::from("/secret/keys/grandpa.seed"),
            cross_chain_seed_file: PathBuf::from("/secret/keys/cross_chain.seed"),
            prometheus_push_endpoint: String::from("https://example.invalid/receive"),
            sidechain_block_beneficiary: String::from("00"),
        }
    }

    #[test]
    fn quotes_values_for_systemd() {
        let rendered = example().render();
        assert!(rendered.contains("POSTGRES_PASSWORD=\"p@ss word\"\n"));
        assert!(rendered.contains("NODE_NAME=\"fno-1\"\n"));
    }

    #[test]
    fn percent_encodes_the_connection_string() {
        let rendered = example().render();
        assert!(rendered.contains(
            "DB_SYNC_POSTGRES_CONNECTION_STRING=\"postgresql://midnight:p%40ss%20word@localhost:5432/cexplorer\"\n"
        ));
    }

    #[test]
    fn carries_the_main_chain_geometry() {
        let rendered = example().render();
        assert!(rendered.contains("MC__EPOCH_DURATION_MILLIS=432000000\n"));
        assert!(rendered.contains("CARDANO_SECURITY_PARAMETER=2160\n"));
    }
}
