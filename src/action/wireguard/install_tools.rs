use anyhow::Context;

use tracing::{span, Span};

use crate::action::base::AptInstall;
use crate::action::{Action, ActionDescription, ActionState, StatefulAction};

const WIREGUARD_TOOLS_REPOSITORY: &str = "https://git.zx2c4.com/wireguard-tools";

/** Build and install `wireguard-tools` at the version the FNO programme pins

The distribution's package is a different version, and the Foundation's tunnel configuration
is written against this one, so the tools are built from the tagged source.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "install_wireguard_tools")]
pub struct InstallWireguardTools {
    version: String,
    build_dependencies: StatefulAction<AptInstall>,
}

impl InstallWireguardTools {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(version: impl Into<String>) -> anyhow::Result<StatefulAction<Self>> {
        let version = version.into();
        let required = version.trim_start_matches('v').to_string();

        let build_dependencies =
            AptInstall::plan(["git", "build-essential", "pkg-config", "libelf-dev"]).await?;

        let state = match installed_version().await {
            Some(installed) if installed == required => {
                tracing::debug!("wireguard-tools {version} is already installed");
                ActionState::Skipped
            },
            Some(installed) => {
                tracing::warn!(
                    "wireguard-tools {installed} is installed, but the FNO programme requires {required}"
                );
                ActionState::Uncompleted
            },
            None => ActionState::Uncompleted,
        };

        Ok(StatefulAction {
            action: Self {
                version,
                build_dependencies,
            },
            state,
        })
    }
}

/// The version `wg` reports, without its leading `v`
pub async fn installed_version() -> Option<String> {
    let output = crate::command("wg").arg("--version").output().await.ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(1)
        .map(|version| version.trim_start_matches('v').to_string())
}

#[async_trait::async_trait]
#[typetag::serde(name = "install_wireguard_tools")]
impl Action for InstallWireguardTools {
    fn tracing_synopsis(&self) -> String {
        format!("Build and install wireguard-tools {}", self.version)
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "install_wireguard_tools",
            version = self.version,
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        let mut explanation: Vec<String> = self
            .build_dependencies
            .describe_execute()
            .into_iter()
            .map(|description| description.description)
            .collect();
        explanation.push(format!(
            "Clone {WIREGUARD_TOOLS_REPOSITORY} at `{}` and `make install`",
            self.version
        ));

        vec![ActionDescription::new(self.tracing_synopsis(), explanation)]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        self.build_dependencies.try_execute().await?;

        // The kernel module is in-tree on the kernels FNO hosts run; the headers are only
        // needed for out of tree builds, so a missing headers package is not fatal
        let kernel = crate::execute_command_stdout(crate::command("uname").arg("-r")).await?;
        let headers = format!("linux-headers-{}", kernel.trim());
        if crate::command_succeeds(crate::command("apt-cache").arg("show").arg(&headers)).await {
            if let Err(e) = crate::execute_command(
                crate::command("apt-get")
                    .env("DEBIAN_FRONTEND", "noninteractive")
                    .arg("install")
                    .arg("-y")
                    .arg(&headers),
            )
            .await
            {
                tracing::warn!("Could not install `{headers}`: {e}");
            }
        } else {
            tracing::warn!("`{headers}` is not available from APT, continuing without it");
        }

        let workdir = tempdir().await?;
        let checkout = workdir.join("wireguard-tools");

        crate::execute_command(
            crate::command("git")
                .arg("clone")
                .arg(WIREGUARD_TOOLS_REPOSITORY)
                .arg(&checkout),
        )
        .await?;
        crate::execute_command(
            crate::command("git")
                .arg("-C")
                .arg(&checkout)
                .arg("checkout")
                .arg(&self.version),
        )
        .await?;

        let src = checkout.join("src");
        crate::execute_command(crate::command("make").arg("-C").arg(&src)).await?;
        crate::execute_command(crate::command("make").arg("-C").arg(&src).arg("install")).await?;

        crate::util::remove_dir_all_if_exists(&workdir)
            .await
            .with_context(|| format!("Removing `{}`", workdir.display()))?;

        let required = self.version.trim_start_matches('v');
        match installed_version().await {
            Some(installed) if installed == required => {
                tracing::info!("wireguard-tools {installed} installed");
                Ok(())
            },
            Some(installed) => Err(anyhow::anyhow!(
                "wireguard-tools {installed} was installed, but {required} was required"
            )),
            None => Err(anyhow::anyhow!("`wg` was not installed")),
        }
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!("Leave wireguard-tools {} installed", self.version),
            vec![String::from(
                "The tunnel may already be carrying traffic this host depends on",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> anyhow::Result<()> {
        tracing::info!("Leaving wireguard-tools {} installed", self.version);
        Ok(())
    }
}

async fn tempdir() -> anyhow::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "midnight-installer-wireguard-{}",
        std::process::id()
    ));
    tokio::fs::create_dir_all(&path)
        .await
        .with_context(|| format!("Creating directory `{}`", path.display()))?;
    Ok(path)
}
