# Dashboard panel guide

> Unreleased: this guide describes the dashboard and collector in the application-metrics feature build.

This guide covers all **78 panels in eight rows** of **Agentix · Applications**,
including the matching P99 panels and Jev P95/P99. Titles below exactly match the
dashboard. It explains the configured queries; it is not a live health assessment.

## How to read the dashboard

- Select the Prometheus data source and the application instance. The default instance is `local`; it is a deployment alias, not a hostname.
- Rows and queries cover application behavior and the Agentix/Taskix service processes. Host resources, Alloy resources and child-Agent resource consumption are excluded.
- `/ second` means a window-averaged rate, not a percentage: 0.1/s is about six observations per minute. `$__rate_interval` controls rate windows, while `$__range` follows the selected dashboard range.
- State counts and queue sizes are gauges of current stored state. They may include old records. Zooming into the last hour does not make them counts of new events in that hour.
- P50, P95 and P99 are histogram estimates, not exact order statistics. P99 is especially sensitive to low request volume and bucket width. Compare quantiles with operation counts, and do not infer an incident from one isolated spike.
- `No data` is not zero. A feature may be unused or disabled, a series may not yet exist, a rate may lack enough samples, or the reporting path may be unavailable. Zero observations also leave ratios and quantiles undefined.
- Scrapes run every 60 seconds; database snapshots also refresh periodically. Expect sampling delay. Workflow and Memory state cover the instance's shared databases across projects; there is no project filter.
- Different operations can describe nested stages of the same request. Adding all operation rates or in-flight gauges does not yield a unique request count.
- Token usage is partial observed usage, not billing. Agent sources cover Codex, Pi, OMP, Claude and OpenCode; Memory stays separate. Native input/output semantics differ by host. Codex cached input is a subset of its input; other hosts may report cache reads separately. Do not combine these series with an OTLP pipeline covering the same requests.
- The process start-time and uptime stat panels display a value from the selected range. When investigating current health, include the present time rather than an old historical range.

## Application overview

### Agentix build

Build-information series for Agentix. Its value is 1; the version is a label, not the numeric value. Use it to see whether application data is present and which build is reporting. Missing data alone cannot distinguish an application outage from a delivery-path failure.

### Taskix build

Build-information series for Taskix, with value 1 and a version label. It is not a process count. A version change can produce a new series; missing data does not identify which part of the reporting path failed.

### Agentix throughput

Completed Agentix observations per second, grouped by instance, component and operation. This includes different stages such as dispatch, RPC and turns. Do not sum stages and call the result a count of user requests.

### Taskix throughput

Completed observations in the Taskix service per second, grouped by component and operation. These include instrumented Memory and provider activity. This is not a count of every standalone Taskix CLI invocation.

### Agentix errors

Agentix observations labeled outcome="error" per second. This is an error frequency, not an error percentage. Separately labeled timeout and cancelled outcomes are excluded; some adapter/provider failures, including timeouts, are recorded as error.

### Taskix errors

Taskix observations labeled outcome="error" per second. Use this for recent operational errors, not historical failed Memory work. Separately labeled timeout and cancelled outcomes are excluded. Divide by the matching total-operation rate to calculate an error ratio.

### Queued application work

Current pending work in Agentix application queues. Short spikes can be normal bursts. A backlog that keeps growing or fails to drain deserves investigation together with queue age and capacity.

### Workflow states

Current stored Task, Job and Issue counts by kind and state across the instance. Completed and cancelled records can include history. Changing the dashboard time range does not turn these counts into newly created records.

## Process resources

### agentix CPU utilization

Agentix service-process CPU time converted to a usage rate. 100% means one fully occupied core; multithreaded usage can exceed 100%. Child Agents and host-wide CPU are excluded. Interpret high CPU together with workload, latency and backlog.

### agentix resident memory

Current resident memory (RSS) of the Agentix service process in bytes. Watch sustained trends and whether usage stabilizes after load. A transient rise alone does not establish a memory leak.

### agentix virtual memory

Virtual address-space size of the Agentix service process in bytes. This is not physical RAM consumption; a large value alone does not indicate memory pressure.

### agentix start time

Start timestamp of the current Agentix process, converted to a date/time for display. A new timestamp indicates a different process lifetime; this is not the time metrics were enabled.

### agentix uptime

Elapsed lifetime of the current Agentix service process, displayed as a duration. A restart resets it. It does not measure total service availability across restarts.

### taskix CPU utilization

