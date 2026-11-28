# Decision providers and protocols

Taskix uses an optional decision service for prompt routing, Issue association,
discussion ownership and lifecycle assessment. The host plugin performs these calls.
Provider identity, protocol and model are independent: Typesafe is a provider,
`typesafe` is its protocol, and Jev is its model family.

## Configuration

Set Decision variables in the environment of each host. Restart the host after changing its environment.

| Variable | Behavior |
| --- | --- |
| `TASKIX_DECISION_ENABLED` | `true` or `1` (case-insensitive) enables calls; default off. |
| `TASKIX_DECISION_PROVIDER` | Provider identity, default `typesafe`. Built-ins are `typesafe`, `openai` and `google`; custom names match `[a-z][a-z0-9_-]{0,63}`. |
| `TASKIX_DECISION_PROTOCOL` | `typesafe`, `openai_decisions` or `gemini`. Defaults from the built-in provider; required for third parties. Selects serialization, independently of provider identity. |
| `TASKIX_DECISION_URL` | Full HTTP(S) endpoint without embedded credentials. OpenAI defaults to `https://api.openai.com/v1/decisions`. Google defaults to `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`. Defaults apply to matching built-in pairs. |
| `TASKIX_DECISION_API_KEY` | Key sent as `x-goog-api-key` for Gemini or as a Bearer credential for the other protocols. The matching Google and OpenAI built-ins can fall back to `GEMINI_API_KEY` and `OPENAI_API_KEY`, respectively. |
| `TASKIX_DECISION_MODEL` | Defaults for matching built-in pairs are `jev-latest`, `gpt-6-luna` and `gemini-3.8-flash`, respectively. For a custom Gemini endpoint, use a URL that selects the configured model. |
| `TASKIX_DECISION_MIN_CONFIDENCE` | Routing/lifecycle/discussion threshold, default `0.65`, range `0.5..1`. |
| `TASKIX_DECISION_METRICS_ENABLED` | Opt-in persistence of decision observations; default off. Does not enable decisions or the HTTP metrics endpoint. |
| `TASKIX_DECISION_METRICS_DB` | Independent metrics SQLite path shared by writers and reports. |

Third parties must supply protocol, endpoint, key and model explicitly. An
explicit protocol overrides a built-in default; mismatched provider/protocol
pairs do not inherit connection/model defaults or credentials. There is no
automatic cross-provider retry or switching.

```fish
# Typesafe / Jev
set -Ux TASKIX_DECISION_ENABLED true
set -Ux TASKIX_DECISION_PROVIDER typesafe
set -Ux TASKIX_DECISION_PROTOCOL typesafe
set -Ux TASKIX_DECISION_URL https://api.typesafe.ai/v1/systemone
set -Ux TASKIX_DECISION_API_KEY YOUR_API_KEY
set -Ux TASKIX_DECISION_MODEL jev-latest
```

```fish
# OpenAI Decisions; URL and model have built-in defaults.
set -Ux TASKIX_DECISION_ENABLED true
set -Ux TASKIX_DECISION_PROVIDER openai
set -Ux TASKIX_DECISION_PROTOCOL openai_decisions
set -Ux TASKIX_DECISION_API_KEY YOUR_OPENAI_API_KEY
```

For Google Gemini, add these entries to `taskix.env` and restart the host.
Clear any previous Decision URL, protocol or model overrides to use Google's
built-in defaults. The model must support JSON structured output.

```dotenv
TASKIX_DECISION_ENABLED=true
TASKIX_DECISION_PROVIDER=google
TASKIX_DECISION_API_KEY=YOUR_GEMINI_API_KEY
```

```fish
# A third party reusing the OpenAI Decisions protocol.
set -Ux TASKIX_DECISION_ENABLED true
set -Ux TASKIX_DECISION_PROVIDER acme
set -Ux TASKIX_DECISION_PROTOCOL openai_decisions
set -Ux TASKIX_DECISION_URL https://api.example.com/v1/decisions
set -Ux TASKIX_DECISION_API_KEY YOUR_API_KEY
set -Ux TASKIX_DECISION_MODEL acme-decision-model
```

## Existing installations

For `typesafe` with the `typesafe` protocol, corresponding `TASKIX_JEV_*`
variables remain fallbacks for enablement, URL, key, model and routing threshold.
Nonblank connection values under the new names take priority. An explicitly set new enable switch,
including `false` or blank, overrides the old switch. Legacy credentials are
never inherited by another provider or protocol.

