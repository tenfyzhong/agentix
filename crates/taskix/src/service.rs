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
        let metrics = crate::telemetry::load_config(&path)?;
        ensure!(
            location.enabled || metrics.enabled,
            "memory is disabled; set TASKIX_MEMORY_ENABLED=true or 1, or enable metrics"
        );
        let _metrics = crate::telemetry::start(&metrics, &location).await?;
        if !location.enabled {
            memory::daemon::shutdown_signal().await?;
            return Ok(json!({"stopped":true}));
        }
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

/// Inspect both independently enabled service capabilities without starting them.
pub async fn diagnostics(cli: &Cli, doctor: bool) -> Result<Value> {
    let path = cli.config_path()?;
    let action = if doctor {
        crate::memory_command::MemoryCommand::Doctor
    } else {
        crate::memory_command::MemoryCommand::Status
    };
    let mut result = memory::run(cli, &action).await?;
    let metrics = metrics_status(&path).await;
    result["memory_enabled"] = json!(MemoryLocation::load(&path)?.enabled);
    result["memory_online"] = result["online"].clone();
    result["enabled"] = json!(result["memory_enabled"] == true || metrics["enabled"] == true);
    result["online"] = json!(result["memory_online"] == true || metrics["online"] == true);
    result["metrics"] = metrics;
    if doctor {
        let config = agentix_task::Config::load(&path)?;
        if config.storage.path.exists() {
            let store = agentix_task::Store::open_read_only(&config.storage.path).await?;
            let service = agentix_task::Service::new(config, store)?;
            let checks = crate::doctor(&service).await?;
            result
                .as_object_mut()
                .unwrap()
                .extend(checks["result"].as_object().unwrap().clone());
        } else {
            result["healthy"] = json!(false);
            result["task_database_exists"] = json!(false);
        }
    }
    Ok(result)
}

async fn metrics_status(path: &std::path::Path) -> Value {
    let Ok(config) = crate::telemetry::load_config(path) else {
        return json!({"configuration_valid":false,"online":false,
            "error":"invalid metrics configuration; check enabled and loopback listen address"});
    };
    let endpoint = format!("http://{}/metrics", config.listen);
    let online = if config.enabled {
        probe_metrics(&endpoint).await.unwrap_or(false)
    } else {
        false
    };
    json!({"enabled":config.enabled,"configuration_valid":true,"listen":config.listen,
        "endpoint":endpoint,"online":online})
}

async fn probe_metrics(endpoint: &str) -> Result<bool> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let mut response = client.get(endpoint).send().await?.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= 1024 * 1024,
            "metrics response too large"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(std::str::from_utf8(&body)?
        .lines()
        .any(|line| line.starts_with("taskix_build_info{")))
}
