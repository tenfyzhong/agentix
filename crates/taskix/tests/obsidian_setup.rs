use std::{fs, path::Path, process::Command};

use agentix_task::{Config, DocumentConfig, StorageConfig};
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
        let config = Config {
            schema_version: 1,
            storage: StorageConfig {
                path: f.path("tasks.sqlite3"),
            },
            documents: DocumentConfig {
                root: f.path("vault"),
                directory: "Tasks".into(),
                archive_directory: "Archived Projects".into(),
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
    fn run(&self, success: bool) -> Value {
        self.run_with(success, &[], None)
    }
    fn run_with(&self, success: bool, args: &[&str], scenario: Option<&str>) -> Value {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskix"));
        cmd.arg("--config")
            .arg(self.path("config.toml"))
            .args(["--json", "obsidian", "setup"]);
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
fn setup_ignores_tasknotes_and_installs_only_native_bases_and_sync() {
    let f = Fixture::new();
    f.write(
        "vault/.obsidian/community-plugins.json",
        &json!(["other", "tasknotes"]),
    );
    f.write(
        "vault/.obsidian/plugins/tasknotes/manifest.json",
        &json!({"id":"tasknotes", "version":"3.0.0"}),
    );
    let legacy = f.path("vault/.obsidian/plugins/tasknotes/data.json");
    fs::write(&legacy, "legacy settings left untouched").unwrap();
    let result = f.run(true)["result"].clone();
    assert_eq!(result["min_obsidian_version"], "1.14.0");
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other", "taskix-sync"])
    );
    assert_eq!(
        fs::read_to_string(legacy).unwrap(),
        "legacy settings left untouched"
    );
    assert!(
        f.path("vault/.obsidian/plugins/taskix-sync/styles.css")
            .is_file()
    );
    assert!(!f.path("vault/.obsidian/plugins/tasknotes/main.js").exists());
    assert!(!f.path("tasks.sqlite3").exists());
}

#[test]
fn installs_native_bases_and_sync_offline_and_repeats_without_database() {
    let f = Fixture::new();
    f.write(
        "vault/.obsidian/community-plugins.json",
        &json!(["other-plugin", "tasknotes"]),
    );
    f.write(
        "vault/.obsidian/core-plugins.json",
        &json!({"graph":false,"file-explorer":true,"bases":false}),
    );
    let result = f.run(true)["result"].clone();
    assert_eq!(result["installed"], true);
    assert_eq!(result["restart_required"], true);
    assert_eq!(result["min_obsidian_version"], "1.14.0");
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other-plugin", "taskix-sync"])
    );
    assert_eq!(
        f.read("vault/.obsidian/core-plugins.json"),
        json!({"graph":false,"file-explorer":true,"bases":true})
    );
    let backup = Path::new(result["backup"].as_str().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(backup.join("community-plugins.json")).unwrap())
            .unwrap(),
        json!(["other-plugin", "tasknotes"])
    );
    let repeat = f.run(true);
    assert_eq!(repeat["result"]["installed"], false);
    assert_eq!(repeat["result"]["changed"], false);
    assert!(!f.path("vault/.obsidian/plugins/tasknotes").exists());
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
    f.run(true);
    assert_eq!(
        f.read("vault/.obsidian/core-plugins.json"),
        json!(["graph", "daily-notes", "bases"])
    );
}

