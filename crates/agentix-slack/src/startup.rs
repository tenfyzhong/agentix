//! Slack CLI owns authorization, refresh and installation. No management tokens
//! enter Agentix configuration, command arguments, or logs.
use crate::manifest::{commands_match, merge_for_owner};
use agentix_domain::{ChannelCommand, ChannelError};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Clone)]
pub struct SlackCommandSync {
    executable: PathBuf,
    app_id: String,
    commands: Vec<ChannelCommand>,
    timeout: Duration,
}

impl SlackCommandSync {
    #[must_use]
    pub fn new(executable: PathBuf, app_id: String, commands: Vec<ChannelCommand>) -> Self {
        Self {
            executable,
            app_id,
            commands,
            timeout: Duration::from_mins(1),
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Synchronize the normal command menu. Returns whether changed.
    pub async fn sync(&self, team: &str) -> Result<bool, ChannelError> {
        self.sync_for_owner(team, true).await
    }

    pub(crate) async fn sync_for_owner(
        &self,
        team: &str,
        has_owner: bool,
    ) -> Result<bool, ChannelError> {
        tokio::time::timeout(self.timeout, self.sync_inner(team, has_owner))
            .await
            .map_err(|_| {
                failure("Slack CLI command sync timed out; check CLI login and network access")
            })?
    }

    async fn sync_inner(&self, team: &str, has_owner: bool) -> Result<bool, ChannelError> {
        if !valid_id(&self.app_id, 'A') || !valid_id(team, 'T') {
            return Err(failure(
                "Slack command sync requires an app ID and workspace team ID",
            ));
        }
        let directory = tempfile::Builder::new()
            .prefix("agentix-slack-")
            .tempdir()
            .map_err(io_error)?;
        let project = directory.path();
        prepare_project(project)?;
        self.run(project, team, &["app", "link", "--environment", "deployed"])
            .await?;
        let remote = self.fetch(project, team).await?;
        let merged = merge_for_owner(&remote, &self.commands, has_owner)?;
        if commands_match(&remote, &merged) {
            return Ok(false);
        }
        // Catch changes made while preparing the update. Slack offers no atomic
        // manifest compare-and-swap, so concurrent administrators must coordinate.
        if self.fetch(project, team).await? != remote {
            return Err(failure(
                "Slack manifest changed during synchronization; retry on the next startup",
            ));
        }
        write_json(&project.join("manifest.json"), &merged)?;
        self.run(project, team, &["app", "install", "--force"])
            .await?;
        let actual = self.fetch(project, team).await?;
        if !commands_match(
            &actual,
            &merge_for_owner(&actual, &self.commands, has_owner)?,
        ) {
            return Err(failure(
                "Slack CLI completed but command synchronization could not be verified",
            ));
        }
        Ok(true)
    }

    async fn fetch(&self, project: &Path, team: &str) -> Result<Value, ChannelError> {
        let output = self
            .run(project, team, &["manifest", "info", "--source", "remote"])
            .await?;
        serde_json::from_slice(&output).map_err(|_| {
            failure("Slack CLI returned an invalid manifest; check the installed CLI version")
        })
    }

    async fn run(
        &self,
        project: &Path,
        team: &str,
        args: &[&str],
    ) -> Result<Vec<u8>, ChannelError> {
        let mut child = Command::new(&self.executable)
            .args(args).args(["--app", &self.app_id, "--team", team, "--no-color", "--skip-update"])
            .current_dir(project).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .kill_on_drop(true).spawn()
            .map_err(|_| failure("cannot execute Slack CLI; install it on PATH or set global slack_cli_path to its absolute path"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| failure("Slack CLI stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| failure("Slack CLI stderr unavailable"))?;
        let (output, errors) = tokio::try_join!(read_output(stdout), read_output(stderr))?;
        let status = child.wait().await.map_err(io_error)?;
        if !status.success() {
            // Only emit fixed hints for recognized codes, never raw CLI output.
            let hint = cli_error_hint(&output, &errors);
            return Err(failure(&format!(
                "Slack CLI {} {} failed ({status}); {hint}",
                args[0], args[1]
            )));
        }
        Ok(output)
    }
}

async fn read_output(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, ChannelError> {
    const LIMIT: u64 = 4 * 1024 * 1024;
    let mut output = Vec::new();
    reader
        .take(LIMIT + 1)
        .read_to_end(&mut output)
        .await
        .map_err(io_error)?;
    if output.len() as u64 > LIMIT {
        return Err(failure("Slack CLI output exceeded the manifest size limit"));
    }
    Ok(output)
}

fn cli_error_hint(stdout: &[u8], stderr: &[u8]) -> &'static str {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    for (code, hint) in [
        (
            "app_not_found",
            "app_not_found: verify channel.slack.app_id belongs to the same app as bot_token and app_token, and that the Slack CLI user can manage it; restart Agentix after correcting configuration",
        ),
        (
            "desc_too_long",
            "desc_too_long: shorten the app description in Slack Basic Information, then restart Agentix",
        ),
        (
            "requires_commands_bot_scope",
            "requires_commands_bot_scope: add the commands bot scope and reinstall the app",
        ),
        (
            "invalid_manifest",
            "invalid_manifest: check the app manifest in Slack App Settings and Slack CLI diagnostic logs",
        ),
    ] {
        if stdout.contains(code) || stderr.contains(code) {
            return hint;
        }
    }
    "run slack auth list/login as the Agentix service user; check app collaborator permissions and Slack CLI diagnostic logs"
}

fn valid_id(id: &str, prefix: char) -> bool {
    id.starts_with(prefix) && id.len() > 1 && id.bytes().all(|c| c.is_ascii_alphanumeric())
}
fn write_json(path: &Path, value: &Value) -> Result<(), ChannelError> {
    std::fs::write(path, value.to_string()).map_err(io_error)
}
#[allow(clippy::needless_pass_by_value)] // Used directly as a map_err callback.
fn io_error(error: std::io::Error) -> ChannelError {
    failure(&format!("Slack command sync I/O failed: {}", error.kind()))
}
fn failure(message: &str) -> ChannelError {
    ChannelError::Transport(message.into())
}

fn prepare_project(project: &Path) -> Result<(), ChannelError> {
    std::fs::create_dir(project.join(".slack")).map_err(io_error)?;
    // A minimal CLI project avoids touching any user project. The fixed hook
    // reads only our generated JSON; no user values are interpolated in it.
    let (hook, script, body) = if cfg!(windows) {
        (
            "cmd /D /C .slack\\manifest.cmd",
            "manifest.cmd",
            "@type manifest.json\r\n",
        )
    } else {
        (
            "/bin/sh .slack/manifest.sh",
            "manifest.sh",
            "#!/bin/sh\ncommand cat manifest.json\n",
        )
    };
    std::fs::write(project.join(".slack").join(script), body).map_err(io_error)?;
    write_json(
        &project.join(".slack/hooks.json"),
        &json!({"hooks":{"get-manifest":hook}}),
    )?;
    write_json(
        &project.join(".slack/config.json"),
        &json!({"manifest":{"source":"local"}}),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires installed Slack CLI; no network or credentials used"]
    async fn installed_cli_reads_generated_manifest_hook() {
        let directory = tempfile::tempdir().unwrap();
        prepare_project(directory.path()).unwrap();
        let manifest = json!({"display_information":{"name":"Agentix test"},"settings":{"socket_mode_enabled":true}});
        write_json(&directory.path().join("manifest.json"), &manifest).unwrap();
        let output = Command::new("slack")
            .args([
                "manifest",
                "info",
                "--source",
                "local",
                "--no-color",
                "--skip-update",
            ])
            .current_dir(directory.path())
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            actual["display_information"],
            manifest["display_information"]
        );
        assert_eq!(actual["settings"]["socket_mode_enabled"], true);
    }
}
