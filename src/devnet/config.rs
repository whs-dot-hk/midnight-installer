/*! The devnet file: which hosts, how to reach them, and which nodes run on each

```toml
chain = "devnet"
version = "1.0.400"
slots_per_epoch = 10

[ssh]
kind = "gcloud"                      # `gcloud compute ssh`, through an IAP tunnel
project = "my-project"
zone = "europe-west1-b"

[[hosts]]
name = "devnet-0"                    # the instance; for `kind = "ssh"`, the destination
validators = ["alice", "bob"]

[[hosts]]
name = "devnet-1"
validators = ["charlie"]
full_nodes = ["archive"]
```
*/

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::devnet::{
    ordered_nodes, DEFAULT_CHAIN, DEFAULT_DEVNET_ROOT, DEFAULT_DEVNET_VERSION, DEFAULT_P2P_PORT,
    DEFAULT_PROMETHEUS_PORT, DEFAULT_RPC_PORT, DEFAULT_SLOTS_PER_EPOCH, DEFAULT_USER,
};
use crate::planner::devnet::valid_name;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevnetConfig {
    /// The devnet's name: its directory and units on every host, and its chain spec's name
    #[serde(default = "default_chain")]
    pub chain: String,
    /// The `midnight-node` release every node runs
    #[serde(default = "default_version")]
    pub version: String,
    /// The release archive's SHA-256, for a version without a built-in checksum
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default = "default_slots_per_epoch")]
    pub slots_per_epoch: u32,
    /// The user the nodes run as, which has to exist on every host
    #[serde(default = "default_user")]
    pub user: String,
    /// Where devnets live on every host
    #[serde(default = "default_root")]
    pub root: PathBuf,
    #[serde(default)]
    pub ports: Ports,
    pub ssh: Ssh,
    pub hosts: Vec<Host>,
}

/// The first node's ports on each host; each further node on that host takes the next one
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ports {
    #[serde(default = "default_p2p")]
    pub p2p: u16,
    #[serde(default = "default_rpc")]
    pub rpc: u16,
    #[serde(default = "default_prometheus")]
    pub prometheus: u16,
}

/// One node's ports: p2p, RPC, Prometheus
pub type NodePorts = (u16, u16, u16);

impl Ports {
    /// The ports of the node at `index` on its host: each base port plus the index
    pub fn for_node(&self, index: usize) -> anyhow::Result<NodePorts> {
        let index = u16::try_from(index)?;
        let port = |base: u16| {
            base.checked_add(index)
                .ok_or_else(|| anyhow::anyhow!("Port {base} + {index} is out of range"))
        };
        Ok((port(self.p2p)?, port(self.rpc)?, port(self.prometheus)?))
    }
}

impl Default for Ports {
    fn default() -> Self {
        Self {
            p2p: DEFAULT_P2P_PORT,
            rpc: DEFAULT_RPC_PORT,
            prometheus: DEFAULT_PROMETHEUS_PORT,
        }
    }
}