Taskix service-process CPU time converted to a usage rate. 100% means one fully occupied core; multithreaded usage can exceed 100%. Child Agents and host-wide CPU are excluded. Interpret high CPU together with workload, latency and backlog.

### taskix resident memory

Current resident memory (RSS) of the Taskix service process in bytes. Watch sustained trends and whether usage stabilizes after load. A transient rise alone does not establish a memory leak.

### taskix virtual memory

Virtual address-space size of the Taskix service process in bytes. This is not physical RAM consumption; a large value alone does not indicate memory pressure.

### taskix start time

Start timestamp of the current Taskix process, converted to a date/time for display. A new timestamp indicates a different process lifetime; this is not the time metrics were enabled.

### taskix uptime

Elapsed lifetime of the current Taskix service process, displayed as a duration. A restart resets it. It does not measure total service availability across restarts.

## Messages

### Operations / second

Completed messaging send/update operations in Telegram, Feishu and Slack per second, grouped by component and operation. Read each operation separately: nested stages are not independent user requests and should not be summed into a user-request total.

### Outcomes / second

Completed messaging send/update operations in Telegram, Feishu and Slack per second, additionally split into success, error, cancelled and timeout. Compare outcomes for the same operation. Cancellation is not automatically a fault, and failure classification depends on the instrumentation boundary.

### Operation latency p50

Estimated P50 duration of completed messaging send/update operations in Telegram, Feishu and Slack, grouped by component and operation. Approximately 50% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operation latency p95

Estimated P95 duration of completed messaging send/update operations in Telegram, Feishu and Slack, grouped by component and operation. Approximately 95% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operation latency p99

Estimated P99 duration of completed messaging send/update operations in Telegram, Feishu and Slack, grouped by component and operation. Approximately 99% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operations in flight

Currently unfinished messaging send/update operations in Telegram, Feishu and Slack, grouped by component and operation. Nested operations may be present at the same time. Sustained occupancy plus increasing latency/backlog warrants investigation; long-running turns can be legitimate.

### Inbound messages

Inbound messages observed by the application per second, grouped by messaging component. This measures input traffic, not successful replies or completed Agent turns.

### Rate-limit retries

Observed messaging rate-limit retries per second by component. A retry is not necessarily a final failure. Sustained growth can indicate excessive send/update frequency or upstream throttling.

### Dispatch rejections

Engine admission rejections per second when the conversation capacity limit is reached. This indicates overload rejection, not a model refusing an answer.

## Agents and dispatch

### Operations / second

Completed backend RPC, turn, first-output and engine/control dispatch observations per second, grouped by component and operation. Read each operation separately: nested stages are not independent user requests and should not be summed into a user-request total.

### Outcomes / second

Completed backend RPC, turn, first-output and engine/control dispatch observations per second, additionally split into success, error, cancelled and timeout. Compare outcomes for the same operation. Cancellation is not automatically a fault, and failure classification depends on the instrumentation boundary.

### Operation latency p50

Estimated P50 duration of completed backend RPC, turn, first-output and engine/control dispatch observations, grouped by component and operation. Approximately 50% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operation latency p95

Estimated P95 duration of completed backend RPC, turn, first-output and engine/control dispatch observations, grouped by component and operation. Approximately 95% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operation latency p99

Estimated P99 duration of completed backend RPC, turn, first-output and engine/control dispatch observations, grouped by component and operation. Approximately 99% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operations in flight

Currently unfinished backend RPC, turn, first-output and engine/control dispatch observations, grouped by component and operation. Nested operations may be present at the same time. Sustained occupancy plus increasing latency/backlog warrants investigation; long-running turns can be legitimate.

### Live backend connections

Current live backend transport connections, grouped by backend. Connections may be idle and concurrent bridge peers can contribute separately. This is not a task, user or active-turn count.

### First output p95

Estimated P95 time from the observed start of a turn to its first observed output, grouped by backend component and operation. One observation is made per turn, not per streaming fragment. This is not strict model time-to-first-token or end-to-end user-response latency; pre-turn queueing is outside this interval. Read with turn volume and queue age.

### First output p99

Estimated P99 time from the observed start of a turn to its first observed output, grouped by backend component and operation. One observation is made per turn, not per streaming fragment. This is not strict model time-to-first-token or end-to-end user-response latency; pre-turn queueing is outside this interval. Read with turn volume and queue age.

### Queue occupancy

