/*! Actions for a devnet host: its nodes' keys, and the genesis they share

Unlike an FNO host's, a devnet's keys are disposable: the chain they validate is rebuilt with
the devnet, so reverting deletes them instead of leaving them in place.
*/

use anyhow::Context;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::process::Command;
use tracing::{span, Span};

use crate::action::{planned, Action, ActionDescription, ActionState, StatefulAction};
use crate::devnet::{
    set_slots_per_epoch, template_has_babe, Committee, DevnetPaths, NodeIdentity, BABE_KEY,
    VALIDATOR_KEYS,
};

/// Run the devnet's `midnight-node` as `user`, from the release directory it reads `res/` from
fn node_command(paths: &DevnetPaths, user: &str, env: &[(&str, String)]) -> Command {
    // `sudo` resets the environment, so anything the node needs is set inside it
    let mut command = crate::command_as(user, "env");
    for (key, value) in env {
        command.arg(format!("{key}={value}"));
    }
    command.arg(paths.binary()).current_dir(&paths.release_dir);
    command
}

async fn read_json(path: &Path) -> anyhow::Result<Value> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Reading `{}`", path.display()))?;
    serde_json::from_str(&contents).with_context(|| format!("Parsing JSON of `{}`", path.display()))
}

async fn write_json(path: &Path, value: &Value, user: &str) -> anyhow::Result<()> {
    let contents = serde_json::to_string_pretty(value)? + "\n";
    crate::util::write_owned_file(path, &contents, 0o644, Some(user)).await
}

/// The release's committee template, which says whether this release's keys include BABE
pub(crate) async fn committee_template(paths: &DevnetPaths) -> anyhow::Result<Value> {
    read_json(
        &paths
            .release_dir
            .join("res")
            .join("dev")
            .join("permissioned-candidates-config.json"),
    )
    .await
}

