//! Generate literal shell exports without initializing runtime configuration.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail, ensure};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Clone, Copy)]
enum Dialect {
    Posix,
    Fish,
    PowerShell,
}

impl Dialect {
    fn parse(name: &str) -> Result<Self> {
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let name = name.trim_start_matches('-').trim_end_matches(".exe");
        match name.to_ascii_lowercase().as_str() {
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => Ok(Self::Posix),
            "fish" => Ok(Self::Fish),
            "powershell" | "pwsh" => Ok(Self::PowerShell),
            _ => bail!("unsupported shell; use --shell sh, bash, zsh, fish, or powershell"),
        }
    }

    fn detect() -> Result<Self> {
        let mut system = System::new();
        let mut pid = Pid::from_u32(std::process::id());
        // Refresh only our ancestry, without collecting environment or command lines.
        for depth in 0..64 {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing(),
            );
            let Some(process) = system.process(pid) else {
                break;
            };
            if depth > 0 {
                let name = process.name().to_string_lossy();
                if let Ok(shell) = Self::parse(&name) {
                    return Ok(shell);
                }
                if matches!(
                    name.as_ref(),
                    "nu" | "nushell" | "csh" | "tcsh" | "elvish" | "xonsh" | "cmd.exe" | "cmd"
                ) {
                    bail!("unsupported calling shell; specify --shell for the target shell");
                }
            }
            let Some(parent) = process.parent().filter(|parent| *parent != pid) else {
                break;
            };
            pid = parent;
        }
        let fallback =
            std::env::var("SHELL").context("cannot detect calling shell; specify --shell")?;
        Self::parse(&fallback)
    }

    fn assignment(self, key: &str, value: &str) -> String {
        match self {
            Self::Posix => format!("export {key}='{}';\n", value.replace('\'', "'\\''")),
            Self::Fish => format!(
                "set -Ux {key} '{}';\n",
                value.replace('\\', "\\\\").replace('\'', "\\'")
            ),
            Self::PowerShell => format!("$env:{key} = '{}';\n", value.replace('\'', "''")),
        }
    }
}

/// Read only `[environment]`, then return a complete script or an error.
/// No output is written until every entry has been validated.
pub fn export(path: &Path, shell: Option<&str>) -> Result<String> {
    let path = crate::expand_home(path)?;
    let source = std::fs::read_to_string(&path)
        .with_context(|| format!("read environment configuration {}", path.display()))?;
    // Parser diagnostics can contain a source line with credentials.
    let config: toml::Table = toml::from_str(&source).map_err(|_| {
        anyhow::anyhow!(
            "invalid TOML in environment configuration {}",
            path.display()
        )
    })?;
    let mut values = BTreeMap::new();
    if let Some(environment) = config.get("environment") {
        let table = environment
            .as_table()
            .context("environment must be a table")?;
        for (key, value) in table {
            ensure!(valid_key(key), "invalid environment variable name");
            let value = match value {
                toml::Value::String(value) => value.clone(),
                toml::Value::Boolean(value) => value.to_string(),
                toml::Value::Integer(value) => value.to_string(),
                toml::Value::Float(value) if value.is_finite() => value.to_string(),
                _ => bail!("environment.{key} must be a string, boolean, integer, or finite float"),
            };
            ensure!(
                !value.contains('\0'),
                "environment.{key} must not contain NUL"
            );
            values.insert(key, value);
        }
    }
    let shell = shell.map_or_else(Dialect::detect, Dialect::parse)?;
    Ok(values
        .into_iter()
        .map(|(key, value)| shell.assignment(key, &value))
        .collect())
}

fn valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_portable_environment_identifiers() {
        for name in ["A", "_", "TASKIX_CONFIG", "http_proxy", "A1"] {
            assert!(valid_key(name), "{name}");
        }
        for name in ["", "1A", "A-B", "A=B", "A;echo", "A\nB", "变量"] {
            assert!(!valid_key(name), "{name}");
        }
    }

    #[test]
    fn recognizes_shell_paths_and_login_names() {
        for name in ["/bin/bash", "-zsh", "/bin/sh", "dash", "ksh", "ash"] {
            assert!(matches!(Dialect::parse(name).unwrap(), Dialect::Posix));
        }
        assert!(matches!(
            Dialect::parse("/usr/local/bin/fish").unwrap(),
            Dialect::Fish
        ));
        assert!(matches!(
            Dialect::parse("C:\\Program Files\\PowerShell\\7\\pwsh.exe").unwrap(),
            Dialect::PowerShell
        ));
        for name in ["", "nu", "fishy", "cmd.exe"] {
            assert!(Dialect::parse(name).is_err());
        }
    }

    #[test]
    fn rejects_non_scalar_environment_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for value in ["[]", "{}", "1979-05-27", "inf", "nan"] {
            std::fs::write(&path, format!("[environment]\nKEY = {value}\n")).unwrap();
            assert!(export(&path, Some("fish")).is_err(), "{value}");
        }
    }
}