Current pending and active work by application queue and state. Read the state label: only pending work is waiting for admission/processing. Compare pending occupancy with the same queue’s pending capacity.

### Queue capacity

Configured pending admission capacity for each application queue. This is a limit, not a measurement of throughput. Compare it with pending occupancy rather than the sum of pending and active work.

### Oldest queued work

Age in seconds of the oldest pending application work. An age that rises continuously is stronger evidence of waiting than a brief queue-length spike. It is not Agent execution duration.

### Lost agent events

Agent events lost due to a lagging event consumer, expressed as events per second. It counts the number of lost events, not just the number of lag incidents. Repeated loss can make runtime observations incomplete.

## Workflow

### Tasks by state

Stored Task counts by state, across all projects in this instance. DONE and other historical records remain part of the population. These are current state counts, not transitions during the selected time range.

### Jobs by state

Stored Job counts by state. PENDING_REVIEW normally means awaiting delivery acceptance, not execution failure. Historical completed Jobs can dominate the total without indicating current load.

### Issues by state

Stored Issue counts by state. Use this to understand the requirement backlog and review stages; it does not measure Agent utilization or recent issue creation.

### Nonterminal entity age

Time since the oldest last update among nonterminal entities, grouped by kind and state. This is not execution latency. Human waiting and pending review can legitimately age without a service incident.

### Observed transitions / second

New persisted workflow events observed after the collector baseline, grouped by event. Inspect the event label for the specific activity. Startup history is not replayed as new traffic; collection is periodic, so bursts can appear delayed.

### Expired execution leases

Number of persisted execution leases whose expiry time has passed. A persistent nonzero count warrants checking executor exits, missing heartbeats and recovery. It is not a historical counter of all lease expirations.

### Pending workflow documents

Workflow documents currently waiting for projection into the document workspace. Brief nonzero values can be normal synchronization delay. A sustained backlog means database changes are not fully reflected in documents.

## Memory

### Operations / second

Completed Memory API, retrieval, extraction, consolidation, projection, model, embedding and provider-wait operations per second, grouped by component and operation. Read each operation separately: nested stages are not independent user requests and should not be summed into a user-request total.

### Outcomes / second

Completed Memory API, retrieval, extraction, consolidation, projection, model, embedding and provider-wait operations per second, additionally split into success, error, cancelled and timeout. Compare outcomes for the same operation. Cancellation is not automatically a fault, and failure classification depends on the instrumentation boundary.

### Operation latency p50

Estimated P50 duration of completed Memory API, retrieval, extraction, consolidation, projection, model, embedding and provider-wait operations, grouped by component and operation. Approximately 50% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operation latency p95

Estimated P95 duration of completed Memory API, retrieval, extraction, consolidation, projection, model, embedding and provider-wait operations, grouped by component and operation. Approximately 95% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operation latency p99

Estimated P99 duration of completed Memory API, retrieval, extraction, consolidation, projection, model, embedding and provider-wait operations, grouped by component and operation. Approximately 99% of observed durations fall below this value. Compare like operations rather than RPCs with full turns or model calls. The query combines outcomes; it is not success-only latency. No observations leave the quantile undefined, and low volume makes tail estimates unstable.

### Operations in flight

Currently unfinished Memory API, retrieval, extraction, consolidation, projection, model, embedding and provider-wait operations, grouped by component and operation. Nested operations may be present at the same time. Sustained occupancy plus increasing latency/backlog warrants investigation; long-running turns can be legitimate.

### Durable Memory work

Persisted Memory work items by kind (extract or consolidate) and state. Pending/running show unfinished work; done, failed and cancelled can include retained history. These are work items, not stored memory entries.

### Ready work age

Age of the oldest pending Memory item whose available_at time has arrived. Future retry delays are excluded. Rising age indicates ready work is not being serviced promptly; it does not measure the full lifetime of an item.

### Retrying items

Memory items currently pending with at least one previous attempt. This is a current retry backlog, not the cumulative number of retry attempts. Terminal failed items are excluded.

### Failed items

Retained Memory work items currently in failed state. This is a gauge of failed work, not a recent failure rate or a metrics-upload error count. Old failures survive process restarts; a high flat value can be historical backlog. Check operation outcomes and item errors before retrying or cleaning up.

### Pending document projection

Memory entries whose latest revision has not been published to the document workspace. Persistent growth indicates projection lag. This is separate from pending workflow documents.

### Pending embeddings