/// How the controller reaches the hosts
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Ssh {
    /// `gcloud compute ssh` / `scp`, by instance name
    Gcloud {
        project: String,
        zone: String,
        /// Through an IAP tunnel, for instances without a public address
        #[serde(default = "default_true")]
        iap: bool,
    },
    /// Plain `ssh` / `scp`, by destination (`user@host`, or a `Host` from `~/.ssh/config`)
    Ssh {
        /// Extra options for both, such as `["-i", "~/.ssh/devnet", "-p", "2222"]`
        #[serde(default)]
        options: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    /// The gcloud instance name, or the ssh destination
    pub name: String,
    /// The instance's zone, where it differs from `ssh.zone`
    #[serde(default)]
    pub zone: Option<String>,
    /// The address the other hosts reach this one on; by default, its first address
    #[serde(default)]
    pub address: Option<String>,
    /// This host's ports, where they differ from the devnet's `[ports]`
    #[serde(default)]
    pub ports: Option<Ports>,
    #[serde(default)]
    pub validators: Vec<String>,
    #[serde(default)]
    pub full_nodes: Vec<String>,
}

impl Host {
    /// This host's nodes as `devnet-keys` and `devnet-nodes` order them: validators first
    pub fn nodes(&self) -> impl Iterator<Item = (&str, bool)> {
        ordered_nodes(&self.validators, &self.full_nodes)
    }
}

impl DevnetConfig {
    /// The ports `host` runs its nodes on
    pub fn ports_of<'a>(&'a self, host: &'a Host) -> &'a Ports {
        host.ports.as_ref().unwrap_or(&self.ports)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Reading `{}`: {e}", path.display()))?;
        let config: Self = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Parsing `{}`: {e}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        valid_name("devnet", &self.chain)?;
        if self.hosts.is_empty() {
            anyhow::bail!("A devnet needs at least one host");
        }
        let mut hosts = HashSet::new();
        let mut nodes = HashSet::new();
        for host in &self.hosts {
            if !hosts.insert(&host.name) {
                anyhow::bail!("The host `{}` is listed twice", host.name);
            }
            if host.nodes().next().is_none() {
                anyhow::bail!("The host `{}` runs no nodes", host.name);
            }
            for (node, _) in host.nodes() {
                valid_name("node", node)?;
                if !nodes.insert(node) {
                    anyhow::bail!("The node `{node}` is named twice; node names are devnet-wide");
                }
            }
        }
        if self.hosts.iter().all(|h| h.validators.is_empty()) {
            anyhow::bail!("A devnet needs at least one validator");
        }
        if self.slots_per_epoch == 0 {
            anyhow::bail!("`slots_per_epoch` must be at least 1");
        }
        Ok(())
    }

    /// Every validator, in the order the committee lists them: by host, then as listed
    pub fn validators(&self) -> impl Iterator<Item = &str> {
        self.hosts
            .iter()
            .flat_map(|h| h.validators.iter().map(String::as_str))
    }
}

fn default_chain() -> String {
    String::from(DEFAULT_CHAIN)
}
fn default_version() -> String {
    String::from(DEFAULT_DEVNET_VERSION)
}
fn default_slots_per_epoch() -> u32 {
    DEFAULT_SLOTS_PER_EPOCH
}
fn default_user() -> String {
    String::from(DEFAULT_USER)
}
fn default_root() -> PathBuf {
    PathBuf::from(DEFAULT_DEVNET_ROOT)
}
fn default_p2p() -> u16 {
    DEFAULT_P2P_PORT
}
fn default_rpc() -> u16 {
    DEFAULT_RPC_PORT
}
fn default_prometheus() -> u16 {
    DEFAULT_PROMETHEUS_PORT
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod test {
    use super::*;

    const TWO_HOSTS: &str = r#"
chain = "devnet"

[ssh]
kind = "gcloud"
project = "p"
zone = "europe-west1-b"

[[hosts]]
name = "a"
validators = ["alice", "bob"]

[[hosts]]
name = "b"
zone = "europe-west1-c"
validators = ["charlie"]
full_nodes = ["archive"]

[hosts.ports]
p2p = 31333
"#;

    #[test]
    fn reads_a_two_host_devnet_with_defaults() {
        let config: DevnetConfig = toml::from_str(TWO_HOSTS).unwrap();
        config.validate().unwrap();
        assert_eq!(config.version, DEFAULT_DEVNET_VERSION);
        assert_eq!(config.ports, Ports::default());
        assert_eq!(
            config.ssh,
            Ssh::Gcloud {
                project: "p".into(),
                zone: "europe-west1-b".into(),
                iap: true
            }
        );
        assert_eq!(config.hosts[1].zone.as_deref(), Some("europe-west1-c"));
        // A host's own ports, with the rest of them defaulted
        assert_eq!(config.ports_of(&config.hosts[0]), &Ports::default());
        assert_eq!(config.ports_of(&config.hosts[1]).p2p, 31333);
        assert_eq!(config.ports_of(&config.hosts[1]).rpc, DEFAULT_RPC_PORT);
        assert_eq!(
            config.validators().collect::<Vec<_>>(),
            vec!["alice", "bob", "charlie"]
        );
    }

    #[test]
    fn each_node_on_a_host_gets_the_next_ports() {
        let ports = Ports::default();
        assert_eq!(ports.for_node(0).unwrap(), (30333, 9944, 9615));
        assert_eq!(ports.for_node(2).unwrap(), (30335, 9946, 9617));
        let high = Ports {
            p2p: 65535,
            ..Ports::default()
        };
        assert!(high.for_node(1).is_err());
    }

    #[test]
    fn refuses_a_node_named_twice_across_hosts() {
        let config: DevnetConfig =
            toml::from_str(&TWO_HOSTS.replace("\"charlie\"", "\"alice\"")).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("named twice"), "{err}");
    }

    #[test]
    fn refuses_a_devnet_without_validators() {
        let config: DevnetConfig = toml::from_str(
            r#"
[ssh]
kind = "ssh"

[[hosts]]
name = "user@10.0.0.1"
full_nodes = ["archive"]
"#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn refuses_an_unknown_key() {
        assert!(toml::from_str::<DevnetConfig>(&TWO_HOSTS.replace("chain =", "chian =")).is_err());
    }
}
