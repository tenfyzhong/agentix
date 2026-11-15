# Environment export

`agentix env` and `taskix env` print executable shell assignments from the
`[environment]` table in their respective configuration files. They do not start
services, open databases, validate runtime credentials, or change the parent
process environment. Apply the output in the shell that will launch the CLI,
agent host, or service.

The `[environment]` sections in `config/agentix.example.toml` and
`config/taskix.example.toml` list the current CLI, service, and host-plugin
settings with defaults and placeholder values. Uncomment the entries you need.
Compatibility aliases and host-owned/internal variables are documented separately
in comments and should not be exported as persistent user settings.

For example, add this table to `~/.config/taskix/config.toml`:

```toml
[environment]
AGENT_TASK_LANG = "zh-CN"
TASKIX_MEMORY_ENABLED = true
TASKIX_DECISION_ENABLED = true
TASKIX_DECISION_PROVIDER = "typesafe"
TASKIX_DECISION_URL = "https://api.typesafe.ai/v1/systemone"
TASKIX_DECISION_API_KEY = "replace-with-your-key"
```

The Agentix file (`~/.config/agentix/config.toml`) can use the same table for
variables such as `AGENTIX_LOGIN_SHELL`, `http_proxy`, `https_proxy`, and `NO_PROXY`.
Existing credential fields and `api_key_env` references keep their meanings;
use the exact referenced variable name as an environment key. Reading normal
configuration does not implicitly apply this table.

In fish:

```fish
agentix env | source
taskix env | source
```

The generated statements use `set -Ux`, so values persist across fish sessions.
An existing global or local variable of the same name can shadow a universal
variable; remove that shadowing definition when migrating existing shell setup.

In bash, zsh, or POSIX sh:

```sh
eval "$(agentix env)"
eval "$(taskix env)"
```

These exports affect the current shell and its future child processes. Put the
commands in the shell's startup file if they should run for every new session.

In PowerShell:

```powershell
agentix env | Out-String | Invoke-Expression
taskix env | Out-String | Invoke-Expression
```

Automatic detection walks the calling process ancestry to find the nearest
shell, so a nested fish shell wins over a login `$SHELL` naming zsh. If no shell
is visible, `$SHELL` is the fallback. Unsupported or undetectable shells produce
an error rather than guessed syntax. To generate a file for a different shell,
use `env --shell sh|bash|zsh|fish|powershell|pwsh`.

Select a different file with `agentix --config PATH env` or
`taskix --config PATH env`; Taskix also honors `TASKIX_CONFIG`. `taskix env` emits
only script text and rejects `--json`. Missing files and invalid entries fail
before any script is printed. An absent or empty table produces no assignments.

Keys must be shell identifiers (`[A-Za-z_][A-Za-z0-9_]*`). Values may be strings,
booleans, integers, or finite floating-point numbers. Arrays, tables, dates, and
NUL bytes are rejected. Values are quoted literally: dollar signs, backticks,
quotes, backslashes, and newlines are preserved without command execution.
Neither `~` nor `$HOME` in values is expanded. Variables are emitted in sorted
order; inherited environment variables and unrelated configuration fields are
not exported. Secrets in this table appear in the generated script; protect the
configuration file and any saved output with appropriate file permissions.

Restart existing agent hosts and services after updating their environment.
Exporting variables in a terminal does not modify already running processes or
unrelated desktop application environments.
