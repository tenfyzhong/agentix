use crate::{Cli, memory};
use agentix_memory::{IpcClient, MemoryLocation};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

#[cfg(unix)]
mod login_environment;

pub async fn serve(cli: &Cli) -> Result<Value> {
    #[cfg(unix)]
    login_environment::reexec().await?;
    let path = cli.config_path()?;
    let location = MemoryLocation::load(&path)?;
    ensure!(
        location.enabled,
        "memory is disabled; set TASKIX_MEMORY_ENABLED=true or 1"
    );
    memory::daemon::serve(&path, location).await
}

pub async fn reload(cli: &Cli) -> Result<Value> {
    let location = MemoryLocation::load(&cli.config_path()?)?;
    ensure!(
        location.enabled,
        "memory is disabled; set TASKIX_MEMORY_ENABLED=true or 1"
    );
    IpcClient::new(&location.path, location.service)?
        .call(json!({"op":"reload"}))
        .await
}
