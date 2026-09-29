use agentix_task::Config;
use tempfile::TempDir;

fn config_file(dir: &TempDir, legacy: &str) -> std::path::PathBuf {
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "schema_version = 1\n[storage]\npath = {:?}\n[documents]\n{legacy}root = {:?}\ndirectory = 'Tasks'\n",
            dir.path().join("tasks.sqlite3").to_str().unwrap(), dir.path().to_str().unwrap(),
        ),
    )
    .unwrap();
    path
}

#[test]
fn obsidian_config_loads_without_format_and_omits_it_from_serialization() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let config = Config::load(&config_file(&dir, "")).unwrap();
    let toml = toml::to_string(&config).unwrap();
    assert!(!toml.contains("format"));
    let json = serde_json::to_value(&config).unwrap();
    assert!(json["documents"].get("format").is_none());
    assert_eq!(config.output_dir(), dir.path().join("Tasks"));
}

#[test]
fn obsidian_config_requires_a_vault_even_with_legacy_markdown_format() {
    for legacy in ["", "format = 'markdown'\n"] {
        let dir = TempDir::new().unwrap();
        let error = Config::load(&config_file(&dir, legacy)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Obsidian root must contain .obsidian"),
            "{error:#}"
        );
    }
}

#[test]
fn obsidian_config_discards_legacy_format_and_language() {
    for format in ["obsidian", "markdown"] {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
        let config = Config::load(&config_file(
            &dir,
            &format!("format = '{format}'\nlanguage = 'en'\n"),
        ))
        .unwrap();
        let documents = serde_json::to_value(config.documents).unwrap();
        assert!(documents.get("format").is_none());
        assert!(documents.get("language").is_none());
    }
}

#[test]
fn task_config_does_not_validate_memory_capabilities() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let path = config_file(&dir, "");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(
        "\n[memory]\nenabled = true\n[memory.agent]\nmodel = 42\nprovider = 'not-configured'\n",
    );
    std::fs::write(&path, text).unwrap();
    assert!(
        Config::load(&path).is_ok(),
        "memory validation belongs to its capability"
    );
}

#[test]
fn source_storage_configuration_is_independent_of_documents_and_models() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "schema_version=1\n[storage]\npath={:?}\n[memory.agent]\nmodel=42\n",
            dir.path().join("tasks.sqlite3")
        ),
    )
    .unwrap();
    let storage = agentix_task::StorageConfig::load(&path).unwrap();
    assert_eq!(storage.path, dir.path().join("tasks.sqlite3"));
    std::fs::write(
        &path,
        "schema_version=1\n[storage]\npath='relative.sqlite3'\n",
    )
    .unwrap();
    assert!(agentix_task::StorageConfig::load(&path).is_err());
}
