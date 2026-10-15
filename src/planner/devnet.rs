/*! The two stages a devnet host runs, in the order the controller runs them

They are subcommands like any other, so a host can be inspected with `plan` and unwound with
`uninstall`, but they are hidden from `--help`: the controller (`midnight-installer devnet`)
is what runs them, with the committee and chain spec only it can assemble.
*/

use std::collections::HashMap;
use std::path::PathBuf;

use crate::action::base::{
    CreateDirectory, CreateFile, CreateSystemdUnit, FetchAndUnpackTarball, InstallBinary,
    InstallTree, StartSystemdUnit,
};
use crate::action::devnet::{committee_template, GenerateDevnetNodeKeys, InstallDevnetGenesis};
use crate::action::{Action, StatefulAction};
use crate::devnet::config::Ports;
use crate::devnet::{
    ordered_nodes, render_env, render_unit, template_has_babe, unit_name, Committee, DevnetPaths,
    HostNode, NodeIdentity, DEFAULT_CHAIN, DEFAULT_DEVNET_ROOT, DEFAULT_DEVNET_VERSION,
    DEFAULT_P2P_PORT, DEFAULT_PROMETHEUS_PORT, DEFAULT_RPC_PORT, DEFAULT_USER,
};
use crate::planner::{diff_from_default, platform_check, require_root, require_user, Planner};

/// What both devnet stages need: which devnet, where, and which of its nodes are on this host
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct DevnetHost {
    /// Where devnets live on this host, one directory per chain
    #[clap(long, default_value = DEFAULT_DEVNET_ROOT)]
    pub root: PathBuf,

    /// The devnet's name: its directory, its units, and its chain spec's name
    #[clap(long, default_value = DEFAULT_CHAIN)]
    pub chain: String,

    /// The user the nodes run as
    #[clap(long, default_value = DEFAULT_USER)]
    pub user: String,

    /// A validator on this host (repeatable)
    #[clap(long = "validator")]
    pub validators: Vec<String>,

    /// A full node on this host (repeatable)
    #[clap(long = "full-node")]
    pub full_nodes: Vec<String>,
}

impl DevnetHost {
    fn default_settings() -> Self {
        Self {
            root: PathBuf::from(DEFAULT_DEVNET_ROOT),
            chain: String::from(DEFAULT_CHAIN),
            user: String::from(DEFAULT_USER),
            validators: vec![],
            full_nodes: vec![],
        }
    }

    pub fn paths(&self) -> DevnetPaths {
        DevnetPaths::new(&self.root, &self.chain)
    }

    /// This host's nodes in the order everything numbers them by: validators first
    fn nodes(&self) -> Vec<(String, bool)> {
        ordered_nodes(&self.validators, &self.full_nodes)
            .map(|(name, validator)| (name.to_string(), validator))
            .collect()
    }

    fn check(&self) -> anyhow::Result<()> {
        valid_name("devnet", &self.chain)?;
        let nodes = self.nodes();
        if nodes.is_empty() {
            anyhow::bail!(
                "Name at least one node for this host, with `--validator` or `--full-node`"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for (name, _) in &nodes {
            valid_name("node", name)?;
            if !seen.insert(name) {
                anyhow::bail!("The node `{name}` is named twice");
            }
        }
        Ok(())
    }

    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        let mut settings = HashMap::new();
        settings.insert("root".into(), serde_json::to_value(&self.root)?);
        settings.insert("chain".into(), serde_json::to_value(&self.chain)?);
        settings.insert("user".into(), serde_json::to_value(&self.user)?);
        settings.insert("validators".into(), serde_json::to_value(&self.validators)?);
        settings.insert("full_nodes".into(), serde_json::to_value(&self.full_nodes)?);
        Ok(settings)
    }
}

/// A name which ends up in a path, a unit name and on a command line: it starts with a
/// letter or digit (so it is never taken for a flag) and uses only those, `-` and `_`
pub(crate) fn valid_name(what: &str, name: &str) -> anyhow::Result<()> {
    let mut chars = name.chars();
    let starts_well = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    if !starts_well || !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        anyhow::bail!(
            "The {what} name `{name}` must start with a letter or digit, and use only letters, digits, `-` and `_`"
        );
    }
    Ok(())
}

