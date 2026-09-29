use agentix_memory::{MemoryConfig, MemoryLocation};

#[test]
fn retrieval_location_does_not_parse_agent_configuration_or_require_a_vault() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path,format!("schema_version=1\n[storage]\npath={:?}\n[memory]\nenabled=true\n[memory.agent]\nmodel=42\n",dir.path().join("tasks.sqlite3"))).unwrap();
    let location = MemoryLocation::load(&path).unwrap();
    assert!(location.enabled);
    assert_eq!(location.path, dir.path().join("memory.sqlite3"));
    assert!(MemoryConfig::load(&path).is_err());
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
enabled=true
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
    let config = MemoryConfig::load(&path).unwrap();
    assert_eq!(config.agent.model, "gpt-6-astra");
    assert_eq!(config.embedding.dimensions, Some(3));
    std::fs::write(
        &path,
        text.replace("api=\"responses\"", "api=\"chat_completions\""),
    )
    .unwrap();
    assert!(
        MemoryConfig::load(&path)
            .unwrap_err()
            .to_string()
            .contains("Responses")
    );
    std::fs::write(
        &path,
        text.replace("max_concurrent_loops=4", "max_concurrent_loops=0"),
    )
    .unwrap();
    assert!(MemoryConfig::load(&path).is_err());
}

#[test]
fn missing_schema_is_a_configuration_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[memory]\nenabled=true\n").unwrap();
    assert!(MemoryLocation::load(&path).is_err());
}

#[test]
fn shipped_example_loads_for_service_and_can_enable_embedding_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let example = include_str!("../../../config/taskix.example.toml");
    std::fs::write(&path, example.replace("enabled = false", "enabled = true")).unwrap();
    let config = MemoryConfig::load(&path).unwrap();
    assert!(config.enabled && config.embedding.enabled);
    assert_eq!(config.agent.model, "gpt-6-astra");
    assert_eq!(config.agent.repository_review_interval_seconds, 86400);
    assert_eq!(config.retrieval.max_context_bytes, 6400);
}
