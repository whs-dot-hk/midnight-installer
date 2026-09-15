use anyhow::Context;
use std::path::{Path, PathBuf};

use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};
use crate::settings::{ArchiveSource, ReleaseArchive};

/** Read a `.tar.gz` release, verify its checksum, and unpack it into a scratch directory

The archive is downloaded, or read from where it was staged on this host. Nothing is
unpacked unless its SHA-256 matches the one the plan was made with, so a replaced release, a
tampered download or a truncated staged file is refused before it can be installed from.

The scratch directory belongs to the installer, so revert deletes it outright. The actions
which follow pick the binaries and configuration they need out of it.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "fetch_and_unpack_tarball")]
pub struct FetchAndUnpackTarball {
    archive: ReleaseArchive,
    dest: PathBuf,
}

impl FetchAndUnpackTarball {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        archive: ReleaseArchive,
        dest: impl AsRef<Path>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            archive,
            dest: dest.as_ref().to_path_buf(),
        }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "fetch_and_unpack_tarball")]
impl Action for FetchAndUnpackTarball {
    fn tracing_synopsis(&self) -> String {
        format!("Read, verify and unpack `{}`", self.archive.source)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "fetch_and_unpack_tarball",
            source = tracing::field::display(&self.archive.source),
            dest = tracing::field::display(self.dest.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![
                format!("SHA-256 `{}`", self.archive.sha256),
                format!("Unpacked into `{}`", self.dest.display()),
            ],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let Self { archive, dest } = self;

        // A leftover unpack from an interrupted run would confuse the `find` the next
        // actions do, so start from an empty directory
        crate::util::remove_dir_all_if_exists(dest)
            .await
            .with_context(|| format!("Removing `{}`", dest.display()))?;

        let bytes = match &archive.source {
            ArchiveSource::Url(url) => crate::util::fetch_bytes(url).await?,
            ArchiveSource::Path(path) => tokio::fs::read(path)
                .await
                .with_context(|| format!("Reading the staged archive `{}`", path.display()))?,
        };

        let actual = crate::util::sha256_hex(&bytes);
        if actual != archive.sha256 {
            anyhow::bail!(
                "`{source}` does not match its expected SHA-256: expected {expected}, got {actual}. The release may have been replaced, the download tampered with, or the staged file truncated; nothing was installed from it",
                source = archive.source,
                expected = archive.sha256,
            );
        }

        crate::util::unpack_tar_gz(bytes, dest).await?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Remove the unpacked archive at `{}`", self.dest.display()),
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