/** Install the node release, and generate the keys of this host's devnet nodes

The keys are generated here and stay here; the controller reads back only each node's
`public.json`. A node which already has keys keeps them, so running this again changes
nothing but the release.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct DevnetKeys {
    #[clap(flatten)]
    #[serde(flatten)]
    pub host: DevnetHost,

    /// The `midnight-node` release the devnet runs
    #[clap(long, default_value = DEFAULT_DEVNET_VERSION)]
    pub version: String,

    /// The SHA-256 of that release's archive, for a version without a built-in checksum
    #[clap(long)]
    pub sha256: Option<String>,
}

#[async_trait::async_trait]
#[typetag::serde(name = "devnet-keys")]
impl Planner for DevnetKeys {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            host: DevnetHost::default_settings(),
            version: DEFAULT_DEVNET_VERSION.into(),
            sha256: None,
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        self.host.check()?;
        let paths = self.host.paths();
        let user = self.host.user.clone();
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        for (directory, owner, mode, prune) in [
            (&self.host.root, None, 0o755, false),
            (&paths.home, None, 0o755, false),
            (&paths.receipt_dir, None, 0o700, false),
            (&paths.scratch_dir, None, 0o700, true),
            (&paths.nodes_dir, Some(user.clone()), 0o755, false),
        ] {
            actions.push(
                CreateDirectory::plan(directory, owner.clone(), owner, Some(mode), prune)
                    .await?
                    .boxed(),
            );
        }

        let archive = crate::settings::midnight_release(&self.version, self.sha256.as_deref())?;
        let scratch = paths.scratch_dir.join("midnight-node");
        actions.push(
            FetchAndUnpackTarball::plan(archive, &scratch)
                .await?
                .boxed(),
        );
        actions.push(
            InstallBinary::plan(&scratch, "midnight-node", paths.binary(), user.clone())
                .await?
                .boxed(),
        );
        actions.push(
            InstallTree::plan(
                &scratch,
                "res",
                paths.release_dir.join("res"),
                user.clone(),
                user.clone(),
            )
            .await?
            .boxed(),
        );

        for (name, validator) in self.host.nodes() {
            actions.push(
                GenerateDevnetNodeKeys::plan(
                    &self.host.root,
                    &self.host.chain,
                    name,
                    validator,
                    &user,
                )
                .await?
                .boxed(),
            );
        }

        Ok(actions)
    }

    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        let mut settings = self.host.settings()?;
        settings.insert("version".into(), serde_json::to_value(&self.version)?);
        Ok(settings)
    }

    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        diff_from_default(self).await
    }

    fn receipt_path(&self) -> PathBuf {
        self.host.paths().receipt(self.typetag_name())
    }

    async fn platform_check(&self) -> anyhow::Result<()> {
        platform_check(self.typetag_name())
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.host.user)
    }
}

/** Put the devnet's genesis on this host, and run its nodes

Each node gets a unit, `midnight-devnet-<chain>-<node>.service`, listening for peers on its
own port (`--p2p-port` plus its index on this host, and likewise for RPC and Prometheus) and
pointed at the devnet's bootnode. It is restarted rather than only started, since its genesis
or environment may just have changed.
*/
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct DevnetNodes {
    #[clap(flatten)]
    #[serde(flatten)]
    pub host: DevnetHost,

    /// The genesis committee, as the controller assembled it (`install` needs it)
    #[clap(long)]
    pub committee: Option<PathBuf>,

    /// The chain spec another host built; without it, it is built here
    #[clap(long)]
    pub chain_spec: Option<PathBuf>,

    /// The multiaddr every node but the bootnode itself dials first
    #[clap(long)]
    pub bootnode: Option<String>,

    /// The first node's p2p port; the next node on this host uses the next port
    #[clap(long, default_value_t = DEFAULT_P2P_PORT)]
    pub p2p_port: u16,

    /// The first node's RPC port (local to this host)
    #[clap(long, default_value_t = DEFAULT_RPC_PORT)]
    pub rpc_port: u16,

    /// The first node's Prometheus port
    #[clap(long, default_value_t = DEFAULT_PROMETHEUS_PORT)]
    pub prometheus_port: u16,
}

impl DevnetNodes {
    fn host_nodes(&self) -> anyhow::Result<Vec<HostNode>> {
        let ports = Ports {
            p2p: self.p2p_port,
            rpc: self.rpc_port,
            prometheus: self.prometheus_port,
        };
        self.host
            .nodes()
            .into_iter()
            .enumerate()
            .map(|(i, (name, validator))| {
                let (p2p_port, rpc_port, prometheus_port) = ports.for_node(i)?;
                Ok(HostNode {
                    name,
                    validator,
                    p2p_port,
                    rpc_port,
                    prometheus_port,
                })
            })
            .collect()
    }

