use std::{fs, path::Path, process::Command};

use agentix_task::{Config, DocumentConfig, DocumentFormat, StorageConfig};
use serde_json::{Value, json};

#[path = "support/obsidian_cli.rs"]
mod obsidian_cli;

struct Fixture {
    dir: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let f = Self { dir };
        fs::create_dir_all(f.path("vault/.obsidian")).unwrap();
        fs::create_dir_all(f.path("bundle")).unwrap();
        f.write(
            "bundle/manifest.json",
            &json!({"id":"tasknotes","version":"4.12.5","minAppVersion":"1.10.0"}),
        );
        fs::write(f.path("bundle/main.js"), "// fixture plugin").unwrap();
        fs::write(f.path("bundle/styles.css"), "/* fixture styles */").unwrap();
        let config = Config {
            schema_version: 1,
            storage: StorageConfig {
                path: f.path("tasks.sqlite3"),
            },
            documents: DocumentConfig {
                format: DocumentFormat::Obsidian,
                root: f.path("vault"),
                directory: "Tasks".into(),
            },
        };
        fs::write(f.path("config.toml"), toml::to_string(&config).unwrap()).unwrap();
        f
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
    fn write(&self, name: &str, value: &Value) {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    fn read(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.path(name)).unwrap()).unwrap()
    }
    fn run(&self, bundle: bool, success: bool) -> Value {
        self.run_with(bundle, success, &[], None)
    }
    fn run_with(
        &self,
        bundle: bool,
        success: bool,
        args: &[&str],
        scenario: Option<&str>,
    ) -> Value {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskcli"));
        cmd.arg("--config")
            .arg(self.path("config.toml"))
            .args(["--json", "obsidian", "setup"]);
        if bundle {
            cmd.arg("--plugin-dir").arg(self.path("bundle"));
        }
        let bin = self.path("bin");
        fs::create_dir_all(&bin).unwrap();
        if let Some(scenario) = scenario {
            obsidian_cli::install(&bin);
            cmd.env("OBSIDIAN_TEST_SCENARIO", scenario);
        }
        // Never contact the developer's running Obsidian instance.
        cmd.env("PATH", &bin).args(args);
        let output = cmd.output().unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn installs_configures_preserves_settings_and_repeats_without_download_or_database() {
    let f = Fixture::new();
    f.write(
        "vault/.obsidian/community-plugins.json",
        &json!(["other-plugin"]),
    );
    f.write(
        "vault/.obsidian/core-plugins.json",
        &json!({"graph":false,"file-explorer":true,"bases":false}),
    );
    let authored = json!({"calendarView":"week","fieldMapping":{"priority":"importance","status":"state"},"customStatuses":[{"id":"personal","value":"REVIEW","label":"Review","color":"red"},{"id":"existing-todo","value":"TODO","label":"Old","autoArchive":true}]});
    f.write("vault/.obsidian/plugins/tasknotes/data.json", &authored);
    let result = f.run(true, true)["result"].clone();
    assert_eq!(result["installed"], true);
    assert_eq!(result["restart_required"], true);
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other-plugin", "tasknotes", "taskcli-sync"])
    );
    assert_eq!(
        f.read("vault/.obsidian/core-plugins.json"),
        json!({"graph":false,"file-explorer":true,"bases":true})
    );
    let settings = f.read("vault/.obsidian/plugins/tasknotes/data.json");
    assert_eq!(settings["calendarView"], "week");
    assert_eq!(settings["fieldMapping"]["priority"], "importance");
    assert_eq!(settings["fieldMapping"]["status"], "status");
    for (key, field) in [
        ("dateCreated", "created_at"),
        ("dateModified", "updated_at"),
        ("completedDate", "completed_at"),
    ] {
        assert_eq!(settings["fieldMapping"][key], field);
    }
    assert_eq!(settings["taskTag"], "task");
    assert_eq!(settings["taskIdentificationMethod"], "tag");
    assert_eq!(settings["defaultTaskStatus"], "TODO");
    assert_eq!(settings["openTaskAfterCreation"], "none");
    assert_eq!(settings["singleClickAction"], "openNote");
    let preset: Value = serde_json::from_str(include_str!(
        "../../../plugins/agent-task-manager/obsidian/tasknotes-settings.json"
    ))
    .unwrap();
    let statuses = settings["customStatuses"].as_array().unwrap();
    assert_eq!(statuses.len(), 11);
    for expected in preset["customStatuses"].as_array().unwrap() {
        let actual = statuses
            .iter()
            .find(|s| s["value"] == expected["value"])
            .unwrap();
        for key in ["color", "isCompleted", "autoArchive", "order"] {
            assert_eq!(actual[key], expected[key]);
        }
    }
    assert!(statuses.iter().any(|s| s == &authored["customStatuses"][0]));
    let backup = Path::new(result["backup"].as_str().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(backup.join("plugins/tasknotes/data.json")).unwrap()
        )
        .unwrap(),
        authored
    );
    let before = fs::read(f.path("vault/.obsidian/plugins/tasknotes/data.json")).unwrap();
    let repeat = f.run(false, true);
    assert_eq!(repeat["result"]["installed"], false);
    assert_eq!(repeat["result"]["changed"], false);
    assert_eq!(
        before,
        fs::read(f.path("vault/.obsidian/plugins/tasknotes/data.json")).unwrap()
    );
    assert!(!f.path("tasks.sqlite3").exists());
    assert!(!f.path("vault/Tasks").exists());
}

