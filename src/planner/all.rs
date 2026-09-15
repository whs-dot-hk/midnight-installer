/*! The whole FNO host, as one plan

Every other planner covers one component. This one covers the host: it asks each of them for
its actions and lays them end to end, so the build-out is a single plan to read, a single
confirmation, and a single receipt which unwinds the lot in reverse.

That is possible because no stage waits for another to have *caught up*, only for it to have
*run*. The services below each other retry: db-sync follows a relay which is still syncing,
and the node follows a db-sync which is still filling. Within one plan each stage's inputs
are produced by actions a few steps earlier, which is why the checks for them happen when
they run rather than when they are planned.
*/

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::action::{Action, StatefulAction};
use crate::planner::{
    cardano::Cardano, db_sync::DbSync, diff_from_default, directories::Directories,
    midnight::Midnight, platform_check, require_root, require_user, validator::Validator,
    wireguard::Wireguard, Planner,
};
use crate::settings::CommonSettings;

/// Everything, in the order the runbook imposes
///
/// Its settings are exactly the validator's: the last stage is the one which needs every
/// setting the stages before it took, so there is nothing to add.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, clap::Parser)]
pub struct All {
    #[clap(flatten)]
    #[serde(flatten)]
    pub validator: Validator,
}

impl All {
    pub fn common(&self) -> &CommonSettings {
        &self.validator.common
    }

    /// The common settings the sub-planners are given
    ///
    /// The base packages are one action at the top of this plan rather than one per stage,
    /// so the stages are told not to plan them: each would otherwise ask `dpkg` about the
    /// same dozen packages again, only for the duplicates to be dropped.
    fn stage_common(&self) -> CommonSettings {
        CommonSettings {
            install_base_packages: false,
            ..self.common().clone()
        }
    }

    fn directories(&self) -> Directories {
        Directories {
            common: self.stage_common(),
        }
    }

    fn cardano(&self) -> Cardano {
        Cardano {
            common: self.stage_common(),
        }
    }

    fn db_sync(&self) -> DbSync {
        DbSync {
            common: self.stage_common(),
            postgres_password: self.validator.postgres_password.clone(),
            database_user: self.validator.database_user.clone(),
            database_name: self.validator.database_name.clone(),
        }
    }

    fn midnight(&self) -> Midnight {
        Midnight {
            common: self.stage_common(),
        }
    }

    fn wireguard(&self) -> Wireguard {
        Wireguard {
            common: self.stage_common(),
        }
    }

    fn validator(&self) -> Validator {
        Validator {
            common: self.stage_common(),
            ..self.validator.clone()
        }
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "all")]
impl Planner for All {
    async fn default() -> anyhow::Result<Self> {
        Ok(Self {
            validator: Validator::default().await?,
        })
    }

    async fn plan(&self) -> anyhow::Result<Vec<StatefulAction<Box<dyn Action>>>> {
        let mut actions: Vec<StatefulAction<Box<dyn Action>>> = vec![];

        if self.common().install_base_packages {
            actions.push(crate::planner::cardano::base_packages().await?);
        }

        let stages = [
            self.directories().plan().await?,
            self.cardano().plan().await?,
            self.db_sync().plan().await?,
            self.midnight().plan().await?,
            self.wireguard().plan().await?,
            self.validator().plan().await?,
        ];

        // The stages overlap: several of them create the same directories. An action is
        // the same work as an earlier one only when every detail of it is the same (the
        // path, but also the owner and mode), and then the first one wins. The same synopsis
        // with different details is two stages disagreeing about one thing, which no order
        // of the two would make right, so it is refused rather than resolved by luck.
        let mut seen: HashMap<String, serde_json::Value> = HashMap::new();
        let mut kept = HashSet::new();
        for stage in stages {
            for action in stage {
                let synopsis = action.tracing_synopsis();
                let identity = identity(&action)?;

                match seen.get(&synopsis) {
                    None => {
                        seen.insert(synopsis, identity);
                        actions.push(action);
                    },
                    Some(earlier) if *earlier == identity => {
                        kept.insert(synopsis);
                    },
                    Some(earlier) => anyhow::bail!(
                        "Two stages of the `all` plan disagree about `{synopsis}`: one plans {earlier}, another {identity}. This is a bug in the installer, not in this host."
                    ),
                }
            }
        }
        tracing::debug!(
            "Dropped {} duplicated action(s) from the whole-host plan",
            kept.len()
        );

        Ok(actions)
    }

    fn settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        self.validator.settings()
    }

    async fn configured_settings(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        diff_from_default(self).await
    }

    fn receipt_path(&self) -> PathBuf {
        self.common().paths().receipt(self.typetag_name())
    }

    async fn platform_check(&self) -> anyhow::Result<()> {
        platform_check(self.typetag_name())
    }

    async fn pre_install_check(&self) -> anyhow::Result<()> {
        require_root()?;
        require_user(&self.common().cardano_user)?;
        require_user(&self.common().midnight_user)
    }
}

/// Everything an action is made of, which is what decides whether two are the same one
///
/// The state is left out: whether the machine already had a directory says nothing about
/// whether two stages meant the same directory.
fn identity(action: &StatefulAction<Box<dyn Action>>) -> anyhow::Result<serde_json::Value> {
    // A `StatefulAction` serializes as `{"action": .., "state": ..}`, and it is the action
    // half which says what the step is
    let mut value = serde_json::to_value(action)
        .map_err(|e| anyhow::anyhow!("Describing `{}`: {e}", action.tracing_synopsis()))?;
    Ok(match value.get_mut("action") {
        Some(inner) => inner.take(),
        None => value,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn the_receipt_keeps_the_settings_flat() -> anyhow::Result<()> {
        let planner: Box<dyn Planner> = All::default().await?.boxed();
        let json = serde_json::to_value(&planner)?;

        assert_eq!(json["planner"], "all");
        assert!(json.get("common").is_some(), "{json}");
        assert!(json.get("database_user").is_some(), "{json}");
        assert!(json.get("validator").is_none(), "{json}");

        let back: Box<dyn Planner> = serde_json::from_value(json)?;
        assert_eq!(back.typetag_name(), "all");
        Ok(())
    }
}
