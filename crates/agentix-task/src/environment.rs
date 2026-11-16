//! Generate literal shell exports without initializing runtime configuration.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail};
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

/// Parse Node dotenv values, retaining portable names for safe shell export.
/// Shared fixtures exercise this contract against node:util.parseEnv.
pub fn parse(source: &str) -> Result<BTreeMap<String, String>> {
    if source.contains('\0') {
        bail!("invalid environment configuration");
    }
    let source = source.replace('\r', "");
    let trim = |s: &str| s.trim_matches([' ', '\t', '\n']).to_owned();
    let mut remaining = source.as_str();
    let mut values = BTreeMap::new();
    while !remaining.is_empty() {
        remaining = remaining.trim_matches([' ', '\t', '\n']);
        if remaining.is_empty() {
            break;
        }
        if remaining.starts_with('#') {
            remaining = remaining.split_once('\n').map_or("", |(_, rest)| rest);
            continue;
        }
        let Some(boundary) = remaining.find(['=', '\n']) else {
            break;
        };
        if remaining.as_bytes()[boundary] == b'\n' {
            remaining = &remaining[boundary + 1..];
            continue;
        }
        let key = trim(&remaining[..boundary]);
        let key = key.strip_prefix("export ").map_or(key.as_str(), str::trim);
        remaining = &remaining[boundary + 1..];
        if !valid_key(key) {
            bail!("invalid environment configuration");
        }
        if remaining.is_empty() || remaining.starts_with('\n') {
            values.insert(key.to_owned(), String::new());
            continue;
        }
        remaining = remaining.trim_matches([' ', '\t', '\n']);
        let quote = remaining
            .chars()
            .next()
            .filter(|c| matches!(c, '\'' | '"' | '`'));
        let closing = quote.and_then(|q| remaining[1..].find(q).map(|i| i + 1));
        let value;
        if let Some(end) = closing {
            value = if quote == Some('"') {
                remaining[1..end].replace("\\n", "\n")
            } else {
                remaining[1..end].to_owned()
            };
            remaining = remaining[end + 1..]
                .split_once('\n')
                .map_or("", |(_, rest)| rest);
        } else {
            let (line, rest) = remaining.split_once('\n').unwrap_or((remaining, ""));
            value = if quote.is_some() {
                line.to_owned()
            } else {
                trim(line.split_once('#').map_or(line, |(value, _)| value))
            };
            remaining = rest;
        }
        values.insert(key.to_owned(), value);
    }
    Ok(values)
}

/// The env file lives beside the selected TOML, even when TOML does not exist.
pub fn path(config: &Path, app: &str) -> Result<std::path::PathBuf> {
    Ok(crate::expand_home(config)?.with_file_name(format!("{app}.env")))
}

// Keep the path allowlist aligned with plugins/shared/environment.mjs.
fn expand_paths(mut values: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    const PATH_KEYS: &[&str] = &[
        "AGENTIX_CONFIG",
        "AGENTIX_LOGIN_SHELL",
        "AGENTIX_CLAUDE_DATA_DIR",
        "AGENTIX_OPENCODE_STATE_DIR",
        "AGENTIX_ANTIGRAVITY_DATA_DIR",
        "AGENTIX_ANTIGRAVITY_SESSION_DIR",
        "CODEX_HOME",
        "TASKIX_CONFIG",
        "TASKIX_BIN",
        "TASKIX_LOGIN_SHELL",
        "TASKIX_ANTIGRAVITY_DATA_DIR",
        "TASKIX_DECISION_METRICS_DB",
        "TASKIX_JEV_METRICS_DB",
        "XDG_STATE_HOME",
    ];
    for (key, value) in &mut values {
        if PATH_KEYS.contains(&key.as_str()) && (value == "~" || value.starts_with("~/")) {
            let home = std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
                .map(std::path::PathBuf::from)
                .or_else(dirs::home_dir)
                .context("home directory unavailable")?;
            *value = if value == "~" {
                home
            } else {
                home.join(value[2..].trim_start_matches('/'))
            }
            .to_string_lossy()
            .into_owned();
        }
    }
    Ok(values)
}

