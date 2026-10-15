/*! `devnet up`, `devnet status` and `devnet down`, run from the operator's machine

`up` runs in four steps, every one safe to repeat:

1. On every host at once: upload this installer, and run `devnet-keys` with it.
2. Read back every node's `public.json`, and assemble the committee from the validators.
3. On the first host: run `devnet-nodes`, which builds the chain spec and starts its nodes.
4. On every other host at once: hand it that chain spec, and run `devnet-nodes`.

The first host's first node is the bootnode every other node dials.
*/

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use crate::devnet::config::{DevnetConfig, NodePorts, Ports};
use crate::devnet::remote::{sh_quote, Remote};
use crate::devnet::{bootnode_multiaddr, unit_name, Committee, DevnetPaths, NodeIdentity};

/// The installer as the hosts run it
const REMOTE_BINARY: &str = "bin/midnight-installer";

/// What the controller needs to run anything on one host
#[derive(Debug, Clone)]
struct Target {
    remote: Remote,
    paths: DevnetPaths,
    config: Arc<DevnetConfig>,
}

impl Target {
    fn name(&self) -> &str {
        self.remote.name()
    }

    fn ports(&self) -> &Ports {
        self.config.ports_of(&self.remote.host)
    }

    /// This host's nodes with their ports, in the order the host stages number them
    fn nodes(&self) -> anyhow::Result<Vec<(&str, bool, NodePorts)>> {
        self.remote
            .host
            .nodes()
            .enumerate()
            .map(|(i, (name, validator))| Ok((name, validator, self.ports().for_node(i)?)))
            .collect()
    }

    fn binary(&self) -> String {
        sh_quote(&self.paths.home.join(REMOTE_BINARY).display().to_string())
    }

    /// The flags both devnet stages take, for this host
    fn host_flags(&self) -> String {
        let mut flags = vec![
            format!(
                "--root {}",
                sh_quote(&self.config.root.display().to_string())
            ),
            format!("--chain {}", sh_quote(&self.config.chain)),
            format!("--user {}", sh_quote(&self.config.user)),
        ];
        for (name, validator) in self.remote.host.nodes() {
            let flag = if validator {
                "--validator"
            } else {
                "--full-node"
            };
            flags.push(format!("{flag} {}", sh_quote(name)));
        }
        flags.join(" ")
    }

    /// Run a stage of this installer on the host, as root, showing what it does
    async fn install(&self, stage: &str, flags: &str) -> anyhow::Result<()> {
        self.remote
            .run_shown(&format!(
                "sudo {binary} install --no-confirm {stage} {flags}",
                binary = self.binary(),
            ))
            .await
    }

    /// A fresh private directory on the host, for files a stage is handed
    ///
    /// Made by `mktemp`, so no one can have put a file there first: the installer and the
    /// genesis go through it, and root reads them.
    async fn staging_dir(&self) -> anyhow::Result<String> {
        let dir = self
            .remote
            .run_stdout("mktemp -d /tmp/midnight-devnet.XXXXXXXX")
            .await?;
        let dir = dir.trim().to_string();
        if !dir.starts_with("/tmp/midnight-devnet.") {
            anyhow::bail!("`mktemp` on `{}` returned `{dir}`", self.name());
        }
        Ok(dir)
    }

    /// Copy a local file into the staging directory, for a stage to read
    async fn stage_file(&self, dir: &str, local: &Path, name: &str) -> anyhow::Result<String> {
        let remote_path = format!("{dir}/{name}");
        self.remote.upload(local, &remote_path).await?;
        Ok(remote_path)
    }

    async fn remove_staging_dir(&self, dir: &str) -> anyhow::Result<()> {
        self.remote
            .run(&format!("rm -rf {}", sh_quote(dir)))
            .await
            .map(|_| ())
    }
}

fn targets(config: &DevnetConfig) -> Vec<Target> {
    let config = Arc::new(config.clone());
    config
        .hosts
        .iter()
        .map(|host| Target {
            remote: Remote::new(host.clone(), config.ssh.clone()),
            paths: DevnetPaths::new(&config.root, &config.chain),
            config: Arc::clone(&config),
        })
        .collect()
}

