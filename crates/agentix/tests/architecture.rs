//! Dependency boundaries are architectural contracts, checked alongside behavior.
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

#[test]
fn native_hosts_have_one_production_bridge_path() {
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(root().join("Cargo.toml")).unwrap()).unwrap();
    let members = manifest["workspace"]["members"].as_array().unwrap();
    assert!(
        members
            .iter()
            .any(|member| member.as_str() == Some("crates/agentix-bridge"))
    );
    assert!(
        !members
            .iter()
            .any(|member| member.as_str() == Some("crates/agentix-pi")),
        "legacy subprocess Pi implementation must not remain in the workspace"
    );
    assert!(!root().join("crates/agentix-pi").exists());
}

fn dependencies(name: &str) -> toml::Table {
    let path = root().join("crates").join(name).join("Cargo.toml");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing architectural layer {}: {error}", path.display()));
    let manifest: toml::Value = toml::from_str(&source).unwrap();
    manifest["dependencies"].as_table().unwrap().clone()
}

#[test]
fn agent_adapters_do_not_depend_on_concrete_multiplexers() {
    for layer in ["agentix-core", "agentix-codex", "agentix-bridge"] {
        let deps = dependencies(layer);
        for forbidden in ["agentix-rmux", "agentix-tmux", "rmux-sdk"] {
            assert!(
                !deps.contains_key(forbidden),
                "{layer} must not depend on {forbidden}"
            );
        }
    }
}

#[test]
fn domain_and_storage_do_not_depend_on_application_or_host_adapters() {
    for layer in ["agentix-domain", "agentix-storage"] {
        let deps = dependencies(layer);
        for forbidden in [
            "agentix-core",
            "agentix-task",
            "agentix-codex",
            "agentix-bridge",
            "agentix-telegram",
            "agentix-feishu",
            "agentix-rmux",
        ] {
            assert!(
                !deps.contains_key(forbidden),
                "{layer} must not depend on {forbidden}"
            );
        }
        if layer == "agentix-domain" {
            assert!(!deps.contains_key("sqlx"));
            assert!(!deps.contains_key("agentix-storage"));
        }
    }
    let application = dependencies("agentix-core");
    assert!(application.contains_key("agentix-domain"));
    assert!(application.contains_key("agentix-storage"));
    assert!(
        !application.contains_key("sqlx"),
        "SQL implementation belongs to storage"
    );
}

#[test]
fn adapters_depend_on_contracts_without_importing_the_application() {
    for adapter in [
        "agentix-codex",
        "agentix-bridge",
        "agentix-rmux",
        "agentix-telegram",
        "agentix-feishu",
    ] {
        let deps = dependencies(adapter);
        assert!(
            deps.contains_key("agentix-domain"),
            "{adapter} needs domain contracts"
        );
        assert!(
            !deps.contains_key("agentix-core"),
            "{adapter} must not import application orchestration"
        );
        assert!(
            !deps.contains_key("agentix-storage"),
            "{adapter} must not import runtime persistence"
        );
    }
}