pub fn load(path: &Path) -> Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(path) {
        Ok(source) => expand_paths(parse(&source)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error).context("read environment configuration"),
    }
}

/// Read a dedicated env file and return a complete script, never partial output.
pub fn export(path: &Path, shell: Option<&str>) -> Result<String> {
    let path = crate::expand_home(path)?;
    let source = std::fs::read_to_string(&path).context("read environment configuration")?;
    let values = expand_paths(parse(&source)?)?;
    let shell = shell.map_or_else(Dialect::detect, Dialect::parse)?;
    Ok(values
        .into_iter()
        .map(|(key, value)| shell.assignment(&key, &value))
        .collect())
}

/// Start with file defaults without unsafe mutation of a process environment.
/// Unix exec preserves the PID and signals. Windows waits for the replacement.
/// Existing environment values, including empty ones, always win.
pub fn bootstrap(config: &Path, app: &str) -> Result<()> {
    let marker = format!("{}_LOCAL_ENV_PID", app.to_ascii_uppercase());
    if std::env::var(&marker).ok().as_deref() == Some(&std::process::id().to_string()) {
        return Ok(());
    }
    let values = load(&path(config, app)?)?;
    let defaults: BTreeMap<_, _> = values
        .into_iter()
        .filter(|(key, _)| std::env::var_os(key).is_none())
        .collect();
    if defaults.is_empty() {
        return Ok(());
    }
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.args(std::env::args_os().skip(1)).envs(defaults);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.env(marker, std::process::id().to_string());
        Err(command.exec()).context("restart with local environment configuration")
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .context("start with local environment configuration")?;
        std::process::exit(status.code().unwrap_or(1))
    }
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
    fn loads_and_exports_home_paths_without_expanding_other_values() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("taskix.env");
        std::fs::write(&file, "TASKIX_BIN=~/bin/taskix\nCODEX_HOME=~\nOPENAI_API_KEY=~/literal\nCUSTOM_PATH=~/literal\nTASKIX_LOGIN_SHELL=~someone/bin/fish\nAGENTIX_LOGIN_SHELL=$HOME/bin/fish\n").unwrap();
        let values = load(&file).unwrap();
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
            .map(std::path::PathBuf::from)
            .or_else(dirs::home_dir)
            .unwrap();
        assert_eq!(
            values["TASKIX_BIN"],
            home.join("bin/taskix").to_string_lossy()
        );
        assert_eq!(values["CODEX_HOME"], home.to_string_lossy());
        assert_eq!(values["OPENAI_API_KEY"], "~/literal");
        assert_eq!(values["CUSTOM_PATH"], "~/literal");
        assert_eq!(values["TASKIX_LOGIN_SHELL"], "~someone/bin/fish");
        assert_eq!(values["AGENTIX_LOGIN_SHELL"], "$HOME/bin/fish");
        let script = export(&file, Some("fish")).unwrap();
        assert!(script.contains(&Dialect::Fish.assignment("TASKIX_BIN", &values["TASKIX_BIN"])));
    }

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
    fn shared_plugin_syntax_fixtures() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../plugins/shared/environment-fixtures.json"
        ))
        .unwrap();
        for item in cases["valid"].as_array().unwrap() {
            assert_eq!(
                serde_json::to_value(parse(item["source"].as_str().unwrap()).unwrap()).unwrap(),
                item["values"]
            );
        }
        for source in cases["invalid"].as_array().unwrap() {
            assert!(parse(source.as_str().unwrap()).is_err());
        }
    }

    #[test]
    fn literal_environment_parser_matches_plugin_contract() {
        let values =
            parse("# comment\r\nA=one=two\nB=\"$HOME $(secret)\"\nC='a\\b'\nEMPTY=\n").unwrap();
        assert_eq!(values["A"], "one=two");
        assert_eq!(values["B"], "$HOME $(secret)");
        assert_eq!(values["C"], "a\\b");
        assert_eq!(values["EMPTY"], "");
        for source in ["BAD-KEY=secret", "A=secret\0"] {
            let error = parse(source).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
    }
}
