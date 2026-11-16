# Local environment files

Agentix and Taskix load local defaults automatically from `agentix.env` and
`taskix.env`, beside their respective `config.toml` files. The default locations
are `~/.config/agentix/agentix.env` and `~/.config/taskix/taskix.env`.
Copy `config/agentix.env.example` or `config/taskix.env.example` and uncomment the
settings you need. No shell export or third-party dotenv/TOML parser is required
for host plugins.

For example, `~/.config/taskix/taskix.env`:

```dotenv
TASKIX_LANG=zh-CN
TASKIX_MEMORY_ENABLED=true
TASKIX_DECISION_ENABLED=true
TASKIX_DECISION_PROVIDER=typesafe
TASKIX_DECISION_URL=https://api.typesafe.ai/v1/systemone
TASKIX_DECISION_API_KEY=replace-with-your-key
```

Explicit command options take priority over inherited process environment,
which takes priority over file defaults. An inherited empty value still overrides
the file. TOML retains structured application configuration and provider
`api_key_env` references; put the referenced credential in the env file.

CLIs load defaults at startup before runtime configuration. On Unix they use
`exec` when defaults must be added, preserving PID and signals without unsafe
process-environment mutation. On Windows the bootstrap waits for the replacement
process and returns its exit status. Child processes inherit the resulting
values. This never modifies a parent shell or fish universal variables.

Agentix Bridge and Taskix Manager independently load their own files, including
when started directly by an agent host. Plugins cache file contents in memory
once per resolved path for the lifetime of the process, and do not modify the
host's shared `process.env`. Restart a resident host/plugin or service after
editing a file. New hook processes see the new file automatically. Missing files
are optional during normal startup; invalid variable names, NUL bytes or unreadable files report errors.

Use `--config PATH` to select a CLI TOML file and its adjacent env file. Taskix
also honors inherited `TASKIX_CONFIG`. Standalone plugins honor inherited
`TASKIX_CONFIG` or `AGENTIX_CONFIG` to locate their corresponding files.
Set these selectors outside the env file; discovery happens before it is read.

## Dotenv format

Plugins use Node's built-in `util.parseEnv`, without a third-party parser or
`process.loadEnvFile`. The API requires Node 20.12.0 / 21.7.0 or newer; use a
current Node LTS release. Compatible Bun hosts must provide `node:util.parseEnv`.
Rust uses the same value rules, covered by shared fixtures tested against Node.

Use `KEY=VALUE` with portable names matching `[A-Za-z_][A-Za-z0-9_]*`.
An optional `export ` prefix is accepted. Duplicate keys use the last value.
Unquoted values are trimmed and `#` starts a comment; quote values containing
literal `#`. Matching single, double or backtick quotes preserve whitespace
and allow multiline values. Double-quoted `\n` becomes a newline; single quotes
preserve backslashes. Empty values are supported. All values remain strings.

There is no variable interpolation, command substitution or shell evaluation.
`$HOME`, `${VAR}`, `$(...)` and `~user` remain literal.
As with Node, lines without `=` are ignored and an unmatched opening quote
remains part of the line's value. This is not a strict syntax checker.
NUL bytes and parsed nonportable variable names are rejected without exposing
values in diagnostics, so generated shell exports remain safe.

## Home shorthand in paths

Known filesystem settings accept `~` and `~/...` in both env files. CLIs,
plugins and optional shell exports expand them using inherited `HOME`, then
`USERPROFILE`, then the operating system home directory. An env-file `HOME`
value does not change this base. Inherited overrides remain unchanged.

Supported keys are `AGENTIX_CONFIG`, `AGENTIX_LOGIN_SHELL`,
`AGENTIX_CLAUDE_DATA_DIR`, `AGENTIX_OPENCODE_STATE_DIR`,
`AGENTIX_ANTIGRAVITY_DATA_DIR`, `AGENTIX_ANTIGRAVITY_SESSION_DIR`, `CODEX_HOME`,
`TASKIX_CONFIG`, `TASKIX_BIN` (legacy), `TASKIX_LOGIN_SHELL`,
`TASKIX_ANTIGRAVITY_DATA_DIR`, `TASKIX_DECISION_METRICS_DB`,
and `TASKIX_JEV_METRICS_DB` (legacy).
Config selectors still need to be set outside the file for discovery.
`TASKIX_BIN` expansion does not restore the legacy executable override; plugins
resolve the Taskix executable through `PATH`.

```dotenv
CODEX_HOME=~/.codex
TASKIX_DECISION_METRICS_DB=~/.local/state/taskix/decision-metrics.sqlite
```

Other values, including credentials, URLs and unknown keys, remain literal even
when they begin with `~/`. For example, `OPENAI_API_KEY=~/secret` is a literal
key, not a reference to a file. Path expansion does not execute shell code.

`XDG_STATE_HOME` is a global host setting. Both application env files ignore
this key during loading and shell export; an inherited global value (including
an empty value) remains unchanged. Configure it in the host environment, not in
`agentix.env` or `taskix.env`.

Logging is configured only through `[logging].level` in each application's
`config.toml`. `RUST_LOG` in application env files is ignored during loading and
export. An inherited `RUST_LOG` does not override either application's logger.

## Migration

Move entries out of the old TOML `[environment]` table into the matching env
file, removing the table header and TOML-specific escaping. The old table is no
longer used for exports or automatic loading. Remove old shell definitions if
you want file defaults to take effect; in fish, remove the corresponding
universal variables with `set -eU NAME` (and any shadowing global/local values).
Neither installed configuration nor existing shell variables is changed for you.

## Optional shell export

`agentix env` and `taskix env` still print executable assignments from these same
env files, without initializing services or databases. They export file values,
not unrelated inherited variables. Use this only when another tool needs them.

```fish
agentix env | source
taskix env | source
```

Fish output uses `set -Ux`, so explicitly applying it persists values across
sessions. In bash, zsh or POSIX sh, use `eval "$(taskix env)"`; in PowerShell use
`taskix env | Out-String | Invoke-Expression` (likewise for Agentix).

Shell detection walks the calling process ancestry, with `$SHELL` as fallback.
Override with `env --shell sh|bash|zsh|fish|powershell|pwsh`. Unsupported shells
produce an error. `--config PATH` still selects the adjacent env file, and
`taskix env` rejects `--json`. An explicitly requested export requires an existing
env file; an empty file emits nothing. Invalid entries fail before any script is
printed. Output is sorted and quoted literally. Protect files and generated
scripts containing credentials with appropriate file permissions.
