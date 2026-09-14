use anyhow::Context;
use std::path::{Path, PathBuf};

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};
use crate::util::{chown_recursive, resolve_owner};

/** Copy a directory out of an unpacked release archive into place

Like [`InstallBinary`](super::InstallBinary), the directory is located inside the archive at
execute time: `path_suffix` is matched against the end of the unpacked path (`share/preprod`,
`res`, `schema`).
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_tree")]
pub struct InstallTree {
    search_root: PathBuf,
    path_suffix: String,
    dest: PathBuf,
    user: Option<String>,
    group: Option<String>,
}

impl InstallTree {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        search_root: impl AsRef<Path>,
        path_suffix: impl AsRef<str>,
        dest: impl AsRef<Path>,
        user: impl Into<Option<String>>,
        group: impl Into<Option<String>>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            search_root: search_root.as_ref().to_path_buf(),
            path_suffix: path_suffix.as_ref().to_string(),
            dest: dest.as_ref().to_path_buf(),
            user: user.into(),
            group: group.into(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_tree")]
impl Action for InstallTree {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Install `{suffix}` from the release archive to `{dest}`",
            suffix = self.path_suffix,
            dest = self.dest.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_tree",
            path_suffix = self.path_suffix,
            dest = tracing::field::display(self.dest.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![format!(
                "Replaces `{}` if it exists, owned by `{}`",
                self.dest.display(),
                self.user.as_deref().unwrap_or("root"),
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self {
            search_root,
            path_suffix,
            dest,
            user,
            group,
        } = self;

        let source = crate::util::find_dir(search_root, path_suffix)
            .with_context(|| format!("The release archive did not contain `{path_suffix}`"))?;

        crate::util::remove_dir_all_if_exists(dest)
            .await
            .with_context(|| format!("Removing `{}`", dest.display()))?;
        tokio::fs::create_dir_all(&*dest)
            .await
            .with_context(|| format!("Creating directory `{}`", dest.display()))?;

        crate::util::copy_dir_all(&source, dest).await?;

        let (uid, gid) = resolve_owner(user.as_deref(), group.as_deref())?;
        chown_recursive(dest, uid, gid)?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Remove the directory `{}`", self.dest.display()),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        crate::util::remove_dir_all_if_exists(&self.dest)
            .await
            .with_context(|| format!("Removing `{}`", self.dest.display()))?;
        Ok(())
    }
}
