/*! Actions for the WireGuard tunnel identity the Foundation peers with */

pub(crate) mod generate_keys;
pub(crate) mod install_tools;

pub use generate_keys::GenerateWireguardKeys;
pub use install_tools::{installed_version, InstallWireguardTools};
