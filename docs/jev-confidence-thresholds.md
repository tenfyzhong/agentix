# Jev confidence threshold guide

We recommend setting `TASKIX_JEV_MIN_CONFIDENCE=0.65` as a starting point for
Jev routing with the context improvements described in
[Task routing performance](task-routing-performance.md). It balances acceptance
and the errors flagged in the historical review below. The built-in default
is `0.65` when the variable is unset or blank. An explicit value overrides it.

## Configuration

For a host launched from fish, set the threshold before starting the host:

```fish
set -gx TASKIX_JEV_MIN_CONFIDENCE 0.65
```

To retain this setting, add it to the fish startup configuration used to launch
the host. An already running host must be restarted with the updated environment;
setting a variable in another terminal does not update that process. For a host
launched by a service, configure the service's environment instead. Jev must also
be enabled with its endpoint and credentials as described in the
[plugin configuration](../plugins/taskix-manager/README.md#optional-jev-routing).

The valid threshold range is `0.5` through `1`. Both the provider's confidence
and the selected-option probability must meet the threshold. The winning
probability must also exceed the runner-up by at least `0.2`. Uncertain results,
incomplete context, stale Job revisions, and other validation failures still
defer to the main Agent. Lowering the threshold does not bypass these checks.

## Threshold comparison

The 2026-09-22 evaluation used 838 historical requests from 131 Agentix Jobs
with the final context implementation. The `0.50` results came from a provider
replay; higher thresholds were applied offline to the same responses. They are
not separate provider runs. All 838 requests, including fallbacks, remain in
the acceptance-rate denominator.

The main Agent screened accepted results and reviewed suspected errors against
their conversation evidence. These are **provisional labels awaiting human
review**, not an independently verified accuracy benchmark.

| Threshold | Accepted / total | Acceptance rate | Reviewer-flagged errors | Flagged errors / accepted | Unresolved accepted results |
| --- | ---: | ---: | ---: | ---: | ---: |
| `0.50` | 712 / 838 | 84.96% | 15 | 2.11% | 27 |
| **`0.65` (recommended default)** | **667 / 838** | **79.59%** | **9** | **1.35%** | **17** |
| `0.80` | 585 / 838 | 69.81% | 2 | 0.34% | 8 |
| `0.90` | 475 / 838 | 56.68% | 0 | None flagged; not proof of zero errors | 6 |

Acceptance means the router accepted a result, not that its decision was correct.
The error fraction counts only errors found under the current review labels;
unresolved results are not counted as correct. Human review may revise either
classification. Do not subtract these fractions from 100% to report accuracy.

## Why start with 0.65?

Lowering the threshold from `0.65` to `0.50` accepted 45 additional requests,
raising acceptance by 5.37 percentage points. Of those additional results,
6 were flagged as errors (13.33%) and 10 remained unresolved. The extra
acceptance therefore came with a higher concentration of suspected errors.

At `0.65`, acceptance was 79.59%, just below the 80% target and below the 90%
stretch target. We recommend improving context and resolving the observed
routing mistakes before lowering the threshold further to meet those targets.
Flagged mistakes included selecting the wrong PR's Job, losing an existing Job
owner, and treating execution approval as discussion.

Choose `0.80` or `0.90` when you prefer more main-Agent fallbacks in
exchange for fewer flagged errors in this sample. Neither setting guarantees
correct routing. Review representative conversations from your own workflow
before changing the recommendation.

## Interpretation limits

The corpus was used for context tuning and includes repeated events. At `0.50`,
the 712 accepted rows represent 518 groups when deduplicated by exact session,
message time, and prompt. Rows are therefore not independent observations.
Historical reconstruction also cannot recover every Inbox or lease state.

These results support a configuration recommendation for the evaluated context
implementation. They do not establish a production error rate, latency benefit,
or behavior after a provider model update. The evaluation itself did not change
the deployment; a subsequent configuration change made `0.65` the built-in
default. Existing explicit overrides remain effective. For replay methodology and performance boundaries,
see [Task routing performance](task-routing-performance.md#historical-replay-result).
