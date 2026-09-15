use std::path::PathBuf;

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};

/** Check, at execute time, that the files a later step needs are in place

In a whole-host plan the paths this names are produced by earlier actions, so it always
passes there. Installing a stage on its own is where it earns its keep: it fails, first and
with the name of the thing that is missing and the step which produces it, rather than
letting a service fail to start minutes later for reasons the operator has to go and read
the journal for. Planning does not check, because the plan for one stage must be the same
whether or not the stage before it has run yet.

It changes nothing, so reverting it does nothing.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "require_paths")]
pub struct RequirePaths {
    label: String,
    /// Each path, and what it is in words
    paths: Vec<(PathBuf, String)>,
    remedy: String,
}

impl RequirePaths {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        label: impl Into<String>,
        paths: Vec<(PathBuf, String)>,
        remedy: impl Into<String>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            label: label.into(),
            paths,
            remedy: remedy.into(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "require_paths")]
impl Action for RequirePaths {
    fn tracing_synopsis(&self) -> String {
        format!("Check that {} is in place", self.label)
    }

    fn tracing_span(&self) -> Span {
        span!(tracing::Level::DEBUG, "require_paths", label = self.label)
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            self.paths
                .iter()
                .map(|(path, what)| format!("`{}` ({what})", path.display()))
                .collect(),
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        for (path, what) in &self.paths {
            if !path.exists() {
                anyhow::bail!(
                    "`{path}` is missing ({what}). {remedy}",
                    path = path.display(),
                    remedy = self.remedy,
                );
            }
        }
        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