/// Run `f` on every host at once, and wait for all of them; every failure is reported
async fn on_every_host<T, F, Fut>(targets: &[Target], f: F) -> anyhow::Result<Vec<T>>
where
    T: Send + 'static,
    F: Fn(Target) -> Fut,
    Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    let handles: Vec<_> = targets
        .iter()
        .map(|target| (target.name().to_string(), tokio::spawn(f(target.clone()))))
        .collect();

    let mut results = vec![];
    let mut errors = vec![];
    for (host, handle) in handles {
        match handle.await {
            Ok(Ok(value)) => results.push(value),
            Ok(Err(e)) => errors.push(format!("{host}: {e:#}")),
            Err(e) => errors.push(format!("{host}: {e}")),
        }
    }
    if !errors.is_empty() {
        anyhow::bail!("{}", errors.join("\n"));
    }
    Ok(results)
}

/// What step 1 learns about one host
struct Prepared {
    identities: Vec<NodeIdentity>,
    /// The address the other hosts dial this one at
    address: String,
}

/// Step 1, on one host: put this installer there, and generate its nodes' keys
async fn prepare(target: Target, binary: PathBuf) -> anyhow::Result<Prepared> {
    let remote = &target.remote;
    remote
        .run("sudo -n true")
        .await
        .context("The SSH user needs passwordless `sudo`")?;

    println!("[{}] Uploading the installer", remote.name());
    let staging = target.staging_dir().await?;
    let result = async {
        let upload = target.stage_file(&staging, &binary, "midnight-installer").await?;
        remote
            .run(&format!(
                "sudo install -D -m 0755 {upload} {binary} && {binary} --version",
                upload = sh_quote(&upload),
                binary = target.binary(),
            ))
            .await
            .context(
                "The uploaded installer does not run there. It has to be a static build: \
                 `cargo build --release --target x86_64-unknown-linux-musl`, then pass it with `--binary`",
            )?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    target.remove_staging_dir(&staging).await?;
    result?;

    let mut flags = format!(
        "{} --version {}",
        target.host_flags(),
        sh_quote(&target.config.version)
    );
    if let Some(sha256) = &target.config.sha256 {
        flags.push_str(&format!(" --sha256 {}", sh_quote(sha256)));
    }
    target.install("devnet-keys", &flags).await?;

    // Only the public half is read back: the seeds stay where they were generated
    let mut identities = vec![];
    for (name, _) in remote.host.nodes() {
        let public = target.paths.node(name).public;
        let contents = remote
            .run_stdout(&format!(
                "sudo cat {}",
                sh_quote(&public.display().to_string())
            ))
            .await?;
        let identity: NodeIdentity = serde_json::from_str(&contents)
            .with_context(|| format!("Parsing `{}` on `{}`", public.display(), remote.name()))?;
        identities.push(identity);
    }

    let address = match &remote.host.address {
        Some(address) => address.clone(),
        None => remote
            .run_stdout("hostname -I")
            .await?
            .split_whitespace()
            .next()
            .map(str::to_string)
            .with_context(|| format!("`{}` reports no address; set `address`", remote.name()))?,
    };

    Ok(Prepared {
        identities,
        address,
    })
}

/// The committee, from every host's identities, in the order the config lists the validators
fn committee(config: &DevnetConfig, identities: &[NodeIdentity]) -> anyhow::Result<Committee> {
    let validators = config
        .validators()
        .map(|name| {
            identities
                .iter()
                .find(|i| i.name == name && i.validator)
                .cloned()
                .with_context(|| format!("No validator keys were read back for `{name}`"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Committee {
        chain: config.chain.clone(),
        slots_per_epoch: config.slots_per_epoch,
        validators,
    })
}

/// The flags of `devnet-nodes`, apart from the files it is handed
fn nodes_flags(target: &Target, bootnode: &str) -> String {
    format!(
        "{host} --bootnode {bootnode} --p2p-port {p2p} --rpc-port {rpc} --prometheus-port {prometheus}",
        host = target.host_flags(),
        bootnode = sh_quote(bootnode),
        p2p = target.ports().p2p,
        rpc = target.ports().rpc,
        prometheus = target.ports().prometheus,
    )
}

/// Steps 3 and 4, on one host: hand it the genesis, and run `devnet-nodes`
async fn start_nodes(
    target: Target,
    bootnode: String,
    committee_file: PathBuf,
    spec_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let staging = target.staging_dir().await?;
    let result = async {
        let mut flags = format!(
            "{} --committee {}",
            nodes_flags(&target, &bootnode),
            sh_quote(
                &target
                    .stage_file(&staging, &committee_file, "committee.json")
                    .await?
            )
        );
        if let Some(spec_file) = &spec_file {
            flags.push_str(&format!(
                " --chain-spec {}",
                sh_quote(
                    &target
                        .stage_file(&staging, spec_file, "chain-spec.json")
                        .await?
                )
            ));
        }
        target.install("devnet-nodes", &flags).await
    }
    .await;
    target.remove_staging_dir(&staging).await?;
    result
}

/// Write `contents` to a new file under the system temporary directory
fn local_file(name: &str, contents: &[u8]) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("midnight-devnet-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).with_context(|| format!("Writing `{}`", path.display()))?;
    Ok(path)
}

/// Bring the devnet up, or bring it in line with the config
pub async fn up(config: &DevnetConfig, binary: PathBuf) -> anyhow::Result<()> {
    let targets = targets(config);

    println!(
        "==> Installing the node and generating keys on {} host(s)",
        targets.len()
    );
    let prepared = on_every_host(&targets, |target| prepare(target, binary.clone())).await?;

    let identities: Vec<NodeIdentity> = prepared
        .iter()
        .flat_map(|p| p.identities.iter().cloned())
        .collect();
    let committee = committee(config, &identities)?;

    let first = &targets[0];
    let bootnode = bootnode_multiaddr(
        &prepared[0].address,
        first.ports().p2p,
        &prepared[0].identities[0].peer_id,
    );

    let committee_file = local_file("committee.json", &serde_json::to_vec_pretty(&committee)?)?;
    let result = async {
        println!(
            "==> Building the genesis on `{}`, and starting its nodes",
            first.name()
        );
        start_nodes(
            first.clone(),
            bootnode.clone(),
            committee_file.clone(),
            None,
        )
        .await?;

        let rest = &targets[1..];
        if rest.is_empty() {
            return Ok(());
        }

        // One genesis for the whole chain: the first host's, byte for byte
        let spec = first
            .remote
            .run(&format!(
                "sudo cat {}",
                sh_quote(&first.paths.chain_spec.display().to_string())
            ))
            .await?
            .stdout;
        let spec_file = local_file("chain-spec.json", &spec)?;

        println!("==> Starting the nodes on {} more host(s)", rest.len());
        let started = on_every_host(rest, |target| {
            start_nodes(
                target,
                bootnode.clone(),
                committee_file.clone(),
                Some(spec_file.clone()),
            )
        })
        .await;
        let _ = std::fs::remove_file(&spec_file);
        started.map(|_| ())
    }
    .await;
    let _ = std::fs::remove_file(&committee_file);
    result?;

    println!("\nDevnet `{}` is up. Bootnode: {bootnode}", config.chain);
    for (target, prepared) in targets.iter().zip(&prepared) {
        for ((_, _, (p2p, rpc, _)), identity) in target.nodes()?.iter().zip(&prepared.identities) {
            println!(
                "  {host:<28} {name:<12} {role:<9} p2p {address}:{p2p}  rpc 127.0.0.1:{rpc}  peer {peer}",
                host = target.name(),
                name = identity.name,
                role = if identity.validator { "validator" } else { "full node" },
                address = prepared.address,
                peer = identity.peer_id,
            );
        }
    }
    println!("\nWatch it with `midnight-installer devnet status <config>`.");
    Ok(())
}

/// One node's state, as its unit and its Prometheus metrics report it
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NodeStatus {
    pub active: String,
    pub best: Option<u64>,
    pub finalized: Option<u64>,
    pub peers: Option<u64>,
}

/// Read `substrate_block_height{status="best"|"finalized"}` and the peer count from metrics
pub fn parse_metrics(metrics: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    let value = |line: &str| {
        line.rsplit(' ')
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v as u64)
    };
    let (mut best, mut finalized, mut peers) = (None, None, None);
    for line in metrics.lines() {
        if line.starts_with("substrate_block_height{") {
            if line.contains("status=\"best\"") {
                best = value(line);
            } else if line.contains("status=\"finalized\"") {
                finalized = value(line);
            }
        } else if line.starts_with("substrate_sub_libp2p_peers_count")
            || line.starts_with("substrate_sync_peers")
        {
            peers = peers.or_else(|| value(line));
        }
    }
    (best, finalized, peers)
}

/// The script which reports every node of one host: a header line, then its state
fn status_script(target: &Target) -> anyhow::Result<String> {
    Ok(target
        .nodes()?
        .into_iter()
        .map(|(name, _, (_, _, prometheus))| {
            let unit = unit_name(&target.config.chain, name);
            format!(
                "echo '@@ {name}'; systemctl is-active {unit} || true; \
                 curl -s -m 3 http://127.0.0.1:{prometheus}/metrics | grep -E '^substrate_(block_height|sub_libp2p_peers_count|sync_peers)' || true",
                unit = sh_quote(&unit),
            )
        })
        .collect::<Vec<_>>()
        .join("; "))
}

fn parse_status(output: &str) -> Vec<(String, NodeStatus)> {
    let mut nodes = vec![];
    for block in output.split("@@ ").skip(1) {
        let mut lines = block.lines();
        let name = lines.next().unwrap_or_default().trim().to_string();
        let active = lines.next().unwrap_or_default().trim().to_string();
        let rest: Vec<&str> = lines.collect();
        let (best, finalized, peers) = parse_metrics(&rest.join("\n"));
        nodes.push((
            name,
            NodeStatus {
                active,
                best,
                finalized,
                peers,
            },
        ));
    }
    nodes
}

/// Report every node of the devnet: its service, its best and finalized block, its peers
pub async fn status(config: &DevnetConfig) -> anyhow::Result<()> {
    let targets = targets(config);
    let reports = on_every_host(&targets, |target| async move {
        let output = target.remote.run_stdout(&status_script(&target)?).await?;
        Ok((target.name().to_string(), parse_status(&output)))
    })
    .await?;

    let show = |v: Option<u64>| v.map_or_else(|| String::from("-"), |v| v.to_string());
    println!(
        "{:<28} {:<12} {:<10} {:>8} {:>10} {:>6}",
        "HOST", "NODE", "SERVICE", "BEST", "FINALIZED", "PEERS"
    );
    for (host, nodes) in reports {
        for (name, node) in nodes {
            println!(
                "{host:<28} {name:<12} {:<10} {:>8} {:>10} {:>6}",
                node.active,
                show(node.best),
                show(node.finalized),
                show(node.peers),
            );
        }
    }
    Ok(())
}

/// The script which takes one host's part of the devnet down
///
/// The receipts first, so every unit this installer started is stopped and removed the way
/// it was put there; then whatever a half-finished run left. A receipt which will not
/// uninstall does not stop the rest: the units and the directory go regardless, and the
/// script fails at the end so the operator hears about it.
fn down_script(target: &Target) -> String {
    let binary = target.binary();
    let home = sh_quote(&target.paths.home.display().to_string());
    let root = sh_quote(&target.config.root.display().to_string());
    let mut script = vec![String::from("failed=0")];
    for stage in ["devnet-nodes", "devnet-keys"] {
        let receipt = sh_quote(&target.paths.receipt(stage).display().to_string());
        script.push(format!(
            "if sudo test -f {receipt}; then sudo {binary} uninstall --no-confirm --receipt {receipt} || failed=1; fi"
        ));
    }
    // By name, not by a glob: `midnight-devnet-dev-*` would also match the units of a
    // devnet called `dev-2` on the same host
    let units = target
        .remote
        .host
        .nodes()
        .map(|(name, _)| sh_quote(&unit_name(&target.config.chain, name)))
        .collect::<Vec<_>>()
        .join(" ");
    script.push(format!(
        "for unit in {units}; do \
         if [ -e \"/etc/systemd/system/$unit\" ]; then \
         sudo systemctl disable --now \"$unit\"; sudo rm -f \"/etc/systemd/system/$unit\"; fi; done"
    ));
    script.push(String::from("sudo systemctl daemon-reload"));
    script.push(format!("sudo rm -rf {home}"));
    script.push(format!("sudo rmdir {root} 2>/dev/null || true"));
    script.push(String::from(
        "if [ \"$failed\" = 1 ]; then echo 'A receipt would not uninstall; the devnet was removed anyway' >&2; exit 1; fi",
    ));
    script.join("; ")
}

/// Stop every node and delete the devnet from every host
pub async fn down(config: &DevnetConfig) -> anyhow::Result<()> {
    let targets = targets(config);
    on_every_host(&targets, |target| async move {
        target.remote.run_shown(&down_script(&target)).await
    })
    .await?;
    println!("Devnet `{}` is down.", config.chain);
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn reads_heights_and_peers_from_metrics() {
        let metrics = "\
substrate_block_height{status=\"best\",chain=\"devnet\"} 42
substrate_block_height{status=\"finalized\",chain=\"devnet\"} 40
substrate_block_height{status=\"sync_target\",chain=\"devnet\"} 42
substrate_sub_libp2p_peers_count{chain=\"devnet\"} 3
";
        assert_eq!(parse_metrics(metrics), (Some(42), Some(40), Some(3)));
        assert_eq!(parse_metrics(""), (None, None, None));
    }

    #[test]
    fn splits_a_host_report_per_node() {
        let output =
            "@@ alice\nactive\nsubstrate_block_height{status=\"best\"} 7\n@@ archive\ninactive\n";
        let nodes = parse_status(output);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].0, "alice");
        assert_eq!(nodes[0].1.best, Some(7));
        assert_eq!(nodes[1].1.active, "inactive");
        assert_eq!(nodes[1].1.best, None);
    }

    fn two_validators() -> DevnetConfig {
        toml::from_str(
            r#"
[ssh]
kind = "ssh"
[[hosts]]
name = "a"
validators = ["bob", "alice"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn the_committee_follows_the_config_order() {
        let config = two_validators();
        let identity = |name: &str| NodeIdentity {
            name: name.into(),
            peer_id: format!("12D3{name}"),
            validator: true,
            aura_pub_key: Some("0x1".into()),
            grandpa_pub_key: Some("0x2".into()),
            cross_chain_pub_key: Some("0x3".into()),
            babe_pub_key: None,
        };
        let committee = committee(&config, &[identity("alice"), identity("bob")]).unwrap();
        assert_eq!(
            committee
                .validators
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>(),
            vec!["bob", "alice"]
        );
        assert!(super::committee(&config, &[identity("alice")]).is_err());
    }

    /// A receipt which will not uninstall must not leave the units running
    #[test]
    fn down_finishes_the_cleanup_whatever_the_receipts_do() {
        let target = targets(&two_validators()).remove(0);
        let script = down_script(&target);
        assert!(!script.contains("&&"), "{script}");
        assert!(script.contains("|| failed=1"), "{script}");
        assert!(
            script.contains("midnight-devnet-devnet-bob.service"),
            "{script}"
        );
        assert!(
            script.contains("rm -rf /var/lib/midnight-devnet/devnet;"),
            "{script}"
        );
        assert!(script.ends_with("exit 1; fi"), "{script}");
    }

    #[test]
    fn numbers_a_hosts_nodes_and_their_ports_together() {
        let target = targets(&two_validators()).remove(0);
        let nodes = target.nodes().unwrap();
        assert_eq!(nodes[0], ("bob", true, (30333, 9944, 9615)));
        assert_eq!(nodes[1], ("alice", true, (30334, 9945, 9616)));
        assert!(status_script(&target)
            .unwrap()
            .contains("127.0.0.1:9616/metrics"));
    }
}
