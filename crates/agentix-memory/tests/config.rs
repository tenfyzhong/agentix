use agentix_memory::{MemoryConfig, MemoryLocation};

#[test]
fn memory_activation_uses_environment_without_serializing_a_toml_switch() {
    const CASE: &str = "TASKIX_MEMORY_CONFIG_TEST_PATH";
    if let Some(path) = std::env::var_os(CASE) {
        let path = std::path::Path::new(&path);
        let enabled = matches!(
            std::env::var("TASKIX_MEMORY_ENABLED").as_deref(),
            Ok("true" | "1")
        );
        assert_eq!(MemoryLocation::load(path).unwrap().enabled, enabled);
        let config = MemoryConfig::load(path).unwrap();
        assert_eq!(config.enabled, enabled);
        assert!(
            serde_json::to_value(config)
                .unwrap()
                .get("enabled")
                .is_none()
        );
        return;
    }
    for value in [
        None,
        Some(""),
        Some("false"),
        Some("0"),
        Some("TRUE"),
        Some("yes"),
        Some(" true "),
        Some("true"),
        Some("1"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("schema_version=1\n[storage]\npath={:?}\n[memory]\n[memory.providers.openai]\nbase_url='http://127.0.0.1:9/v1'\n", dir.path().join("tasks.sqlite3"))).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "memory_activation_uses_environment_without_serializing_a_toml_switch",
                "--nocapture",
            ])
            .env(CASE, &path)
            .env_remove("TASKIX_MEMORY_ENABLED");
        if let Some(value) = value {
            command.env("TASKIX_MEMORY_ENABLED", value);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "value {value:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn retrieval_location_does_not_parse_agent_configuration_or_require_a_vault() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "schema_version=1\n[storage]\npath={:?}\n[memory]\n[memory.agent]\nmodel=42\n",
            dir.path().join("tasks.sqlite3")
        ),
    )
    .unwrap();
    let location = MemoryLocation::load_with_env(&path, |_| Some("true".into())).unwrap();
    assert!(location.enabled);
    assert_eq!(location.path, dir.path().join("memory.sqlite3"));
    assert!(load_enabled_config(&path).is_err());
}

#[test]
fn model_configuration_is_explicit_and_capability_checked_without_downgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let text = format!(
        r#"schema_version=1
[storage]
path={:?}
[memory]
[memory.providers.local]
protocol="openai"
base_url="http://127.0.0.1:8080/v1"
[memory.agent]
provider="local"
api="responses"
model="gpt-6-astra"
max_concurrent_loops=4
[memory.embedding]
enabled=true
provider="local"
model="test-embedding"
dimensions=3
"#,
        dir.path().join("tasks.sqlite3")
    );
    std::fs::write(&path, &text).unwrap();
    let config = load_enabled_config(&path).unwrap();
    assert_eq!(config.agent.model, "gpt-6-astra");
    assert_eq!(config.embedding.dimensions, Some(3));
    std::fs::write(
        &path,
        text.replace("api=\"responses\"", "api=\"chat_completions\""),
    )
    .unwrap();
    assert!(
        load_enabled_config(&path)
            .unwrap_err()
            .to_string()
            .contains("Responses")
    );
    std::fs::write(
        &path,
        text.replace("max_concurrent_loops=4", "max_concurrent_loops=0"),
    )
    .unwrap();
    assert!(load_enabled_config(&path).is_err());
}

#[test]
fn missing_schema_is_a_configuration_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[memory]\n").unwrap();
    assert!(MemoryLocation::load_with_env(&path, |_| Some("true".into())).is_err());
}

#[test]
fn shipped_example_loads_for_service_and_can_enable_embedding_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let example = include_str!("../../../config/taskix.example.toml");
    std::fs::write(&path, example.replace("enabled = false", "enabled = true")).unwrap();
    let config = load_enabled_config(&path).unwrap();
    assert!(config.enabled && config.embedding.enabled);
    assert_eq!(config.agent.model, "gpt-6-astra");
    assert_eq!(config.agent.repository_review_interval_seconds, 86400);
    assert_eq!(config.retrieval.max_context_bytes, 6400);
}

#[test]
fn maintenance_limits_are_validated_and_defaults_are_backward_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let example = include_str!("../../../config/taskix.example.toml")
        .replace("enabled = false", "enabled = true");
    std::fs::write(&path, &example).unwrap();
    let config = load_enabled_config(&path).unwrap();
    assert_eq!(config.agent.extraction_debounce_ms, 1000);
    assert_eq!(config.embedding.max_concurrent_projects, 4);
    assert_eq!(config.projection.reconcile_interval_seconds, 300);
    for (from, to) in [
        (
            "extraction_debounce_ms = 1000",
            "extraction_debounce_ms = 60001",
        ),
        ("max_concurrent_projects = 4", "max_concurrent_projects = 0"),
        (
            "reconcile_interval_seconds = 300",
            "reconcile_interval_seconds = 0",
        ),
        ("lease_seconds = 300", "lease_seconds = 241"),
    ] {
        std::fs::write(&path, example.replace(from, to)).unwrap();
        assert!(
            load_enabled_config(&path).is_err(),
            "invalid setting accepted: {to}"
        );
    }
    let legacy = example
        .replace("extraction_debounce_ms = 1000", "")
        .replace("max_concurrent_projects = 4", "")
        .replace("reconcile_interval_seconds = 300", "");
    std::fs::write(&path, legacy).unwrap();
    assert_eq!(
        load_enabled_config(&path)
            .unwrap()
            .agent
            .extraction_debounce_ms,
        1000
    );
    std::fs::write(
        &path,
        example.replace(
            "extraction_debounce_ms = 1000",
            "extraction_debounce_ms = 0",
        ),
    )
    .unwrap();
    assert_eq!(
        load_enabled_config(&path)
            .unwrap()
            .agent
            .extraction_debounce_ms,
        0
    );
}

fn load_enabled_config(path: &std::path::Path) -> anyhow::Result<MemoryConfig> {
    MemoryConfig::load_with_env(path, |_| Some("true".into()))
}