#[test]
fn enables_bases_in_legacy_array_without_losing_other_plugins() {
    let f = Fixture::new();
    f.write(
        "vault/.obsidian/core-plugins.json",
        &json!(["graph", "daily-notes"]),
    );
    f.run(true, true);
    assert_eq!(
        f.read("vault/.obsidian/core-plugins.json"),
        json!(["graph", "daily-notes", "bases"])
    );
}

#[test]
fn malformed_settings_or_plugin_lists_are_preserved_before_installation() {
    for (file, content) in [
        ("plugins/tasknotes/data.json", "not json"),
        ("plugins/tasknotes/data.json", "[]"),
        ("community-plugins.json", "{}"),
        ("plugins/taskcli-sync/data.json", "[]"),
        ("core-plugins.json", "null"),
        ("plugins/tasknotes/data.json", "{\"customStatuses\":false}"),
        ("plugins/tasknotes/data.json", "{\"fieldMapping\":[]}"),
    ] {
        let f = Fixture::new();
        let path = f.path(&format!("vault/.obsidian/{file}"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        f.run(true, false);
        assert_eq!(fs::read_to_string(path).unwrap(), content);
        assert!(!f.path("vault/.obsidian/plugins/tasknotes/main.js").exists());
        assert!(!f.path("tasks.sqlite3").exists());
    }
}

#[test]
fn invalid_or_incomplete_bundles_leave_vault_unchanged() {
    for bad in ["identity", "version", "missing", "empty"] {
        let f = Fixture::new();
        match bad {
            "identity" => f.write(
                "bundle/manifest.json",
                &json!({"id":"other","version":"4.12.5"}),
            ),
            "version" => f.write(
                "bundle/manifest.json",
                &json!({"id":"tasknotes","version":"3.0.0"}),
            ),
            "missing" => fs::remove_file(f.path("bundle/main.js")).unwrap(),
            _ => fs::write(f.path("bundle/main.js"), "").unwrap(),
        }
        f.run(true, false);
        assert_eq!(fs::read_dir(f.path("vault/.obsidian")).unwrap().count(), 0);
    }
}

#[test]
fn markdown_configuration_is_rejected_without_creating_task_state() {
    let f = Fixture::new();
    let path = f.path("config.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("\"obsidian\"", "\"markdown\""),
    )
    .unwrap();
    let result = f.run(true, false);
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Obsidian")
    );
    assert!(!f.path("tasks.sqlite3").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_configuration_paths_cannot_change_external_files() {
    for relative in [
        ".obsidian",
        ".obsidian/plugins",
        ".obsidian/plugins/tasknotes",
        ".obsidian/community-plugins.json",
        ".obsidian/plugins/tasknotes/data.json",
        ".obsidian/plugins/taskcli-sync",
        ".obsidian/plugins/taskcli-sync/data.json",
    ] {
        let f = Fixture::new();
        let external = f.path("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("sentinel"), "unchanged").unwrap();
        let dest = f.path(&format!("vault/{relative}"));
        if dest.exists() {
            fs::remove_dir(&dest).unwrap();
        }
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&external, &dest).unwrap();
        f.run(true, false);
        assert_eq!(fs::read_dir(external).unwrap().count(), 1);
    }
}

#[test]
fn ambiguous_status_definitions_are_rejected_before_installation() {
    for statuses in [
        json!([{"id":"one","value":"TODO"},{"id":"two","value":"TODO"}]),
        json!([{"id":"agent-todo","value":"REVIEW"}]),
    ] {
        let f = Fixture::new();
        f.write(
            "vault/.obsidian/plugins/tasknotes/data.json",
            &json!({"customStatuses":statuses}),
        );
        f.run(true, false);
        assert!(!f.path("vault/.obsidian/plugins/tasknotes/main.js").exists());
    }
}

#[test]
fn installs_sync_plugin_with_absolute_paths_and_preserves_user_configuration() {
    let f = Fixture::new();
    f.run(true, true);
    let manifest = f.read("vault/.obsidian/plugins/taskcli-sync/manifest.json");
    assert_eq!(manifest["id"], "taskcli-sync");
    assert_eq!(manifest["isDesktopOnly"], true);
    assert!(
        f.path("vault/.obsidian/plugins/taskcli-sync/main.js")
            .is_file()
    );
    let config = f.read("vault/.obsidian/plugins/taskcli-sync/data.json");
    assert_eq!(
        Path::new(config["cliPath"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        Path::new(env!("CARGO_BIN_EXE_taskcli"))
            .canonicalize()
            .unwrap()
    );
    assert_eq!(
        Path::new(config["configPath"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        f.path("config.toml").canonicalize().unwrap()
    );
    let custom =
        json!({"cliPath":"/custom/taskcli", "configPath":"/custom/config.toml", "other":true});
    f.write("vault/.obsidian/plugins/taskcli-sync/data.json", &custom);
    f.run(false, true);
    assert_eq!(f.run(false, true)["result"]["changed"], false);
    assert_eq!(
        f.read("vault/.obsidian/plugins/taskcli-sync/data.json"),
        custom
    );
    let settings = f.read("vault/.obsidian/plugins/tasknotes/data.json");
    for (value, color) in [
        ("ACTIVE", "#bfdbfe"),
        ("PENDING_REVIEW", "#fed7aa"),
        ("COMPLETED", "#bbf7d0"),
    ] {
        let status = settings["customStatuses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["value"] == value)
            .unwrap();
        assert_eq!(status["color"], color);
        assert_eq!(status["isCompleted"], value == "COMPLETED");
        assert_eq!(status["excludeFromCycle"], true);
        assert_eq!(status["autoArchive"], false);
    }
}

#[test]
fn setup_reloads_the_configured_vault_after_installing_files() {
    let f = Fixture::new();
    let result = f.run_with(true, true, &[], Some("success"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    assert_eq!(result["restart_required"], false);
    assert!(result["reload_error"].is_null());
    assert!(f.path("vault/reloaded").exists());
    let calls = fs::read_to_string(f.path("vault/cli-calls")).unwrap();
    assert!(calls.contains("vault=vault"));
    assert!(calls.contains("info=path"));
    assert!(calls.ends_with("reload\n"));
    fs::remove_file(f.path("vault/cli-calls")).unwrap();
    let repeat = f.run_with(false, true, &[], Some("success"));
    assert_eq!(repeat["result"]["changed"], false);
    assert_eq!(repeat["result"]["reloaded"], false);
    assert!(!f.path("vault/cli-calls").exists());
}

#[test]
fn setup_no_reload_does_not_contact_obsidian() {
    let f = Fixture::new();
    let result = f.run_with(true, true, &["--no-reload"], Some("success"));
    assert_eq!(result["result"]["reloaded"], false);
    assert_eq!(result["result"]["restart_required"], true);
    assert!(!f.path("vault/cli-calls").exists());
}

#[test]
fn setup_reload_failures_keep_the_installation_and_explain_manual_recovery() {
    for scenario in [
        None,
        Some("wrong-vault"),
        Some("reload-fails"),
        Some("reload-error-output"),
    ] {
        let f = Fixture::new();
        let result = f.run_with(true, true, &[], scenario)["result"].clone();
        assert_eq!(result["reloaded"], false, "{scenario:?}: {result}");
        assert_eq!(result["restart_required"], true);
        assert!(
            result["reload_error"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        assert!(
            result["next_step"]
                .as_str()
                .unwrap()
                .contains("restart Obsidian")
        );
        assert!(
            f.path("vault/.obsidian/plugins/taskcli-sync/main.js")
                .exists()
        );
        assert!(f.path("vault/.obsidian/plugins/tasknotes/main.js").exists());
        if scenario == Some("wrong-vault") {
            let calls = fs::read_to_string(f.path("vault/cli-calls")).unwrap();
            assert!(!calls.contains("plugin:disable"));
            assert!(!calls.contains("reload"));
        }
    }
}

#[test]
fn setup_preserves_settings_saved_during_plugin_shutdown() {
    let f = Fixture::new();
    f.write(
        "vault/.obsidian/community-plugins.json",
        &json!(["other", "tasknotes", "taskcli-sync"]),
    );
    let result = f.run_with(true, true, &[], Some("loaded"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    let settings = f.read("vault/.obsidian/plugins/tasknotes/data.json");
    assert_eq!(settings["calendarView"], "month");
    assert_eq!(settings["taskTag"], "task");
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other", "tasknotes", "taskcli-sync"])
    );
    let backup = Path::new(result["backup"].as_str().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(backup.join("plugins/tasknotes/data.json")).unwrap()
        )
        .unwrap()["taskTag"],
        "old"
    );
}

#[test]
fn setup_restores_disabled_plugins_if_publication_fails() {
    let f = Fixture::new();
    f.run_with(true, false, &[], Some("publication-fails"));
    let calls = fs::read_to_string(f.path("vault/cli-calls")).unwrap();
    assert!(calls.contains("plugin:enable"));
    assert!(!calls.contains("reload"));
}

#[test]
fn setup_reload_timeout_keeps_installed_files() {
    let f = Fixture::new();
    let result = f.run_with(true, true, &[], Some("reload-timeout"))["result"].clone();
    assert_eq!(result["restart_required"], true);
    assert!(
        result["reload_error"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    assert!(f.path("vault/.obsidian/plugins/tasknotes/main.js").exists());
}

#[test]
fn setup_retains_desired_enabled_list_when_reload_or_shutdown_fails() {
    for scenario in ["loaded-reload-fails", "disable-fails"] {
        let f = Fixture::new();
        let result = f.run_with(true, true, &[], Some(scenario))["result"].clone();
        assert_eq!(result["restart_required"], true);
        assert!(result["reload_error"].is_string());
        assert_eq!(
            f.read("vault/.obsidian/community-plugins.json"),
            json!(["other", "tasknotes", "taskcli-sync"])
        );
    }
}

#[test]
fn setup_passes_vault_names_with_spaces_as_one_argument() {
    let f = Fixture::new();
    let renamed = f.path("vault with spaces");
    fs::rename(f.path("vault"), &renamed).unwrap();
    let config_path = f.path("config.toml");
    let mut config: Config = toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config.documents.root = renamed.clone();
    fs::write(config_path, toml::to_string(&config).unwrap()).unwrap();
    let result = f.run_with(true, true, &[], Some("success"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    assert!(renamed.join("reloaded").exists());
}

#[test]
fn setup_merges_configuration_initialized_when_the_cli_launches_obsidian() {
    let f = Fixture::new();
    let result = f.run_with(true, true, &[], Some("startup-settings"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    assert_eq!(
        f.read("vault/.obsidian/core-plugins.json"),
        json!({"graph":true,"bases":true})
    );
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other", "tasknotes", "taskcli-sync"])
    );
}
