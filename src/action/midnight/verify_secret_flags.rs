use std::path::PathBuf;

use anyhow::Context;
use tracing::{span, Span};

use crate::action::{Action, ActionDescription, StatefulAction};

/** Check, after the node is running, that the secret-root flags reached the process

`--node-key-file` and `--keystore-path` are written into the unit because the key material
they name lives under the secret root rather than under `--base-path`. Whether they take
effect is a separate question: a systemd **drop-in** cannot amend `ExecStart`, only reset
it and restate the whole command, so any drop-in carries a full copy of whatever the unit
said when it was written. Add an argument to the unit afterwards and the drop-in's copy
silently wins without it.

That is not hypothetical — it is how a preprod host ran for three days: a drop-in added to
turn telemetry off held a pre-secret-root copy of the command, the node fell back to the
`--base-path` defaults, and re-created a keystore holding real private keys outside the
root that is meant to hold all of them. `systemctl cat` shows both `ExecStart` lines and
looks entirely reasonable; nothing warns you.

So this reads the one thing that cannot be argued with, `/proc/<pid>/cmdline`, and fails
the stage if either flag is missing. Failing is the point: reporting success over a node
writing private keys to the wrong place is worse than stopping.

It changes nothing, so reverting it does nothing.
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "action_name", rename = "verify_secret_flags")]
pub struct VerifySecretFlags {
    unit: String,
    node_key_file: PathBuf,
    keystore_path: PathBuf,
    base_path: PathBuf,
    secret_root: PathBuf,
}

impl VerifySecretFlags {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        unit: impl AsRef<str>,
        node_key_file: impl Into<PathBuf>,
        keystore_path: impl Into<PathBuf>,
        base_path: impl Into<PathBuf>,
        secret_root: impl Into<PathBuf>,
    ) -> anyhow::Result<StatefulAction<Self>> {
        Ok(StatefulAction::uncompleted(Self {
            unit: unit.as_ref().to_string(),
            node_key_file: node_key_file.into(),
            keystore_path: keystore_path.into(),
            base_path: base_path.into(),
            secret_root: secret_root.into(),
        }))
    }

    /// The flags as they appear on a command line, for matching and for reporting
    fn expected(&self) -> [String; 2] {
        [
            format!("--node-key-file {}", self.node_key_file.display()),
            format!("--keystore-path {}", self.keystore_path.display()),
        ]
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "verify_secret_flags")]
impl Action for VerifySecretFlags {
    fn tracing_synopsis(&self) -> String {
        format!(
            "Check that `{unit}` really runs with its `{root}` flags",
            unit = self.unit,
            root = self.secret_root.display(),
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "verify_secret_flags",
            unit = self.unit,
            secret_root = tracing::field::display(self.secret_root.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![format!(
                "Read the running command line and confirm `--node-key-file` and `--keystore-path` are on it, so that no drop-in has reset `ExecStart` back to a copy without them",
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> anyhow::Result<()> {
        let cmdline = running_cmdline(&self.unit).await?;

        let missing: Vec<String> = self
            .expected()
            .into_iter()
            .filter(|flag| !cmdline.contains(flag.as_str()))
            .collect();

        if !missing.is_empty() {
            anyhow::bail!(
                "`{unit}` is running without {list}.\n\
                 Something is overriding `ExecStart` — inspect it with `systemctl cat {unit}`. \
                 A drop-in that resets `ExecStart=` restates the whole command, so it holds a \
                 stale copy which drops whatever was added to the unit. Restate the flags there, \
                 run `systemctl daemon-reload`, and run this step again.\n\
                 Until then the node falls back to `{base}` and writes private keys outside `{root}`.",
                unit = self.unit,
                list = missing
                    .iter()
                    .map(|flag| format!("`{flag}`"))
                    .collect::<Vec<_>>()
                    .join(" or "),
                base = self.base_path.display(),
                root = self.secret_root.display(),
            );
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

/// The command line of the unit's main process, arguments separated by single spaces
async fn running_cmdline(unit: &str) -> anyhow::Result<String> {
    let pid = crate::check::unit_main_pid(unit)
        .await
        .with_context(|| format!("Asking systemd for the main PID of `{unit}`"))?;

    let raw = tokio::fs::read(format!("/proc/{pid}/cmdline"))
        .await
        .with_context(|| format!("Reading the command line of `{unit}` (pid {pid})"))?;

    Ok(String::from_utf8_lossy(&raw)
        .split('\0')
        .filter(|argument| !argument.is_empty())
        .collect::<Vec<_>>()
        .join(" "))
}
