/*! Configurable knobs, derived paths, and their related errors */

use anyhow::Context;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use clap::ArgAction;
use url::Url;

/// Where the installer keeps its own state (receipts, scratch space)
pub const STATE_DIR_NAME: &str = ".midnight-installer";

pub const DEFAULT_DATA_ROOT: &str = "/data";
pub const DEFAULT_CARDANO_NETWORK: &str = "preprod";
pub const DEFAULT_CARDANO_MAGIC: u32 = 1;
pub const DEFAULT_CARDANO_NODE_VERSION: &str = "10.6.2";
pub const DEFAULT_DB_SYNC_VERSION: &str = "13.6.0.7";
pub const DEFAULT_POSTGRES_VERSION: &str = "17";
pub const DEFAULT_MIDNIGHT_VERSION: &str = "0.22.2";
pub const DEFAULT_MITHRIL_VERSION: &str = "2630.0";
pub const DEFAULT_WIREGUARD_TOOLS_VERSION: &str = "v1.0.20250521";

/// The SHA-256 of each release archive the default versions install, as published alongside
/// the release (`cardano-node-<v>-sha256sums.txt`, `SHA256SUMS-amd64`, `CHECKSUM.asc`) or,
/// for `cardano-db-sync` which publishes none, as computed from the archive
const KNOWN_SHA256: &[(&str, &str)] = &[
    (
        "https://github.com/IntersectMBO/cardano-node/releases/download/10.6.2/cardano-node-10.6.2-linux-amd64.tar.gz",
        "f3d8816cd82adbcfa1b49a753e7e53b7131b3f0ce3eb70ee68c52e5df3f84e29",
    ),
    (
        "https://github.com/IntersectMBO/cardano-db-sync/releases/download/13.6.0.7/cardano-db-sync-13.6.0.7-linux.tar.gz",
        "b44f6012e15f411b124a976b9c8f11da41628ea73af506f06832cacfeb0ff818",
    ),
    (
        "https://github.com/IntersectMBO/mithril/releases/download/2630.0/mithril-2630.0-linux-x64.tar.gz",
        "248f08278382bf02d55cf16263791c53969fa4f749a48df4fdd96416ff9f31ba",
    ),
    (
        "https://github.com/midnightntwrk/midnight-node/releases/download/node-0.22.2/midnight-node-0.22.2-linux-amd64.tar.gz",
        "e3cdf87a9125c4aede2151fdd5dc12a04d34e13390ae10393904be1a33ccf640",
    ),
];

pub const DEFAULT_DB_NAME: &str = "cexplorer";
pub const DEFAULT_DB_USER: &str = "midnight";

pub const PROMETHEUS_PUSH_ENDPOINT: &str = "https://telemetry.shielded.tools/api/v1/receive";
pub const TELEMETRY_URL: &str = "wss://telemetry.shielded.tools/submit 1";

pub const CARDANO_NODE_SERVICE: &str = "cardano-node.service";
pub const CARDANO_DB_SYNC_SERVICE: &str = "cardano-db-sync.service";
pub const MIDNIGHT_NODE_SERVICE: &str = "midnight-node.service";

/** A string which must never reach a receipt, a log line, or a plan description

It serializes as `"<redacted>"`, so a receipt written next to the installed system carries
no password — reverting never needs the value, only the names of what to undo. Reading a
receipt back therefore yields a `Secret` which is a placeholder, not a credential.
*/
#[derive(Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

pub const REDACTED: &str = "<redacted>";

impl serde::Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(REDACTED)
    }
}