Metrics aliases are provider-independent: new `TASKIX_DECISION_METRICS_*`
variables take priority over `TASKIX_JEV_METRICS_*`. The default path is
`$XDG_STATE_HOME/taskix/decision-metrics.sqlite`, or
`~/.local/state/taskix/decision-metrics.sqlite`.
Existing `jev-metrics.sqlite` files are not automatically moved or selected.
To keep using historical observations, explicitly set `TASKIX_DECISION_METRICS_DB`
to the old file path. The task database is unchanged.

## Decisions and failure behavior

All three adapters use the existing choice questions and bounded state. Typesafe
keeps its `state` and keyed `questions` format. OpenAI uses `input_json` and a
named question array. Gemini passes the named questions to the native
`generateContent` API and requests JSON structured output. Gemini confidence and
probabilities are estimates produced by the model.
Answers are matched by name and normalized before the same validation gates.
Duplicate answer names or probability keys, refusals, missing answers and invalid
distributions cannot become accepted decisions. Only applicable questions gate
the action, as before.

Confidence and selected probability must both meet the threshold, with a margin
of at least `0.2` over the runner-up. These defaults are starting points, not
cross-provider calibration. Historical Jev benchmarks apply only to the tested
Typesafe model and configuration.

The existing eight-second deadline, 1 MiB response cap and actual serialized
request caps (30,000 UTF-8 bytes for routing, 24,000 for discussion) remain.
The OpenAI and Gemini adapters allow up to 400 questions and 255 choices per
question. Oversized requests, errors and uncertainty hand decisions back to the
main Agent. All lifecycle mutations require Taskix revision, lease and state
guards.

All Decisions requests honor the existing `https_proxy` / `HTTPS_PROXY` (HTTPS
endpoint), `http_proxy` / `HTTP_PROXY` (HTTP endpoint), and `no_proxy` /
`NO_PROXY` settings from `taskix.env` or the host environment. Configured
proxies must use HTTP or HTTPS. This requires Node 22.21+ on the 22.x line
or Node 24.5+. Hosts without supported proxy handling defer decisions
when a proxy is configured.

The adapters handle choice questions. See the upstream
[OpenAI Decisions API](https://developers.openai.com/api/docs/guides/decisions)
and [Gemini GenerateContent API](https://ai.google.dev/api/generate-content).

## Observability and upgrades

`taskix routing metrics report`, `report --details`, `report --json`, `list` and
`label` retain their commands. JSON adds `by_provider` and `by_provider_kind`;
request lists include provider and protocol. The compact report stays grouped by
kind; `--details` shows provider/protocol breakdowns. Observations contain scores,
timing and identifiers, not prompt text, credentials, URLs or raw responses.

Upgrade the plugin, CLI and memory service together. Metrics v3 adds `provider`
and `protocol`; transactional writers migrate recognized v1/v2 databases and
preserve answers and human labels. Old rows become `typesafe` / `typesafe`.
Readers support v1/v2/v3 without migrating on reads. Older binaries reject v3;
do not rewrite version metadata to downgrade a database.

The exporter publishes `taskix_decision_*` with provider/protocol labels. The old
`taskix_jev_*` series remain Typesafe/Typesafe-only compatibility output. Use one
metric family per query to avoid double counting. The repository dashboard uses
the new family and retains existing panel positions. Update its generated Alloy
allowlist together with the dashboard. Export retains at most 32 custom
provider/protocol pairs, in addition to built-in identities; overflow or invalid
identities aggregate into `other` / `other`. CLI/SQLite retain the exact identity.

Collection remains best-effort and default-off. The exporter does not replay
historical counters on startup, and HTTP scrapes do not query SQLite. Local mock
and interoperability tests establish protocol behavior, not live model quality,
account access or remote Grafana deployment.

Store these variables in `taskix.env` beside the Taskix configuration. The CLI
and plugin load it automatically; restart resident hosts after edits. See
[Local environment files](environment-export.md).

## Optional pass credentials

Memory accepts `api_key_pass` directly, or `api_key_env` with a matching `_PASS`
entry in `taskix.env`. Plugins accept `TASKIX_DECISION_API_KEY_PASS`. The Google
built-in also accepts `GEMINI_API_KEY_PASS`. Configure GPG/pinentry for the process
user first, or configure a direct API key.
See [pass setup and precedence](https://github.com/tenfyzhong/agentix/wiki/Pass-Secrets).
