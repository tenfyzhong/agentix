# Decision providers and protocols

Taskix uses an optional decision service for prompt routing, Issue association,
discussion ownership, lifecycle assessment and memory extraction screening.
The shared host runtime and the Rust memory service support the same configuration.
Provider identity, protocol and model are independent: Typesafe is a provider,
`typesafe` is its protocol, and Jev is its model family.

## Configuration

Set variables in the environment of each host and the Taskix service. Restart
running processes after changing their environment; `taskix reload` reloads the
configuration file, not the process environment.

| Variable | Behavior |
| --- | --- |
| `TASKIX_DECISION_ENABLED` | `true` or `1` (case-insensitive) enables calls; default off. |
| `TASKIX_DECISION_PROVIDER` | Provider identity, default `typesafe`. Built-ins are `typesafe` and `openai`; custom names match `[a-z][a-z0-9_-]{0,63}`. |
| `TASKIX_DECISION_PROTOCOL` | `typesafe` or `openai_decisions`. Defaults from the built-in provider; required for third parties. Selects serialization, independently of provider identity. |
| `TASKIX_DECISION_URL` | Full HTTP(S) endpoint without embedded credentials. Defaults to `https://api.openai.com/v1/decisions` only for `openai` with `openai_decisions`. |
| `TASKIX_DECISION_API_KEY` | Bearer credential. Only the matching OpenAI built-in may fall back to `OPENAI_API_KEY`. |
| `TASKIX_DECISION_MODEL` | Defaults to `jev-latest` for Typesafe with its protocol, or `gpt-6-luna` for OpenAI with its protocol. |
| `TASKIX_DECISION_MIN_CONFIDENCE` | Routing/lifecycle/discussion threshold, default `0.65`, range `0.5..1`. |
| `TASKIX_MEMORY_DECISION_MIN_CONFIDENCE` | Independent memory screening threshold, default `0.75`, range `0.5..1`; never inherits the routing threshold. |
| `TASKIX_DECISION_METRICS_ENABLED` | Opt-in persistence of decision observations; default off. Does not enable decisions or the HTTP metrics endpoint. |
| `TASKIX_DECISION_METRICS_DB` | Independent metrics SQLite path shared by writers and reports. |

Third parties must supply protocol, endpoint, key and model explicitly. An
explicit protocol overrides a built-in default; mismatched provider/protocol
pairs do not inherit connection/model defaults or credentials. There is no
automatic cross-provider retry or switching.

```fish
# Typesafe / Jev
set -gx TASKIX_DECISION_ENABLED true
set -gx TASKIX_DECISION_PROVIDER typesafe
set -gx TASKIX_DECISION_PROTOCOL typesafe
set -gx TASKIX_DECISION_URL https://api.typesafe.ai/v1/systemone
set -gx TASKIX_DECISION_API_KEY YOUR_API_KEY
set -gx TASKIX_DECISION_MODEL jev-latest
```

```fish
# OpenAI Decisions; URL and model have built-in defaults.
set -gx TASKIX_DECISION_ENABLED true
set -gx TASKIX_DECISION_PROVIDER openai
set -gx TASKIX_DECISION_PROTOCOL openai_decisions
set -gx TASKIX_DECISION_API_KEY YOUR_OPENAI_API_KEY
```

```fish
# A third party reusing the OpenAI Decisions protocol.
set -gx TASKIX_DECISION_ENABLED true
set -gx TASKIX_DECISION_PROVIDER acme
set -gx TASKIX_DECISION_PROTOCOL openai_decisions
set -gx TASKIX_DECISION_URL https://api.example.com/v1/decisions
set -gx TASKIX_DECISION_API_KEY YOUR_API_KEY
set -gx TASKIX_DECISION_MODEL acme-decision-model
```

## Existing installations

For `typesafe` with the `typesafe` protocol, corresponding `TASKIX_JEV_*`
variables remain fallbacks for enablement, URL, key, model and routing threshold.
`TASKIX_MEMORY_JEV_MIN_CONFIDENCE` remains the legacy memory threshold. New
nonblank connection values take priority. An explicitly set new enable switch,
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

Both adapters use the existing choice questions and bounded state. Typesafe
keeps its `state` and keyed `questions` format. OpenAI uses `input_json` and a
named question array; structured choice descriptions become JSON strings.
Answers are matched by name and normalized before the same validation gates.
Duplicate answer names or probability keys, refusals, missing answers and invalid
distributions cannot become accepted decisions. Only applicable questions gate
the action, as before.

Confidence and selected probability must both meet the threshold, with a margin
of at least `0.2` over the runner-up. These defaults are starting points, not
cross-provider calibration. Historical Jev benchmarks apply only to the tested
Typesafe model and configuration.

The existing eight-second deadline, 1 MiB response cap and actual serialized
request caps (30,000 UTF-8 bytes for routing/triage, 24,000 for discussion) remain.
OpenAI permits up to 400 questions and 255 choices per question; unsupported
requests defer rather than dropping candidates. Errors or uncertainty hand task
decisions back to the main Agent. Memory screening only suppresses extraction on
a confident `skip`; other outcomes retain extraction. All lifecycle mutations
still require Taskix revision, lease and state guards.

This integration uses `choice`, not general object generation, images, predicates
or scoring. See the [OpenAI Decisions API](https://developers.openai.com/api/docs/guides/decisions)
for the upstream protocol.

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