impl Secret {
    pub fn new(inner: impl Into<String>) -> Self {
        Self(inner.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{REDACTED}\"")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/** Settings shared by every [`Planner`](crate::planner::Planner)

Settings which only apply to one planner live on that planner.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, clap::Parser)]
pub struct CommonSettings {
    /// The root of the FNO data layout
    #[clap(
        long,
        default_value = DEFAULT_DATA_ROOT,
        env = "MIDNIGHT_INSTALLER_DATA_ROOT",
        global = true
    )]
    pub data_root: PathBuf,

    /// The Linux user which owns and runs the Cardano services
    #[clap(
        long,
        default_value_t = default_service_user(),
        env = "MIDNIGHT_INSTALLER_CARDANO_USER",
        global = true
    )]
    pub cardano_user: String,

    /// The Linux user which owns and runs the Midnight node
    #[clap(
        long,
        default_value_t = default_service_user(),
        env = "MIDNIGHT_INSTALLER_MIDNIGHT_USER",
        global = true
    )]
    pub midnight_user: String,

    /// The Cardano network to follow
    #[clap(
        long,
        default_value = DEFAULT_CARDANO_NETWORK,
        env = "MIDNIGHT_INSTALLER_CARDANO_NETWORK",
        global = true
    )]
    pub cardano_network: String,

    /// The Cardano network magic
    #[clap(
        long,
        default_value_t = DEFAULT_CARDANO_MAGIC,
        env = "MIDNIGHT_INSTALLER_CARDANO_MAGIC",
        global = true
    )]
    pub cardano_magic: u32,

    /// The `cardano-node` release to install
    #[clap(
        long,
        default_value = DEFAULT_CARDANO_NODE_VERSION,
        env = "MIDNIGHT_INSTALLER_CARDANO_NODE_VERSION",
        global = true
    )]
    pub cardano_node_version: String,

    /// The SHA-256 of the `cardano-node` release archive (known for the default version)
    #[clap(long, env = "MIDNIGHT_INSTALLER_CARDANO_NODE_SHA256", global = true)]
    pub cardano_node_sha256: Option<String>,

    /// The `cardano-db-sync` release to install
    #[clap(
        long,
        default_value = DEFAULT_DB_SYNC_VERSION,
        env = "MIDNIGHT_INSTALLER_DB_SYNC_VERSION",
        global = true
    )]
    pub db_sync_version: String,

    /// The SHA-256 of the `cardano-db-sync` release archive (known for the default version)
    #[clap(long, env = "MIDNIGHT_INSTALLER_DB_SYNC_SHA256", global = true)]
    pub db_sync_sha256: Option<String>,

    /// The Mithril distribution to install the client from
    #[clap(
        long,
        default_value = DEFAULT_MITHRIL_VERSION,
        env = "MIDNIGHT_INSTALLER_MITHRIL_VERSION",
        global = true
    )]
    pub mithril_version: String,

    /// The SHA-256 of the Mithril release archive (known for the default version)
    #[clap(long, env = "MIDNIGHT_INSTALLER_MITHRIL_SHA256", global = true)]
    pub mithril_sha256: Option<String>,

    /// The PostgreSQL major version to install
    #[clap(
        long,
        default_value = DEFAULT_POSTGRES_VERSION,
        env = "MIDNIGHT_INSTALLER_POSTGRES_VERSION",
        global = true
    )]
    pub postgres_version: String,

    /// The `midnight-node` release to install
    #[clap(
        long,
        default_value = DEFAULT_MIDNIGHT_VERSION,
        env = "MIDNIGHT_INSTALLER_MIDNIGHT_VERSION",
        global = true
    )]
    pub midnight_version: String,

    /// The SHA-256 of the `midnight-node` release archive (known for the default version)
    #[clap(long, env = "MIDNIGHT_INSTALLER_MIDNIGHT_SHA256", global = true)]
    pub midnight_sha256: Option<String>,

    /// The `wireguard-tools` tag the FNO programme requires
    #[clap(
        long,
        default_value = DEFAULT_WIREGUARD_TOOLS_VERSION,
        env = "MIDNIGHT_INSTALLER_WIREGUARD_TOOLS_VERSION",
        global = true
    )]
    pub wireguard_tools_version: String,

    /// Install the APT packages the steps assume are present
    #[clap(
        long = "no-install-base-packages",
        action(ArgAction::SetFalse),
        default_value = "true",
        env = "MIDNIGHT_INSTALLER_INSTALL_BASE_PACKAGES",
        global = true
    )]
    pub install_base_packages: bool,
}

impl Default for CommonSettings {
    fn default() -> Self {
        Self {
            data_root: PathBuf::from(DEFAULT_DATA_ROOT),
            cardano_user: default_service_user(),
            midnight_user: default_service_user(),
            cardano_network: DEFAULT_CARDANO_NETWORK.into(),
            cardano_magic: DEFAULT_CARDANO_MAGIC,
            cardano_node_version: DEFAULT_CARDANO_NODE_VERSION.into(),
            cardano_node_sha256: None,
            db_sync_version: DEFAULT_DB_SYNC_VERSION.into(),
            db_sync_sha256: None,
            mithril_version: DEFAULT_MITHRIL_VERSION.into(),
            mithril_sha256: None,
            postgres_version: DEFAULT_POSTGRES_VERSION.into(),
            midnight_version: DEFAULT_MIDNIGHT_VERSION.into(),
            midnight_sha256: None,
            wireguard_tools_version: DEFAULT_WIREGUARD_TOOLS_VERSION.into(),
            install_base_packages: true,
        }
    }
}

impl CommonSettings {
    pub fn paths(&self) -> Paths {
        Paths::new(&self.data_root, &self.cardano_network)
    }

    pub fn cardano_node_archive(&self) -> anyhow::Result<ReleaseArchive> {
        ReleaseArchive::new(
            format!(
                "https://github.com/IntersectMBO/cardano-node/releases/download/{version}/cardano-node-{version}-linux-amd64.tar.gz",
                version = self.cardano_node_version,
            ),
            self.cardano_node_sha256.as_deref(),
            "cardano-node-sha256",
        )
    }

    pub fn db_sync_archive(&self) -> anyhow::Result<ReleaseArchive> {
        ReleaseArchive::new(
            format!(
                "https://github.com/IntersectMBO/cardano-db-sync/releases/download/{version}/cardano-db-sync-{version}-linux.tar.gz",
                version = self.db_sync_version,
            ),
            self.db_sync_sha256.as_deref(),
            "db-sync-sha256",
        )
    }

    pub fn mithril_archive(&self) -> anyhow::Result<ReleaseArchive> {
        ReleaseArchive::new(
            format!(
                "https://github.com/IntersectMBO/mithril/releases/download/{version}/mithril-{version}-linux-x64.tar.gz",
                version = self.mithril_version,
            ),
            self.mithril_sha256.as_deref(),
            "mithril-sha256",
        )
    }

    pub fn midnight_archive(&self) -> anyhow::Result<ReleaseArchive> {
        ReleaseArchive::new(
            format!(
                "https://github.com/midnightntwrk/midnight-node/releases/download/node-{version}/midnight-node-{version}-linux-amd64.tar.gz",
                version = self.midnight_version,
            ),
            self.midnight_sha256.as_deref(),
            "midnight-sha256",
        )
    }

    pub fn db_sync_config_url(&self) -> anyhow::Result<Url> {
        parse_url(format!(
            "https://book.world.dev.cardano.org/environments/{network}/db-sync-config.json",
            network = self.cardano_network,
        ))
    }

    pub fn mithril_aggregator_endpoint(&self) -> String {
        format!(
            "https://aggregator.release-{network}.api.mithril.network/aggregator",
            network = self.cardano_network,
        )
    }

    pub fn mithril_genesis_vkey_url(&self) -> anyhow::Result<Url> {
        parse_url(format!(
            "https://raw.githubusercontent.com/IntersectMBO/mithril/main/mithril-infra/configuration/release-{network}/genesis.vkey",
            network = self.cardano_network,
        ))
    }

    pub fn mithril_ancillary_vkey_url(&self) -> anyhow::Result<Url> {
        parse_url(format!(
            "https://raw.githubusercontent.com/IntersectMBO/mithril/main/mithril-infra/configuration/release-{network}/ancillary.vkey",
            network = self.cardano_network,
        ))
    }

    pub fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        let Self {
            data_root,
            cardano_user,
            midnight_user,
            cardano_network,
            cardano_magic,
            cardano_node_version,
            cardano_node_sha256,
            db_sync_version,
            db_sync_sha256,
            mithril_version,
            mithril_sha256,
            postgres_version,
            midnight_version,
            midnight_sha256,
            wireguard_tools_version,
            install_base_packages,
        } = self;

        let mut map = HashMap::default();
        map.insert(
            "data_root".into(),
            serde_json::to_value(data_root.display().to_string())?,
        );
        map.insert("cardano_user".into(), serde_json::to_value(cardano_user)?);
        map.insert("midnight_user".into(), serde_json::to_value(midnight_user)?);
        map.insert(
            "cardano_network".into(),
            serde_json::to_value(cardano_network)?,
        );
        map.insert("cardano_magic".into(), serde_json::to_value(cardano_magic)?);
        map.insert(
            "cardano_node_version".into(),
            serde_json::to_value(cardano_node_version)?,
        );
        map.insert(
            "cardano_node_sha256".into(),
            serde_json::to_value(cardano_node_sha256)?,
        );
        map.insert(
            "db_sync_version".into(),
            serde_json::to_value(db_sync_version)?,
        );
        map.insert(
            "db_sync_sha256".into(),
            serde_json::to_value(db_sync_sha256)?,
        );
        map.insert(
            "mithril_version".into(),
            serde_json::to_value(mithril_version)?,
        );
        map.insert(
            "mithril_sha256".into(),
            serde_json::to_value(mithril_sha256)?,
        );
        map.insert(
            "postgres_version".into(),
            serde_json::to_value(postgres_version)?,
        );
        map.insert(
            "midnight_version".into(),
            serde_json::to_value(midnight_version)?,
        );
        map.insert(
            "midnight_sha256".into(),
            serde_json::to_value(midnight_sha256)?,
        );
        map.insert(
            "wireguard_tools_version".into(),
            serde_json::to_value(wireguard_tools_version)?,
        );
        map.insert(
            "install_base_packages".into(),
            serde_json::to_value(install_base_packages)?,
        );

        Ok(map)
    }
}

/** A release archive and the SHA-256 it has to match before anything is installed from it

The checksum comes from the operator's setting when given, else from [`KNOWN_SHA256`] for the
versions this installer was built against. A version this installer does not know, with no
checksum given, is refused rather than installed unverified.
*/
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ReleaseArchive {
    pub url: Url,
    pub sha256: String,
}

impl ReleaseArchive {
    fn new(url: String, sha256: Option<&str>, flag: &str) -> anyhow::Result<Self> {
        let sha256 = sha256
            .or_else(|| {
                KNOWN_SHA256
                    .iter()
                    .find(|(known_url, _)| *known_url == url)
                    .map(|(_, sha256)| *sha256)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("No SHA-256 is known for `{url}`, so it cannot be verified after download. Pass `--{flag}` with the checksum the project publishes for that release.")
            })?
            .trim()
            .to_ascii_lowercase();

        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("`{sha256}` is not a SHA-256: expected 64 hex digits");
        }

        Ok(Self {
            url: parse_url(url)?,
            sha256,
        })
    }
}

/** The Cardano epoch geometry the Midnight node needs to follow the main chain

These are properties of the Cardano network being followed, not of this host, so they are
only known for networks this installer has them for. A network without them is refused
rather than started with another network's numbers.
*/
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct MainChainParams {
    pub security_parameter: u32,
    pub active_slots_coeff: f64,
    pub block_stability_margin: u32,
    pub first_epoch_timestamp_millis: u64,
    pub first_epoch_number: u32,
    pub epoch_duration_millis: u64,
    pub first_slot_number: u64,
    pub slot_duration_millis: u64,
}

impl MainChainParams {
    pub fn preprod() -> Self {
        Self {
            security_parameter: 2160,
            active_slots_coeff: 0.05,
            block_stability_margin: 30,
            first_epoch_timestamp_millis: 1_655_769_600_000,
            first_epoch_number: 4,
            epoch_duration_millis: 432_000_000,
            first_slot_number: 86_400,
            slot_duration_millis: 1_000,
        }
    }