/** Generate the keys one devnet node does not have yet, and write down the public half

A validator gets an AURA, a GRANDPA and a cross-chain key (and BABE, for a release whose
committee has one), each as a seed file the node reads at start up. Every node gets a libp2p
key. What the rest of the devnet needs to know — the public keys and the peer id — goes in
`public.json`, which is the only file the controller reads back.

A key the node already has is kept, because the genesis names it. Only what is missing is
generated, which is how a node from an older release gains the BABE key a newer one wants.
So reverting deletes the node only if this action created it; one which existed before is
left as it was found.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "generate_devnet_node_keys")]
pub struct GenerateDevnetNodeKeys {
    root: PathBuf,
    chain: String,
    name: String,
    validator: bool,
    user: String,
    /// Whether the node was created by this action, and so is this action's to delete
    #[serde(default)]
    created: bool,
}

impl GenerateDevnetNodeKeys {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        root: impl Into<PathBuf>,
        chain: impl Into<String>,
        name: impl Into<String>,
        validator: bool,
        user: impl Into<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        let this = Self {
            root: root.into(),
            chain: chain.into(),
            name: name.into(),
            validator,
            user: user.into(),
            created: false,
        };

        // A node which already has its identity keeps it: the genesis committee names it.
        // It is still planned, because the release installed a moment earlier may want a
        // key it does not have yet.
        if let Some(identity) = this.existing().await {
            if identity.validator != validator {
                anyhow::bail!(
                    "`{}` already has keys as a {}; remove the devnet before changing what it is",
                    this.name,
                    if validator { "full node" } else { "validator" }
                );
            }
        }

        Ok(planned(this, ActionState::Uncompleted))
    }

    fn paths(&self) -> DevnetPaths {
        DevnetPaths::new(&self.root, &self.chain)
    }

    async fn existing(&self) -> Option<NodeIdentity> {
        let public = self.paths().node(&self.name).public;
        let contents = tokio::fs::read_to_string(public).await.ok()?;
        serde_json::from_str(&contents).ok()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "generate_devnet_node_keys")]
impl Action for GenerateDevnetNodeKeys {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Generate the keys devnet {role} `{name}` does not have yet",
            role = if self.validator { "validator" } else { "node" },
            name = self.name
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "generate_devnet_node_keys",
            name = self.name,
            validator = self.validator,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let node = self.paths().node(&self.name);
        let mut explanation = vec![format!("A libp2p key at `{}`", node.node_key.display())];
        if self.validator {
            explanation.push(format!(
                "AURA, GRANDPA and cross-chain seeds in `{}`, mode `600`",
                node.secrets.display()
            ));
        }
        explanation.push(format!(
            "The public keys and peer id in `{}`",
            node.public.display()
        ));
        explanation.push(String::from("Keys the node already has are kept"));
        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let paths = self.paths();
        let node = paths.node(&self.name);
        let user = self.user.as_str();
        let uid = crate::util::uid_of(user)?;
        let gid = crate::util::gid_of(user).ok();

        let existing = self.existing().await;
        self.created = existing.is_none();
        let mut identity = existing.unwrap_or_else(|| NodeIdentity {
            name: self.name.clone(),
            peer_id: String::new(),
            validator: self.validator,
            aura_pub_key: None,
            grandpa_pub_key: None,
            cross_chain_pub_key: None,
            babe_pub_key: None,
        });
        let mut generated = vec![];

        for (dir, mode) in [(&node.dir, 0o755), (&node.secrets, 0o700)] {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("Creating directory `{}`", dir.display()))?;
            nix::unistd::chown(dir.as_path(), Some(uid), gid)
                .with_context(|| format!("Changing the owner of `{}`", dir.display()))?;
            tokio::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(mode))
                .await
                .with_context(|| format!("Setting mode `{mode:#o}` on `{}`", dir.display()))?;
        }

        if self.validator {
            let mut keys: Vec<(&str, &str)> = VALIDATOR_KEYS
                .iter()
                .map(|(key, scheme, _)| (*key, *scheme))
                .collect();
            if template_has_babe(&committee_template(&paths).await?) {
                keys.push((BABE_KEY.0, BABE_KEY.1));
            }

            for (key, scheme) in keys {
                let known = match key {
                    "aura" => &mut identity.aura_pub_key,
                    "grandpa" => &mut identity.grandpa_pub_key,
                    "cross_chain" => &mut identity.cross_chain_pub_key,
                    "babe" => &mut identity.babe_pub_key,
                    _ => unreachable!("no other key is generated"),
                };
                if known.is_some() && node.seed(key).is_file() {
                    continue;
                }

                // The runner which never logs output: the output is the secret
                let output = crate::execute_command_secret(
                    node_command(&paths, user, &[]).args([
                        "key",
                        "generate",
                        "--scheme",
                        scheme,
                        "--output-type",
                        "json",
                    ]),
                    "midnight-node key generate",
                )
                .await?;
                let json: Value = serde_json::from_slice(&output.stdout)
                    .with_context(|| format!("Parsing the generated {key} key"))?;
                let field = |name: &str| {
                    json[name]
                        .as_str()
                        .map(str::to_string)
                        .with_context(|| format!("The generated {key} key has no `{name}`"))
                };
                crate::util::write_owned_file(
                    &node.seed(key),
                    &format!("{}\n", field("secretSeed")?),
                    0o600,
                    Some(user),
                )
                .await?;
                *known = Some(field("publicKey")?);
                generated.push(key);
            }
        }

        if identity.peer_id.is_empty() || !node.node_key.is_file() {
            // `generate-node-key` will not run without a chain spec, hence `--chain dev`; it
            // prints the peer id on stderr. Run as the user, so the key is the user's.
            let output = crate::execute_command(
                node_command(&paths, user, &[])
                    .args(["key", "generate-node-key", "--chain", "dev", "--file"])
                    .arg(&node.node_key),
            )
            .await?;
            identity.peer_id = peer_id(&[output.stdout, output.stderr].concat())
                .context("`generate-node-key` did not print a peer id")?;
            generated.push("network");
        }

        if generated.is_empty() {
            tracing::info!(
                "Devnet node `{}` has every key it needs: peer {}",
                self.name,
                identity.peer_id
            );
            return Ok(());
        }
        let public = serde_json::to_value(&identity)?;
        write_json(&node.public, &public, user).await?;
        tracing::info!(
            "Devnet node `{}`: generated {}; peer {}",
            self.name,
            generated.join(", "),
            identity.peer_id
        );
        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Delete devnet node `{}`", self.name),
            vec![
                format!(
                    "Its keys and chain data in `{}`: a devnet is rebuilt, not restored",
                    self.paths().node(&self.name).dir.display()
                ),
                String::from("A node which existed before this run is left as it was"),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        if !self.created {
            tracing::info!(
                "Leaving devnet node `{}` in place: it existed before this run",
                self.name
            );
            return Ok(());
        }
        let dir = self.paths().node(&self.name).dir;
        crate::util::remove_dir_all_if_exists(&dir)
            .await
            .with_context(|| format!("Removing `{}`", dir.display()))
    }
}

/// The first `12D3…` peer id in a node's output
fn peer_id(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("12D3"))
        .map(str::to_string)
}

/** Put the devnet's genesis in place on this host

Every host writes the mock follower's committee. The chain spec is either built here, from
the release's `dev` preset with the committee swapped in, or — on every host but the first —
copied from the one the first host built, because all of a chain's nodes have to agree on its
genesis byte for byte.

A genesis which changes makes the chain data under it worthless, so when the spec changes
the nodes' data is deleted with it. One built from the same committee as before is kept:
rebuilding it would not be guaranteed to come out the same.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_devnet_genesis")]
pub struct InstallDevnetGenesis {
    root: PathBuf,
    chain: String,
    user: String,
    committee: Committee,
    /// The first host's spec, on every other host; `None` to build it here
    chain_spec: Option<PathBuf>,
    /// This host's nodes, whose data a new genesis invalidates
    nodes: Vec<String>,
}

impl InstallDevnetGenesis {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        root: impl Into<PathBuf>,
        chain: impl Into<String>,
        user: impl Into<String>,
        committee: Committee,
        chain_spec: Option<PathBuf>,
        nodes: Vec<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        if let Some(spec) = &chain_spec {
            if !spec.is_file() {
                anyhow::bail!("The chain spec `{}` does not exist", spec.display());
            }
        }
        Ok(StatefulAction::uncompleted(Self {
            root: root.into(),
            chain: chain.into(),
            user: user.into(),
            committee,
            chain_spec,
            nodes,
        }))
    }

    fn paths(&self) -> DevnetPaths {
        DevnetPaths::new(&self.root, &self.chain)
    }

    async fn build_spec(&self, paths: &DevnetPaths) -> anyhow::Result<Vec<u8>> {
        let user = self.user.as_str();
        let template = committee_template(paths).await?;
        write_json(
            &paths.permissioned_candidates,
            &self.committee.permissioned_candidates(&template)?,
            user,
        )
        .await?;
        let parameters = read_json(
            &paths
                .release_dir
                .join("res")
                .join("dev")
                .join("system-parameters-config.json"),
        )
        .await?;
        write_json(
            &paths.system_parameters,
            &self.committee.system_parameters(&parameters),
            user,
        )
        .await?;

        tracing::info!("Building the chain spec of devnet `{}`", self.chain);
        let output = crate::execute_command(
            node_command(
                paths,
                user,
                &[
                    ("CFG_PRESET", String::from("dev")),
                    ("CHAINSPEC_NAME", self.chain.clone()),
                    (
                        "CHAINSPEC_PERMISSIONED_CANDIDATES_CONFIG",
                        paths.permissioned_candidates.display().to_string(),
                    ),
                    (
                        "CHAINSPEC_SYSTEM_PARAMETERS_CONFIG",
                        paths.system_parameters.display().to_string(),
                    ),
                ],
            )
            .args(["build-spec", "--disable-default-bootnode"]),
        )
        .await?;

        let mut spec: Value =
            serde_json::from_slice(&output.stdout).context("`build-spec` did not print JSON")?;
        set_slots_per_epoch(&mut spec, self.committee.slots_per_epoch)?;
        Ok((serde_json::to_string_pretty(&spec)? + "\n").into_bytes())
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_devnet_genesis")]
impl Action for InstallDevnetGenesis {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Put the genesis of devnet `{}` ({} validators) in place",
            self.chain,
            self.committee.validators.len()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_devnet_genesis",
            chain = self.chain,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let paths = self.paths();
        let spec = match &self.chain_spec {
            Some(source) => format!(
                "Copy the chain spec from `{}` to `{}`",
                source.display(),
                paths.chain_spec.display()
            ),
            None => format!(
                "Build `{}` from the `dev` preset with this committee, {} slots per epoch",
                paths.chain_spec.display(),
                self.committee.slots_per_epoch
            ),
        };
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!(
                    "Committee: {}",
                    self.committee
                        .validators
                        .iter()
                        .map(|v| v.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                spec,
                String::from("If the spec changes, delete the chain data it no longer matches"),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let paths = self.paths();
        let user = self.user.as_str();

        let current_spec = tokio::fs::read(&paths.chain_spec).await.ok();
        let current_committee: Option<Committee> = tokio::fs::read_to_string(&paths.committee)
            .await
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok());

        let new_spec = match &self.chain_spec {
            Some(source) => Some(
                tokio::fs::read(source)
                    .await
                    .with_context(|| format!("Reading `{}`", source.display()))?,
            ),
            None if current_spec.is_some()
                && current_committee.as_ref() == Some(&self.committee) =>
            {
                tracing::info!("The committee is unchanged, keeping the chain spec");
                None
            },
            None => Some(self.build_spec(&paths).await?),
        };

        if let Some(new_spec) = new_spec {
            if current_spec.as_deref() != Some(new_spec.as_slice()) {
                for node in &self.nodes {
                    let data = paths.node(node).data;
                    if data.exists() {
                        tracing::info!("The genesis changed, deleting the chain data of `{node}`");
                        crate::util::remove_dir_all_if_exists(&data)
                            .await
                            .with_context(|| format!("Removing `{}`", data.display()))?;
                    }
                }
                let contents =
                    String::from_utf8(new_spec).context("The chain spec is not UTF-8")?;
                crate::util::write_owned_file(&paths.chain_spec, &contents, 0o644, Some(user))
                    .await?;
            }
        }

        write_json(
            &paths.mock_registrations,
            &self.committee.mock_registrations()?,
            user,
        )
        .await?;
        write_json(
            &paths.committee,
            &serde_json::to_value(&self.committee)?,
            user,
        )
        .await?;
        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Delete the genesis of devnet `{}`", self.chain),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        let paths = self.paths();
        for path in [
            &paths.chain_spec,
            &paths.committee,
            &paths.mock_registrations,
            &paths.permissioned_candidates,
            &paths.system_parameters,
        ] {
            crate::util::remove_file_if_exists(path)
                .await
                .with_context(|| format!("Removing `{}`", path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn finds_the_peer_id_among_the_log_lines() {
        let output =
            b"2026-10-15 Generating key\n12D3KooWQR72Ka2242aAanJ2aTfAQAfazhWL1QBB4EcivnsJpE7K\n";
        assert_eq!(
            peer_id(output).as_deref(),
            Some("12D3KooWQR72Ka2242aAanJ2aTfAQAfazhWL1QBB4EcivnsJpE7K")
        );
        assert_eq!(peer_id(b"nothing here"), None);
    }
}
