/*! A Midnight devnet: a chain of its own, spread over one or more hosts reached by SSH

An FNO host joins a network someone else runs. A devnet is a network of its own: its
genesis committee is the validators listed in a [`config::DevnetConfig`], its main chain is
the node's built-in mock follower rather than Cardano, and it can be thrown away and rebuilt.

The work is split between the machine the operator runs `midnight-installer devnet` on (the
*controller*) and the hosts the nodes run on. Every host runs this same installer, uploaded by
the controller, as two stages with plans and receipts like any other:

1. `devnet-keys` installs the pinned node release and generates each node's keys **on the
   host**. Only the public half (`public.json`) is read back: a seed never leaves its host.
2. `devnet-nodes` writes the genesis, from the committee the controller assembled out of
   every host's public keys, and a systemd unit per node, and starts them.

The first host builds the chain spec; the controller hands that same file to every other host,
because the nodes of one chain have to agree on its genesis byte for byte.
*/

pub mod config;
pub mod controller;
pub mod remote;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The node release a devnet runs unless its config says otherwise
pub const DEFAULT_DEVNET_VERSION: &str = "1.0.400";
pub const DEFAULT_CHAIN: &str = "devnet";
/// The user the nodes run as, which has to exist on every host
pub const DEFAULT_USER: &str = "midnight";
/// Where a host keeps its devnets: one directory per chain, all of it disposable
pub const DEFAULT_DEVNET_ROOT: &str = "/var/lib/midnight-devnet";
pub const DEFAULT_SLOTS_PER_EPOCH: u32 = 10;
pub const DEFAULT_P2P_PORT: u16 = 30333;
pub const DEFAULT_RPC_PORT: u16 = 9944;
pub const DEFAULT_PROMETHEUS_PORT: u16 = 9615;

/// The validator keys a devnet node needs: name, scheme, and the seed variable it is read from
///
/// The cross-chain key is also the BEEFY key, as in the node's own local environment, so
/// `BEEFY_SEED_FILE` points at the cross-chain seed rather than a key of its own.
pub const VALIDATOR_KEYS: &[(&str, &str, &str)] = &[
    ("aura", "sr25519", "AURA_SEED_FILE"),
    ("grandpa", "ed25519", "GRANDPA_SEED_FILE"),
    ("cross_chain", "ecdsa", "CROSS_CHAIN_SEED_FILE"),
];
/// Releases whose committee carries a BABE key get one more
pub const BABE_KEY: (&str, &str, &str) = ("babe", "sr25519", "BABE_SEED_FILE");

/// Every path one devnet uses on one host
#[derive(Debug, Clone)]
pub struct DevnetPaths {
    /// `<root>/<chain>`
    pub home: PathBuf,
    /// The unpacked release: `midnight-node` and the `res/` it reads relative to its cwd
    pub release_dir: PathBuf,
    pub receipt_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub nodes_dir: PathBuf,
    pub committee: PathBuf,
    pub chain_spec: PathBuf,
    pub mock_registrations: PathBuf,
    pub permissioned_candidates: PathBuf,
    pub system_parameters: PathBuf,
}

impl DevnetPaths {
    pub fn new(root: impl AsRef<Path>, chain: &str) -> Self {
        let home = root.as_ref().join(chain);
        Self {
            release_dir: home.join("release"),
            receipt_dir: home.join("receipts"),
            scratch_dir: home.join("scratch"),
            nodes_dir: home.join("nodes"),
            committee: home.join("committee.json"),
            chain_spec: home.join("chain-spec.json"),
            mock_registrations: home.join("mock-registrations.json"),
            permissioned_candidates: home.join("permissioned-candidates-config.json"),
            system_parameters: home.join("system-parameters-config.json"),
            home,
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.release_dir.join("midnight-node")
    }

    pub fn node(&self, name: &str) -> NodePaths {
        let dir = self.nodes_dir.join(name);
        NodePaths {
            secrets: dir.join("secrets"),
            node_key: dir.join("node.key"),
            public: dir.join("public.json"),
            env_file: dir.join("node.env"),
            data: dir.join("data"),
            dir,
        }
    }

    pub fn receipt(&self, stage: &str) -> PathBuf {
        self.receipt_dir.join(format!("{stage}.json"))
    }
}

/// Where one node's files are
#[derive(Debug, Clone)]
pub struct NodePaths {
    pub dir: PathBuf,
    /// Mode `700`: the seeds, which are the validator's private keys
    pub secrets: PathBuf,
    pub node_key: PathBuf,
    /// The node's public keys and peer id, which is all the controller ever reads
    pub public: PathBuf,
    pub env_file: PathBuf,
    pub data: PathBuf,
}

impl NodePaths {
    pub fn seed(&self, key: &str) -> PathBuf {
        self.secrets.join(format!("{key}.seed"))
    }
}

/** What a node is known by: its name, peer id, and (for a validator) its public keys

Written by `devnet-keys` next to the secrets, read back by the controller, and the stuff the
genesis committee is made of.
*/
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeIdentity {
    pub name: String,
    pub peer_id: String,
    pub validator: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aura_pub_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grandpa_pub_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_chain_pub_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub babe_pub_key: Option<String>,
}

impl NodeIdentity {
    fn key(&self, value: &Option<String>, key: &str) -> anyhow::Result<String> {
        value
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Validator `{}` has no {key} key", self.name))
    }