Active, unexpired memories missing a vector for the configured embedding generation and current memory revision. Memory edits or embedding-generation changes can temporarily raise this count. A backlog can reduce semantic-search coverage.

### Embedding failures

Persisted embedding failure records awaiting repair. These are separate from extract/consolidate work-item failures and are not safely added together as a count of unique failed tasks.

### Memory lifecycle states

Stored memory entries grouped by lifecycle status. This is the memory population, not the background-work queue. One work item can produce multiple memories, so these counts need not match Durable Memory work.

### Retrieval fallback and results

Rates of Memory retrieval events. fallback counts queries using fts_fallback or offline_fts; results counts returned memory entries, not queries. Neither series is a retrieval accuracy or hit-rate percentage.

## Observed token usage

### Agent input tokens / second

Observed input-token deltas per second for all five agents, with `source · instance` legends. Sources are `codex_app_server`, `pi_bridge`, `omp_bridge`, `claude_bridge`, and `opencode_bridge`. This is not current context length. The initial cumulative notification establishes a baseline, so the initial turn may be undercounted.

### Agent output tokens / second

Observed output-token deltas per second for all five agents, distinguished by source and instance. Usage notifications can arrive in batches, so spikes do not represent instantaneous token-generation speed.

### Agent cached tokens / second

Observed cache-read token deltas per second for all five agents, distinguished by source and instance. Missing native usage remains absent. Codex includes cached input in its input count; other hosts can report cache reads separately. Cache writes are not included.

### Memory input tokens / second

Input tokens reported by actual Memory model responses per second, grouped by source. Missing usage fields stay missing; this is not an estimate of all provider consumption.

### Memory output tokens / second

Output tokens reported by actual Memory model responses per second. It covers observed usage only and does not establish billing cost or complete provider accounting.

### Agent observed tokens in selected range

increase(agentix_tokens_total[$__range]), grouped by source and token kind. The rightmost point estimates the increment over the selected range. Because this is a time-series panel, earlier points use the same-length rolling window ending at each point; never sum the plotted points. Extrapolation can produce fractional values.

### Memory observed tokens in selected range

increase(taskix_tokens_total[$__range]), grouped by source and token kind. Read the rightmost point for the selected-range increment. Earlier points are rolling windows, not independent subtotals, and must not be summed. Only reported usage is included.

## Jev decisions

### Decisions / second

New persisted Jev requests observed after the collector baseline per second, grouped by decision kind and accepted/fallback outcome. Historical requests are not replayed on startup. Disabled or unused decision paths may have no new data.

### Adoption ratio

Accepted decision rate divided by all observed decision rates for each kind. It measures automatic adoption, not correctness. With no new requests the denominator is zero and the ratio is undefined, not evidence of 0% or 100% quality.

### Mean decision latency

Observed duration sum divided by duration count for each Jev kind, using rates over the query window. This is an arithmetic mean of persisted request durations, not a percentile. Only requests observed after the baseline contribute.

### Decision latency p95

Estimated P95 of persisted Jev request durations observed since the baseline, grouped by kind. Uses rates of cumulative duration buckets, not quantiles of averages. Needs the updated Taskix collector and Alloy bucket allowlist. Sparse traffic and wide tail buckets reduce precision; values above 3600 seconds cannot be resolved by the finite buckets. Compare with decision volume and the mean.

### Decision latency p99

Estimated P99 of persisted Jev request durations observed since the baseline, grouped by kind. Uses rates of cumulative duration buckets, not quantiles of averages. Needs the updated Taskix collector and Alloy bucket allowlist. Sparse traffic and wide tail buckets reduce precision; values above 3600 seconds cannot be resolved by the finite buckets. Compare with decision volume and the mean.

### Mean request confidence

Mean of the available per-request confidence values, grouped by Jev kind. Each request contributes its available average answer confidence. This is reported confidence, not calibrated accuracy; requests without confidence are excluded.

### Fallback reasons

Jev fallback events per second by decision kind and bounded reason. Use reason labels to distinguish low confidence, uncertainty, timeout, provider errors, disabled configuration and other causes. A fallback can be intentional and is not automatically a service failure.

### Human review counts

Current human-reviewed request counts by kind and result, including historical requests and reviews added later. This is not the number of Jobs currently waiting for review.

### Reviewed accuracy

Correct reviews divided by correct plus incorrect reviews, per decision kind. Other review outcomes are excluded. It describes only the reviewed subset, not all requests; no qualifying reviews means no meaningful accuracy value.