    pub fn for_network(network: &str) -> Option<Self> {
        match network {
            DEFAULT_CARDANO_NETWORK => Some(Self::preprod()),
            _ => None,
        }
    }
}

/// Every path the FNO data layout is made of, derived from the data root
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_root: PathBuf,
    pub state_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub receipt_dir: PathBuf,
    pub cardano_data: PathBuf,
    pub cardano_db: PathBuf,
    pub cardano_socket: PathBuf,
    pub cardano_schema: PathBuf,
    pub cardano_db_sync_state: PathBuf,
    pub cardano_db_sync_config: PathBuf,
    pub mithril_tools: PathBuf,
    pub postgres_data: PathBuf,
    pub postgres_credentials_file: PathBuf,
    pub midnight_data: PathBuf,
    pub midnight_node_data: PathBuf,
    pub midnight_keys_dir: PathBuf,
    pub midnight_res_dir: PathBuf,
    pub midnight_registration_file: PathBuf,
    pub midnight_runtime_data: PathBuf,
    pub midnight_network_dir: PathBuf,
    pub midnight_keystore_dir: PathBuf,
    pub midnight_env_file: PathBuf,
    pub midnight_chain_spec: PathBuf,
    pub wireguard_data: PathBuf,
}

impl Paths {
    pub fn new(data_root: impl AsRef<Path>, cardano_network: &str) -> Self {
        let data_root = data_root.as_ref().to_path_buf();
        let state_dir = data_root.join(STATE_DIR_NAME);
        let cardano_data = data_root.join("cardano");
        let postgres_data = data_root.join("postgresql");
        let midnight_data = data_root.join("midnight");
        let midnight_node_data = data_root.join("midnight_node");
        let midnight_runtime_data = midnight_node_data.join("data");
        let chain_dir = midnight_runtime_data
            .join("chains")
            .join(format!("midnight_{cardano_network}"));

        Self {
            scratch_dir: state_dir.join("scratch"),
            receipt_dir: state_dir.join("receipts"),
            cardano_db: cardano_data.join("db"),
            cardano_socket: cardano_data.join("db").join("node.socket"),
            cardano_schema: cardano_data.join("schema"),
            cardano_db_sync_state: cardano_data.join("db-sync-state"),
            cardano_db_sync_config: cardano_data.join("db-sync-config.json"),
            mithril_tools: cardano_data.join("mithril-tools"),
            postgres_credentials_file: postgres_data.join("fno-db-credentials.env"),
            midnight_keys_dir: midnight_node_data.join("keys"),
            midnight_res_dir: midnight_node_data.join("res"),
            midnight_registration_file: midnight_node_data.join("partner-chains-public-keys.json"),
            midnight_network_dir: chain_dir.join("network"),
            midnight_keystore_dir: chain_dir.join("keystore"),
            midnight_env_file: midnight_node_data.join(".env"),
            midnight_chain_spec: midnight_node_data
                .join("res")
                .join(cardano_network)
                .join("chain-spec-raw.json"),
            wireguard_data: data_root.join("wireguard"),
            cardano_data,
            postgres_data,
            midnight_data,
            midnight_node_data,
            midnight_runtime_data,
            state_dir,
            data_root,
        }
    }