    /// The node's entry in `initial_permissioned_candidates`
    pub fn candidate(&self, with_babe: bool) -> anyhow::Result<Value> {
        let cross_chain = self.key(&self.cross_chain_pub_key, "cross-chain")?;
        let mut candidate = json!({
            "aura_pub_key": self.key(&self.aura_pub_key, "AURA")?,
            "grandpa_pub_key": self.key(&self.grandpa_pub_key, "GRANDPA")?,
            "sidechain_pub_key": cross_chain,
            "beefy_pub_key": cross_chain,
        });
        if with_babe {
            candidate["babe_pub_key"] = json!(self.key(&self.babe_pub_key, "BABE")?);
        }
        Ok(candidate)
    }
}

/** The genesis committee: every validator of the devnet, in the order the config lists them

It is the one input every host's genesis is derived from, so it is what decides whether a
chain spec already on a host is still the right one.
*/
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Committee {
    pub chain: String,
    pub slots_per_epoch: u32,
    pub validators: Vec<NodeIdentity>,
}

impl Committee {
    /// `permissioned-candidates-config.json`: the release's template, with this committee
    pub fn permissioned_candidates(&self, template: &Value) -> anyhow::Result<Value> {
        let with_babe = template_has_babe(template);
        let candidates = self
            .validators
            .iter()
            .map(|v| v.candidate(with_babe))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut config = template.clone();
        config["initial_permissioned_candidates"] = json!(candidates);
        Ok(config)
    }

    /// `system-parameters-config.json`: every seat in the committee is a permissioned one
    pub fn system_parameters(&self, template: &Value) -> Value {
        let mut config = template.clone();
        config["d_parameter"] = json!({
            "num_permissioned_candidates": self.validators.len(),
            "num_registered_candidates": 0,
        });
        config
    }

