/*! Actions for `cardano-db-sync`, which turns the relay's chain into the SQL the Midnight
node queries */

pub(crate) mod install_db_sync;

pub use install_db_sync::InstallCardanoDbSync;