#[test]
fn malformed_settings_or_plugin_lists_are_preserved_before_installation() {
    for (file, content) in [
        ("community-plugins.json", "{}"),
        ("plugins/taskix-sync/data.json", "[]"),
        ("core-plugins.json", "null"),
    ] {
        let f = Fixture::new();
        let path = f.path(&format!("vault/.obsidian/{file}"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        f.run(false);
        assert_eq!(fs::read_to_string(path).unwrap(), content);
        assert!(!f.path("vault/.obsidian/plugins/tasknotes/main.js").exists());
        assert!(!f.path("tasks.sqlite3").exists());
    }
}

#[test]
fn non_vault_configuration_is_rejected_without_creating_task_state() {
    let f = Fixture::new();
    fs::remove_dir(f.path("vault/.obsidian")).unwrap();
    let result = f.run(false);
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
        ".obsidian/community-plugins.json",
        ".obsidian/plugins/taskix-sync",
        ".obsidian/plugins/taskix-sync/data.json",
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
        f.run(false);
        assert_eq!(fs::read_dir(external).unwrap().count(), 1);
    }
}

#[test]
fn installs_sync_plugin_with_absolute_paths_and_preserves_user_configuration() {
    let f = Fixture::new();
    f.run(true);
    let manifest = f.read("vault/.obsidian/plugins/taskix-sync/manifest.json");
    assert_eq!(manifest["id"], "taskix-sync");
    assert_eq!(manifest["isDesktopOnly"], true);
    assert_eq!(manifest["minAppVersion"], "1.14.0");
    assert!(
        f.path("vault/.obsidian/plugins/taskix-sync/main.js")
            .is_file()
    );
    let config = f.read("vault/.obsidian/plugins/taskix-sync/data.json");
    assert_eq!(
        Path::new(config["cliPath"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        Path::new(env!("CARGO_BIN_EXE_taskix"))
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
        json!({"cliPath":"/custom/taskix", "configPath":"/custom/config.toml", "other":true});
    f.write("vault/.obsidian/plugins/taskix-sync/data.json", &custom);
    f.run(true);
    assert_eq!(f.run(true)["result"]["changed"], false);
    assert_eq!(
        f.read("vault/.obsidian/plugins/taskix-sync/data.json"),
        custom
    );
}

#[test]
fn setup_reloads_the_configured_vault_after_installing_files() {
    let f = Fixture::new();
    let result = f.run_with(true, &[], Some("success"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    assert_eq!(result["restart_required"], false);
    assert!(result["reload_error"].is_null());
    assert!(f.path("vault/reloaded").exists());
    let calls = fs::read_to_string(f.path("vault/cli-calls")).unwrap();
    assert!(calls.contains("vault=vault"));
    assert!(calls.contains("info=path"));
    assert!(calls.ends_with("reload\n"));
    fs::remove_file(f.path("vault/cli-calls")).unwrap();
    let repeat = f.run_with(true, &[], Some("success"));
    assert_eq!(repeat["result"]["changed"], false);
    assert_eq!(repeat["result"]["reloaded"], false);
    assert!(!f.path("vault/cli-calls").exists());
}

#[test]
fn setup_no_reload_does_not_contact_obsidian() {
    let f = Fixture::new();
    let result = f.run_with(true, &["--no-reload"], Some("success"));
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
        let result = f.run_with(true, &[], scenario)["result"].clone();
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
            f.path("vault/.obsidian/plugins/taskix-sync/main.js")
                .exists()
        );
        assert!(
            f.path("vault/.obsidian/plugins/taskix-sync/styles.css")
                .exists()
        );
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
        &json!(["other", "taskix-sync"]),
    );
    let result = f.run_with(true, &[], Some("loaded"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    let settings = f.read("vault/.obsidian/plugins/taskix-sync/data.json");
    assert_eq!(settings["savedDuringShutdown"], true);
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other", "taskix-sync"])
    );
    let backup = Path::new(result["backup"].as_str().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(backup.join("plugins/taskix-sync/data.json")).unwrap()
        )
        .unwrap()["savedDuringShutdown"],
        true
    );
}

#[test]
fn setup_restores_disabled_plugins_if_publication_fails() {
    let f = Fixture::new();
    f.run_with(false, &[], Some("publication-fails"));
    let calls = fs::read_to_string(f.path("vault/cli-calls")).unwrap();
    assert!(calls.contains("plugin:enable"));
    assert!(!calls.contains("reload"));
}

#[test]
fn setup_reload_timeout_keeps_installed_files() {
    let f = Fixture::new();
    let result = f.run_with(true, &[], Some("reload-timeout"))["result"].clone();
    assert_eq!(result["restart_required"], true);
    assert!(
        result["reload_error"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    assert!(
        f.path("vault/.obsidian/plugins/taskix-sync/styles.css")
            .exists()
    );
}

#[test]
fn setup_retains_desired_enabled_list_when_reload_or_shutdown_fails() {
    for scenario in ["loaded-reload-fails", "disable-fails"] {
        let f = Fixture::new();
        let result = f.run_with(true, &[], Some(scenario))["result"].clone();
        assert_eq!(result["restart_required"], true);
        assert!(result["reload_error"].is_string());
        assert_eq!(
            f.read("vault/.obsidian/community-plugins.json"),
            json!(["other", "taskix-sync"])
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
    let result = f.run_with(true, &[], Some("success"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    assert!(renamed.join("reloaded").exists());
}

#[test]
fn setup_merges_configuration_initialized_when_the_cli_launches_obsidian() {
    let f = Fixture::new();
    let result = f.run_with(true, &[], Some("startup-settings"))["result"].clone();
    assert_eq!(result["reloaded"], true);
    assert_eq!(
        f.read("vault/.obsidian/core-plugins.json"),
        json!({"graph":true,"bases":true})
    );
    assert_eq!(
        f.read("vault/.obsidian/community-plugins.json"),
        json!(["other", "taskix-sync"])
    );
}
