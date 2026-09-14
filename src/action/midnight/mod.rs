/*! Actions for the Midnight node: the binary, the validator keys, and validator mode

The key material here is the one thing on an FNO host which cannot be rebuilt from anywhere
else, so every action which touches it refuses to overwrite what is already there, and none
of them delete it on revert.
*/

pub(crate) mod configure_keystore;
pub(crate) mod create_registration_file;
pub(crate) mod create_validator_env;
pub(crate) mod generate_keys;
pub(crate) mod install_midnight_node;
pub(crate) mod migrate_legacy_layout;
pub(crate) mod prepare_seed_files;

use anyhow::Context;
use std::path::{Path, PathBuf};

use tokio::process::Command;

pub use configure_keystore::ConfigureKeystore;
pub use create_registration_file::CreateRegistrationFile;
pub use create_validator_env::CreateValidatorEnvFile;
pub use generate_keys::{GenerateNetworkKey, GenerateValidatorKey, KeyScheme};
pub use install_midnight_node::InstallMidnightNode;
pub use migrate_legacy_layout::MigrateLegacyLayout;
pub use prepare_seed_files::PrepareSeedFiles;

/** How to run `midnight-node` for its `key` subcommands

The binary reads its `res/` configuration relative to the working directory and picks the
network from `CFG_PRESET`, so a bare invocation fails; this pins both, and runs it as the
service user so that anything it writes is owned by the user the node runs as.
*/
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct NodeInvocation {
    pub user: String,
    pub binary: PathBuf,
    /// The directory holding `res/`: the node's data directory, which is also the service's
    pub work_dir: PathBuf,
    pub network: String,
}

impl NodeInvocation {
    pub(crate) fn command(&self) -> Command {
        // `sudo` resets the environment, so the preset has to be set inside it
        let mut command = crate::command_as(&self.user, "env");
        command
            .arg(format!("CFG_PRESET={}", self.network))
            .arg(&self.binary)
            .current_dir(&self.work_dir);
        command
    }
}

/// One string field of a key file written by `midnight-node key generate --output-type json`
pub(crate) async fn read_key_field(path: &Path, field: &str) -> anyhow::Result<String> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Reading `{}`", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&contents)
        .with_context(|| format!("Parsing JSON of `{}`", path.display()))?;

    json.get(field)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .with_context(|| format!("`{}` has no `{field}`", path.display()))
}
