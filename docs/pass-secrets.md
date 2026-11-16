# Credentials from pass

Requires a build containing pass support. No new third-party Rust or Node package
is needed; pass, GnuPG and a working pinentry are external prerequisites.

## Configure pinentry first

**Configure and verify pinentry for the service/plugin user BEFORE enabling pass.
Otherwise configure API keys directly in the environment or taskix.env, and Agentix
channel secrets directly in TOML (or proxy credentials in their existing files).**
Do not commit plaintext keys to Git.

On Apple Silicon macOS, install pass and pinentry-mac. Add this to
`~/.gnupg/gpg-agent.conf`, adjusting the Homebrew prefix if necessary:

```conf
pinentry-program /opt/homebrew/bin/pinentry-mac
```

Reload with `gpgconf --reload gpg-agent`. Initialize pass with your GPG key if
necessary, then store and verify credentials interactively:

```sh
pass insert taskix/openai
pass insert taskix/decision
pass show taskix/openai > /dev/null
```

A logged-in user's LaunchAgent can use graphical pinentry. Terminal pinentry
requires a TTY. Before desktop login, over SSH or in system daemons, do not rely
on a visible dialog. `pinentry-mode loopback` does not use normal pinentry.
Verify decryption under the service user's context. Host hooks may have shorter
deadlines: unlock GPG before launching those hosts. Cancellation or timeout is
an error, not an automatic switch to another credential.

## Agentix channel and proxy credentials

Configure the pass entry directly in `agentix/config.toml`; no env indirection is
needed. Keep only the table for the channel you select:

```toml
[channel.telegram]
token_pass = "agentix/telegram/token"

[channel.feishu]
app_id = "your-app-id"
app_secret_pass = "agentix/feishu/app-secret"

[channel.slack]
bot_token_pass = "agentix/slack/bot-token"
app_token_pass = "agentix/slack/app-token"
```

Each `_pass` field is mutually exclusive with a nonblank direct `token`,
`app_secret`, `bot_token` or `app_token`. App IDs are identifiers and remain direct
values. Loading/validating configuration and exporting environment scripts do not
decrypt credentials. Only the selected channel resolves its credentials when it
is used, including connectivity checks that use that channel.

For a Codex proxy, choose exactly one capability credential source:

```toml
[agent.codex.proxy]
ws_auth = "capability-token"
ws_token_pass = "agentix/codex/token"
```

`ws_token_pass`, `ws_token_file` and `ws_token_sha256` are mutually exclusive.
For `ws_auth = "signed-bearer-token"`, use `ws_shared_secret_pass` instead of
`ws_shared_secret_file`; the resolved secret must contain at least 32 bytes.
The corresponding `--ws-token-pass` and `--ws-shared-secret-pass` flags are also
available on `agentix serve`. CLI fields override the same TOML fields; switching
source types requires removing the other source from TOML.

Pass returns the credential on its first line. Successful resolutions are cached
in the process; restart Agentix after rotating a stored credential. Decryption
errors abort the operation without falling back or exposing pass output.

**Verify pinentry before using these fields.** Without working pinentry, remove
the pass field and use the existing direct credential setting instead. Protect
plaintext configuration with file permissions and keep it out of Git.

## Direct Memory reference

In `config.toml`:

```toml
[memory.providers.openai]
api = "responses"
base_url = "https://api.openai.com/v1"
api_key_pass = "taskix/openai"
```

No env entry is needed. `api_key_pass` and `api_key_env` are mutually exclusive.
Both model and embedding providers support this configuration.

## Environment reference

Alternatively, retain `api_key_env = "TASKIX_OPENAI_API_KEY"` in TOML and put this
in `taskix.env` or the inherited environment:

```dotenv
TASKIX_OPENAI_API_KEY_PASS=taskix/openai
```

The suffix applies to any variable named by `api_key_env`. A present direct value
wins; an explicitly empty value fails validation rather than invoking pass.
Without working pinentry, remove `_PASS` and set the actual API key instead.

## Plugin decisions

Plugins read `taskix.env` without parsing TOML:

```dotenv
TASKIX_DECISION_API_KEY_PASS=taskix/decision
```

Existing source precedence remains: current decision settings, legacy
`TASKIX_JEV_*` for Typesafe, then `OPENAI_*` only for built-in OpenAI Decisions.
Within each source a nonempty direct `API_KEY` wins over `API_KEY_PASS`.
Supported references: `TASKIX_DECISION_API_KEY_PASS`, `TASKIX_JEV_API_KEY_PASS`,
`OPENAI_API_KEY_PASS`. A selected pass failure stops resolution without trying
another credential. Without working pinentry, use `TASKIX_DECISION_API_KEY`.

## Runtime behavior

- Invoke `pass show -- ENTRY` without a shell; use only the first line.
- Entries are literal relative store paths, not shell expressions.
- Limit decryption to 120 seconds and output to 64 KiB.
- Reject missing commands, failed/cancelled decryption, invalid/empty output,
  oversized output and timeout without exposing output or child stderr.
- Cache successful keys per process. Restart services/plugin hosts after rotating
  a stored key; config reload may reuse the cached key.
- Keep decrypted keys in memory, without writing them to files, global or child
  environments, or shell exports. Only credential consumers receive them.
- `taskix doctor` does not decrypt. Pass availability is `null` with a deferred
  check; service status reports provider initialization failures.

`agentix env` and `taskix env` export literal references, never decrypted keys.
`_PASS` is interpreted by the credential consumers above, not as a general
mechanism for arbitrary settings.