    /// What the mock main-chain follower serves as the committee, every epoch
    pub fn mock_registrations(&self) -> anyhow::Result<Value> {
        let permissioned = self
            .validators
            .iter()
            .map(|v| {
                let candidate = v.candidate(false)?;
                Ok(json!({
                    "name": v.name,
                    "sidechain_pub_key": candidate["sidechain_pub_key"],
                    "aura_pub_key": candidate["aura_pub_key"],
                    "grandpa_pub_key": candidate["grandpa_pub_key"],
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(json!([{
            "registrations": [],
            "permissioned": permissioned,
            "nonce": "0x1234",
            "d_parameter": { "permissioned": self.validators.len(), "registered": 0 },
        }]))
    }
}

/// Whether a release's committee template carries a BABE key (node 2.x does)
pub fn template_has_babe(template: &Value) -> bool {
    template["initial_permissioned_candidates"][0]
        .get("babe_pub_key")
        .is_some()
}

/// Set the epoch length in a spec from `build-spec`, whichever genesis form it uses
pub fn set_slots_per_epoch(spec: &mut Value, slots_per_epoch: u32) -> anyhow::Result<()> {
    let runtime = &mut spec["genesis"]["runtimeGenesis"];
    let form = if runtime.get("patch").is_some() {
        "patch"
    } else if runtime.get("config").is_some() {
        "config"
    } else {
        anyhow::bail!(
            "The chain spec has neither `runtimeGenesis.patch` nor `runtimeGenesis.config`"
        );
    };
    runtime[form]["sidechain"]["slotsPerEpoch"] = json!(slots_per_epoch);
    Ok(())
}

/// One node as a host runs it: its name, whether it validates, and its ports
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostNode {
    pub name: String,
    pub validator: bool,
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub prometheus_port: u16,
}

/// A host's nodes in the one order everything uses for them: validators first, as listed
///
/// The controller, the two host stages and `status` all number a host's nodes by this order
/// (their ports are the base port plus that number), so there is exactly one of it.
pub fn ordered_nodes<'a>(
    validators: &'a [String],
    full_nodes: &'a [String],
) -> impl Iterator<Item = (&'a str, bool)> {
    validators
        .iter()
        .map(|name| (name.as_str(), true))
        .chain(full_nodes.iter().map(|name| (name.as_str(), false)))
}

/// The multiaddr other nodes dial a node at, from whatever kind of address it has
pub fn bootnode_multiaddr(address: &str, port: u16, peer_id: &str) -> String {
    let address = match address.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => format!("/ip4/{ip}"),
        Ok(std::net::IpAddr::V6(ip)) => format!("/ip6/{ip}"),
        Err(_) => format!("/dns/{address}"),
    };
    format!("{address}/tcp/{port}/p2p/{peer_id}")
}

/// The systemd unit a devnet node runs as
pub fn unit_name(chain: &str, node: &str) -> String {
    format!("midnight-devnet-{chain}-{node}.service")
}

/// Render a node's systemd unit
pub fn render_unit(
    paths: &DevnetPaths,
    chain: &str,
    user: &str,
    node: &HostNode,
    bootnode: Option<&str>,
) -> String {
    let node_paths = paths.node(&node.name);
    let mut args = vec![
        format!("--chain {}", paths.chain_spec.display()),
        format!("--base-path {}", node_paths.data.display()),
        format!("--name {}", node.name),
        format!("--node-key-file {}", node_paths.node_key.display()),
        format!("--port {}", node.p2p_port),
        format!("--rpc-port {}", node.rpc_port),
        format!("--prometheus-port {}", node.prometheus_port),
        String::from("--rpc-cors=all"),
        String::from("--rpc-methods=unsafe"),
        String::from("--state-pruning archive"),
        String::from("--blocks-pruning archive"),
        String::from("--no-mdns"),
    ];
    if node.validator {
        args.push(String::from("--validator"));
    }
    if let Some(bootnode) = bootnode {
        args.push(format!("--bootnodes {bootnode}"));
    }

    format!(
        "\
[Unit]
Description=Midnight devnet {chain}: {name}{role}
Wants=network-online.target
After=network-online.target
StartLimitIntervalSec=0

[Service]
User={user}
Group={user}
Type=simple
WorkingDirectory={release_dir}
EnvironmentFile={env_file}
ExecStart={binary} \\
    {args}
Restart=on-failure
RestartSec=5
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
",
        name = node.name,
        role = if node.validator { " (validator)" } else { "" },
        release_dir = paths.release_dir.display(),
        env_file = node_paths.env_file.display(),
        binary = paths.binary().display(),
        args = args.join(" \\\n    "),
    )
}

/// Render a node's environment: the mock follower, and for a validator its seed files
pub fn render_env(paths: &DevnetPaths, node: &HostNode, with_babe: bool) -> String {
    let node_paths = paths.node(&node.name);
    let mut lines = vec![
        String::from("USE_MAIN_CHAIN_FOLLOWER_MOCK=true"),
        format!(
            "MOCK_REGISTRATIONS_FILE={}",
            paths.mock_registrations.display()
        ),
    ];
    if node.validator {
        for (key, _, variable) in VALIDATOR_KEYS {
            lines.push(format!("{variable}={}", node_paths.seed(key).display()));
        }
        lines.push(format!(
            "BEEFY_SEED_FILE={}",
            node_paths.seed("cross_chain").display()
        ));
        if with_babe {
            let (key, _, variable) = BABE_KEY;
            lines.push(format!("{variable}={}", node_paths.seed(key).display()));
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod test {
    use super::*;

    fn validator(name: &str, n: u8) -> NodeIdentity {
        NodeIdentity {
            name: name.into(),
            peer_id: format!("12D3Koo{n}"),
            validator: true,
            aura_pub_key: Some(format!("0xa{n}")),
            grandpa_pub_key: Some(format!("0xb{n}")),
            cross_chain_pub_key: Some(format!("0xc{n}")),
            babe_pub_key: None,
        }
    }

    fn committee() -> Committee {
        Committee {
            chain: "devnet".into(),
            slots_per_epoch: 10,
            validators: vec![validator("alice", 1), validator("bob", 2)],
        }
    }

    #[test]
    fn the_committee_is_every_validator_and_cross_chain_doubles_as_beefy() {
        let template = json!({
            "permissioned_candidates_policy_id": "0x00",
            "initial_permissioned_candidates": [{"aura_pub_key": "", "grandpa_pub_key": "", "sidechain_pub_key": "", "beefy_pub_key": ""}],
        });
        let config = committee().permissioned_candidates(&template).unwrap();
        let candidates = config["initial_permissioned_candidates"]
            .as_array()
            .unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[1]["sidechain_pub_key"], "0xc2");
        assert_eq!(candidates[1]["beefy_pub_key"], "0xc2");
        assert_eq!(config["permissioned_candidates_policy_id"], "0x00");
    }

    #[test]
    fn a_babe_template_needs_babe_keys() {
        let template = json!({"initial_permissioned_candidates": [{"babe_pub_key": ""}]});
        let err = committee().permissioned_candidates(&template).unwrap_err();
        assert!(err.to_string().contains("BABE"), "{err}");
    }

    #[test]
    fn every_seat_is_permissioned() {
        let params = committee().system_parameters(&json!({"terms_and_conditions": {}}));
        assert_eq!(params["d_parameter"]["num_permissioned_candidates"], 2);
        assert_eq!(params["d_parameter"]["num_registered_candidates"], 0);
        assert!(params.get("terms_and_conditions").is_some());

        let mock = committee().mock_registrations().unwrap();
        assert_eq!(mock[0]["permissioned"][0]["name"], "alice");
        assert_eq!(mock[0]["d_parameter"]["permissioned"], 2);
    }

    #[test]
    fn sets_the_epoch_length_in_either_genesis_form() {
        for form in ["patch", "config"] {
            let mut spec = json!({"genesis": {"runtimeGenesis": {form: {"sidechain": {"slotsPerEpoch": 300}}}}});
            set_slots_per_epoch(&mut spec, 10).unwrap();
            assert_eq!(
                spec["genesis"]["runtimeGenesis"][form]["sidechain"]["slotsPerEpoch"],
                10
            );
        }
        assert!(set_slots_per_epoch(&mut json!({}), 10).is_err());
    }

    #[test]
    fn a_validator_unit_validates_and_follows_the_bootnode() {
        let paths = DevnetPaths::new("/var/lib/midnight-devnet", "devnet");
        let node = HostNode {
            name: "bob".into(),
            validator: true,
            p2p_port: 30334,
            rpc_port: 9945,
            prometheus_port: 9616,
        };
        let unit = render_unit(
            &paths,
            "devnet",
            "midnight",
            &node,
            Some("/ip4/10.0.0.1/tcp/30333/p2p/12D3"),
        );
        assert!(
            unit.contains("--chain /var/lib/midnight-devnet/devnet/chain-spec.json"),
            "{unit}"
        );
        assert!(unit.contains("--port 30334"), "{unit}");
        assert!(unit.contains("--validator"), "{unit}");
        assert!(
            unit.contains("--bootnodes /ip4/10.0.0.1/tcp/30333/p2p/12D3"),
            "{unit}"
        );
        assert!(
            unit.contains("WorkingDirectory=/var/lib/midnight-devnet/devnet/release"),
            "{unit}"
        );

        let env = render_env(&paths, &node, false);
        assert!(env.contains("USE_MAIN_CHAIN_FOLLOWER_MOCK=true\n"));
        assert!(env.contains(
            "BEEFY_SEED_FILE=/var/lib/midnight-devnet/devnet/nodes/bob/secrets/cross_chain.seed\n"
        ));
        assert!(!env.contains("BABE_SEED_FILE"));
    }

    #[test]
    fn dials_the_bootnode_by_whatever_address_it_has() {
        assert_eq!(
            bootnode_multiaddr("10.0.0.1", 30333, "12D3"),
            "/ip4/10.0.0.1/tcp/30333/p2p/12D3"
        );
        assert_eq!(
            bootnode_multiaddr("fd00::1", 30333, "12D3"),
            "/ip6/fd00::1/tcp/30333/p2p/12D3"
        );
        assert_eq!(
            bootnode_multiaddr("devnet-0.internal", 30333, "12D3"),
            "/dns/devnet-0.internal/tcp/30333/p2p/12D3"
        );
    }

    #[test]
    fn a_full_node_has_no_seeds() {
        let paths = DevnetPaths::new("/r", "devnet");
        let node = HostNode {
            name: "archive".into(),
            validator: false,
            p2p_port: 30333,
            rpc_port: 9944,
            prometheus_port: 9615,
        };
        assert!(!render_unit(&paths, "devnet", "midnight", &node, None).contains("--validator"));
        assert!(!render_env(&paths, &node, false).contains("SEED_FILE"));
    }
}
