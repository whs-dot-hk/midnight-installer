/*! Running commands on, and copying files to, a devnet host

Either through `gcloud compute ssh` / `gcloud compute scp` (optionally through an IAP tunnel,
for instances with no public address) or through plain `ssh` / `scp`. Both run the command
through the remote user's shell, so everything sent is quoted with [`sh_quote`].
*/

use std::path::Path;
use std::process::{Output, Stdio};

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::devnet::config::{Host, Ssh};

/// One host, and how to reach it
#[derive(Debug, Clone)]
pub struct Remote {
    pub host: Host,
    ssh: Ssh,
}

impl Remote {
    pub fn new(host: Host, ssh: Ssh) -> Self {
        Self { host, ssh }
    }

    pub fn name(&self) -> &str {
        &self.host.name
    }

    /// The command which runs `script` on the host
    fn command(&self, script: &str) -> Command {
        match &self.ssh {
            Ssh::Gcloud { project, zone, iap } => {
                let mut command = Command::new("gcloud");
                command
                    .args(["compute", "ssh", &self.host.name])
                    .arg(format!(
                        "--zone={}",
                        self.host.zone.as_deref().unwrap_or(zone)
                    ))
                    .arg(format!("--project={project}"))
                    .arg("--quiet");
                if *iap {
                    command.arg("--tunnel-through-iap");
                }
                command.arg(format!("--command={script}"));
                command
            },
            Ssh::Ssh { options } => {
                let mut command = Command::new("ssh");
                command
                    .args(options)
                    .args(["-o", "BatchMode=yes"])
                    .arg(&self.host.name)
                    .arg(script);
                command
            },
        }
    }

    /// Run `script` on the host and collect its output; a failure is an error
    pub async fn run(&self, script: &str) -> anyhow::Result<Output> {
        let output = self
            .command(script)
            .stdin(Stdio::null())
            .output()
            .await
            .with_context(|| self.unreachable())?;
        if !output.status.success() {
            anyhow::bail!(
                "`{script}` failed on `{host}` ({status}):\n{stderr}",
                host = self.host.name,
                status = output.status,
                stderr = without_noise(&String::from_utf8_lossy(&output.stderr)),
            );
        }
        Ok(output)
    }

    /// Run `script` on the host and return its standard output
    pub async fn run_stdout(&self, script: &str) -> anyhow::Result<String> {
        let output = self.run(script).await?;
        String::from_utf8(output.stdout).context("The output was not UTF-8")
    }

    /// Run `script` on the host, showing its output line by line, prefixed with the host
    pub async fn run_shown(&self, script: &str) -> anyhow::Result<()> {
        let mut child = self
            .command(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| self.unreachable())?;

        let prefix = format!("[{}]", self.host.name);
        let stdout = child.stdout.take().context("No stdout")?;
        let stderr = child.stderr.take().context("No stderr")?;
        tokio::join!(show_lines(stdout, &prefix), show_lines(stderr, &prefix));

        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("`{script}` failed on `{}` ({status})", self.host.name);
        }
        Ok(())
    }

    /// Copy a local file to `remote_path` on the host, as the SSH user
    pub async fn upload(&self, local: &Path, remote_path: &str) -> anyhow::Result<()> {
        let mut command = match &self.ssh {
            Ssh::Gcloud { project, zone, iap } => {
                let mut command = Command::new("gcloud");
                command
                    .args(["compute", "scp"])
                    .arg(format!(
                        "--zone={}",
                        self.host.zone.as_deref().unwrap_or(zone)
                    ))
                    .arg(format!("--project={project}"))
                    .arg("--quiet");
                if *iap {
                    command.arg("--tunnel-through-iap");
                }
                command
            },
            Ssh::Ssh { options } => {
                let mut command = Command::new("scp");
                command
                    .args(scp_options(options))
                    .args(["-o", "BatchMode=yes"]);
                command
            },
        };
        command
            .arg(local)
            .arg(format!("{}:{remote_path}", self.host.name))
            .stdin(Stdio::null());

        let output = command.output().await.with_context(|| self.unreachable())?;
        if !output.status.success() {
            anyhow::bail!(
                "Copying `{}` to `{}:{remote_path}` failed:\n{}",
                local.display(),
                self.host.name,
                without_noise(&String::from_utf8_lossy(&output.stderr)),
            );
        }
        Ok(())
    }

    fn unreachable(&self) -> String {
        match &self.ssh {
            Ssh::Gcloud { .. } => format!(
                "Running `gcloud` to reach `{}`: is the Google Cloud SDK installed and logged in?",
                self.host.name
            ),
            Ssh::Ssh { .. } => format!("Running `ssh` to reach `{}`", self.host.name),
        }
    }
}

/// Print each line of `stream` behind `prefix`, leaving out `gcloud`'s connection banner
async fn show_lines(stream: impl tokio::io::AsyncRead + Unpin, prefix: &str) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !is_noise(&line) && !line.trim().is_empty() {
            println!("{prefix} {line}");
        }
    }
}

/// `scp` spells the port option `-P`, where `ssh` spells it `-p`
fn scp_options(options: &[String]) -> Vec<String> {
    options
        .iter()
        .map(|option| {
            if option == "-p" {
                String::from("-P")
            } else {
                option.clone()
            }
        })
        .collect()
}

/// Quote `value` as one word for a POSIX shell
pub fn sh_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// What `gcloud` prints on every IAP connection, which says nothing about the command
fn is_noise(line: &str) -> bool {
    let line = line.trim();
    line == "WARNING:"
        || line.contains("consider installing NumPy")
        || line.contains("increasing_the_tcp_upload_bandwidth")
        || line.starts_with("Warning: Permanently added")
        || line.starts_with("Existing host keys found")
}

fn without_noise(text: &str) -> String {
    text.lines()
        .filter(|line| !is_noise(line) && !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn quotes_what_the_shell_would_split() {
        assert_eq!(
            sh_quote("/var/lib/midnight-devnet"),
            "/var/lib/midnight-devnet"
        );
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("$HOME"), "'$HOME'");
    }

    #[test]
    fn drops_the_iap_banner() {
        let stderr = "WARNING:\n\nTo increase the performance of the tunnel, consider installing NumPy. For instructions,\nplease see https://cloud.google.com/iap/docs/using-tcp-forwarding#increasing_the_tcp_upload_bandwidth\n\nsudo: a password is required\n";
        assert_eq!(without_noise(stderr), "sudo: a password is required");
    }

    #[test]
    fn scp_takes_the_port_as_capital_p() {
        assert_eq!(
            scp_options(&["-p".into(), "2222".into(), "-i".into(), "k".into()]),
            vec!["-P", "2222", "-i", "k"]
        );
    }

    #[test]
    fn gcloud_goes_through_iap_in_the_hosts_zone() {
        let remote = Remote::new(
            Host {
                name: "vm".into(),
                zone: Some("europe-west1-c".into()),
                address: None,
                ports: None,
                validators: vec![],
                full_nodes: vec![],
            },
            Ssh::Gcloud {
                project: "p".into(),
                zone: "europe-west1-b".into(),
                iap: true,
            },
        );
        let command = remote.command("true");
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(command.as_std().get_program(), "gcloud");
        assert!(args.contains(&"--zone=europe-west1-c".into()), "{args:?}");
        assert!(args.contains(&"--tunnel-through-iap".into()), "{args:?}");
        assert!(args.contains(&"--command=true".into()), "{args:?}");
    }
}
