use crate::{Cli, memory};
use agentix_memory::{IpcClient, MemoryLocation};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub(crate) mod logging;
#[cfg(unix)]
mod login_environment;

pub async fn serve(cli: &Cli) -> Result<Value> {
    #[cfg(unix)]
    login_environment::reexec().await?;
    let path = cli.config_path()?;
    let config = logging::LoggingConfig::load(&path)?;
    let _log_guard = logging::init(&config)?;
    let result = async {
        let location = MemoryLocation::load(&path)?;
        ensure!(
            location.enabled,
            "memory is disabled; set TASKIX_MEMORY_ENABLED=true or 1"
        );
        memory::daemon::serve(&path, location, config).await
    }
    .await;
    match &result {
        Ok(_) => tracing::info!("taskix service stopped"),
        Err(error) => tracing::error!(error = %format!("{error:#}"), "taskix service failed"),
    }
    result
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