    async fn committee(&self) -> anyhow::Result<Committee> {
        // Optional on the command line so that `uninstall devnet-nodes` needs no committee
        // to find its receipt; a plan cannot be made without one
        let path = self
            .committee
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("`--committee` is needed to plan this stage"))?;
        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| anyhow::anyhow!("Reading `{}`: {e}", path.display()))?;
        let committee: Committee = serde_json::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Parsing `{}`: {e}", path.display()))?;
        if committee.chain != self.host.chain {
            anyhow::bail!(
                "The committee is for devnet `{}`, not `{}`",
                committee.chain,
                self.host.chain
            );
        }
        Ok(committee)
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "devnet-nodes")]
impl Planner for DevnetNodes {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            host: DevnetHost::default_settings(),
            committee: None,
            chain_spec: None,
            bootnode: None,
            p2p_port: DEFAULT_P2P_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            prometheus_port: DEFAULT_PROMETHEUS_PORT,
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        self.host.check()?;
        let paths = self.host.paths();
        let user = self.host.user.clone();
        let committee = self.committee().await?;
        let nodes = self.host_nodes()?;

        // Both written by `devnet-keys`, which has to have run on this host first
        let template = committee_template(&paths).await.map_err(|e| {
            anyhow::anyhow!(
                "No node release in `{}` ({e:#}); run `devnet-keys` first",
                paths.release_dir.display()
            )
        })?;
        let with_babe = template_has_babe(&template);

        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![InstallDevnetGenesis::plan(
            &self.host.root,
            &self.host.chain,
            &user,
            committee,
            self.chain_spec.clone(),
            nodes.iter().map(|n| n.name.clone()).collect(),
        )
        .await?
        .boxed()];

        for node in &nodes {
            let node_paths = paths.node(&node.name);
            let identity: NodeIdentity = serde_json::from_str(
                &std::fs::read_to_string(&node_paths.public).map_err(|e| {
                    anyhow::anyhow!("`{}` has no keys ({e}); run `devnet-keys` first", node.name)
                })?,
            )?;
            // Every node dials the bootnode, except the bootnode
            let bootnode = self
                .bootnode
                .as_deref()
                .filter(|addr| !addr.ends_with(&format!("/p2p/{}", identity.peer_id)));

            actions.push(
                CreateFile::plan(
                    &node_paths.env_file,
                    user.clone(),
                    user.clone(),
                    Some(0o644),
                    render_env(&paths, node, with_babe),
                    true,
                )
                .await?
                .boxed(),
            );
            let unit = unit_name(&self.host.chain, &node.name);
            actions.push(
                CreateSystemdUnit::plan(
                    &unit,
                    render_unit(&paths, &self.host.chain, &user, node, bootnode),
                )
                .await?
                .boxed(),
            );
            actions.push(StartSystemdUnit::plan_restart(&unit).await?.boxed());
        }

        Ok(actions)
    }

    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        let mut settings = self.host.settings()?;
        settings.insert("bootnode".into(), serde_json::to_value(&self.bootnode)?);
        settings.insert("p2p_port".into(), serde_json::to_value(self.p2p_port)?);
        settings.insert("rpc_port".into(), serde_json::to_value(self.rpc_port)?);
        settings.insert(
            "prometheus_port".into(),
            serde_json::to_value(self.prometheus_port)?,
        );
        Ok(settings)
    }

    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        diff_from_default(self).await
    }

    fn receipt_path(&self) -> PathBuf {
        self.host.paths().receipt(self.typetag_name())
    }

    async fn platform_check(&self) -> anyhow::Result<()> {
        platform_check(self.typetag_name())
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.host.user)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn names_must_be_safe_in_paths_and_units() {
        assert!(valid_name("node", "alice-1_b").is_ok());
        // A leading `-` would be read as a flag on the hosts' command lines
        for bad in ["", "a b", "../x", "a/b", "a.service", "-alice", "_x"] {
            assert!(valid_name("node", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_node_on_a_host_gets_the_next_ports() {
        let nodes = DevnetNodes {
            host: DevnetHost {
                validators: vec!["alice".into(), "bob".into()],
                full_nodes: vec!["archive".into()],
                ..DevnetHost::default_settings()
            },
            committee: None,
            chain_spec: None,
            bootnode: None,
            p2p_port: 30333,
            rpc_port: 9944,
            prometheus_port: 9615,
        };
        let host_nodes = nodes.host_nodes().unwrap();
        assert_eq!(
            host_nodes
                .iter()
                .map(|n| (n.name.as_str(), n.validator, n.p2p_port, n.rpc_port))
                .collect::<Vec<_>>(),
            vec![
                ("alice", true, 30333, 9944),
                ("bob", true, 30334, 9945),
                ("archive", false, 30335, 9946),
            ]
        );
    }
}
