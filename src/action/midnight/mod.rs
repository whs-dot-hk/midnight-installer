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
pub(crate) mod verify_secret_flags;

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
pub use verify_secret_flags::VerifySecretFlags;

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

/** The seed files the node's environment points at: variable, keystore key type, key file

Each seed is named as a keystore entry is, `<key type as hex><public key as hex>`, which is
what the FNO guides' `.env` shows (`AURA_SEED_FILE=~/keystore/61757261…`). The key type is
the one `midnight-node` inserts the seed under at start up, so the cross-chain seed is
`crch` even though the guides insert the same key into the keystore as `beef`.
*/
pub(crate) const SEED_FILES: &[(&str, &str, &str)] = &[
    ("AURA_SEED_FILE", "aura", "aura.json"),
    ("GRANDPA_SEED_FILE", "gran", "grandpa.json"),
    ("CROSS_CHAIN_SEED_FILE", "crch", "cross_chain.json"),
];

/// The keystore-style name of a seed: the key type and the public key, both as bare hex
pub(crate) fn seed_file_name(key_type: &str, public_key: &str) -> String {
    let public_key = public_key.strip_prefix("0x").unwrap_or(public_key);
    let key_type: String = key_type.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{key_type}{public_key}")
}

/// One seed file: its environment variable, where it goes, and the phrase it holds
pub(crate) struct SeedFile {
    pub variable: &'static str,
    pub path: PathBuf,
    pub phrase: String,
}

/// The seed files, each named after the public key in its key file in `keys_dir`
pub(crate) async fn seed_files(keys_dir: &Path, seed_dir: &Path) -> anyhow::Result<Vec<SeedFile>> {
    let mut seeds = vec![];
    for (variable, key_type, source) in SEED_FILES {
        let path = keys_dir.join(source);
        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Reading `{}`", path.display()))?;
        let json: serde_json::Value = serde_json::from_str(&contents)
            .with_context(|| format!("Parsing JSON of `{}`", path.display()))?;
        let field = |name: &str| {
            json.get(name)
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .with_context(|| format!("`{}` has no `{name}`", path.display()))
        };
        seeds.push(SeedFile {
            variable,
            path: seed_dir.join(seed_file_name(key_type, &field("publicKey")?)),
            phrase: field("secretPhrase")?,
        });
    }
    Ok(seeds)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn names_a_seed_as_its_keystore_entry() {
        assert_eq!(seed_file_name("aura", "0xd43593c7"), "61757261d43593c7");
        assert_eq!(seed_file_name("gran", "88dc3417"), "6772616e88dc3417");
        assert_eq!(seed_file_name("crch", "0x020a1091"), "63726368020a1091");
    }
}