    /// The top level directories the FNO data layout is made of
    pub fn base_directories(&self) -> Vec<PathBuf> {
        vec![
            self.cardano_data.clone(),
            self.midnight_data.clone(),
            self.midnight_node_data.clone(),
            self.postgres_data.clone(),
            self.wireguard_data.clone(),
        ]
    }

    pub fn receipt(&self, planner: &str) -> PathBuf {
        self.receipt_dir.join(format!("{planner}.json"))
    }

    pub fn scratch(&self, name: &str) -> PathBuf {
        self.scratch_dir.join(name)
    }

    pub fn postgres_cluster(&self, postgres_version: &str) -> PathBuf {
        self.postgres_data.join(postgres_version).join("main")
    }
}

/// The default Linux user for the services: the invoking (pre-`sudo`) user, else `ubuntu`
pub fn default_service_user() -> String {
    let sudo_user = std::env::var("SUDO_USER").unwrap_or_default();
    if !sudo_user.is_empty() && sudo_user != "root" {
        return sudo_user;
    }
    if user_exists("ubuntu") {
        return "ubuntu".into();
    }
    sudo_user
}

pub fn user_exists(user: &str) -> bool {
    matches!(nix::unistd::User::from_name(user), Ok(Some(_)))
}

/// The home directory of `user`, as `getent passwd` would report it
pub fn user_home(user: &str) -> anyhow::Result<PathBuf> {
    let user_record = nix::unistd::User::from_name(user)
        .with_context(|| format!("Getting user `{user}`"))?
        .with_context(|| format!("No user `{user}` on this system, create it first"))?;
    if !user_record.dir.is_dir() {
        anyhow::bail!(
            "The home directory of `{user}` (`{}`) does not exist",
            user_record.dir.display()
        );
    }
    Ok(user_record.dir)
}

/// `~user/.local/bin`, where the FNO binaries live
pub fn user_bin_dir(user: &str) -> anyhow::Result<PathBuf> {
    Ok(user_home(user)?.join(".local").join("bin"))
}

/// `~user/.local/share`, where the chain configuration lives
pub fn user_share_dir(user: &str) -> anyhow::Result<PathBuf> {
    Ok(user_home(user)?.join(".local").join("share"))
}

fn parse_url(url: String) -> anyhow::Result<Url> {
    Url::parse(&url).with_context(|| format!("Parsing URL `{url}`"))
}
