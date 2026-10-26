// Regenerate checked-in monitoring artifacts with: node monitoring/generate.mjs
import {mkdirSync, writeFileSync, rmSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';

const scope = 'instance=~"$instance"';
const ds = {type: 'prometheus', uid: '${DS_PROMETHEUS}'};
const select = (name, labels = '') => `${name}{${scope}${labels ? `,${labels}` : ''}}`;
const rate = (name, labels = '', by = 'instance,component,operation') => `sum by (${by}) (rate(${select(name, labels)}[$__rate_interval]))`;
const quantile = (service, operation, q) => `histogram_quantile(${q}, sum by (instance,component,operation,le) (rate(${select(`${service}_operation_duration_seconds_bucket`, operation)}[$__rate_interval])))`;
const panel = (title, expr, unit = 'short', description = '') => ({title, expr, unit, description});
const operationPanels = (service, filter) => [
  panel('Operations / second', rate(`${service}_operations_total`, filter), 'ops'),
  panel('Outcomes / second', rate(`${service}_operations_total`, filter, 'instance,component,operation,outcome'), 'ops'),
  panel('Operation latency p50', quantile(service, filter, 0.5), 's'),
  panel('Operation latency p95', quantile(service, filter, 0.95), 's'),
  panel('Operation latency p99', quantile(service, filter, 0.99), 's'),
  panel('Operations in flight', select(`${service}_operations_in_flight`, filter)),
];
const specs = [
  ['overview', 'Application overview', [
    panel('Agentix build', select('agentix_build_info'), 'short', 'A missing series means application data is unavailable; it does not identify a host or collector failure.'),
    panel('Taskix build', select('taskix_build_info')),
    panel('Agentix throughput', rate('agentix_operations_total'), 'ops'),
    panel('Taskix throughput', rate('taskix_operations_total'), 'ops'),
    panel('Agentix errors', rate('agentix_operations_total', 'outcome="error"'), 'ops'),
    panel('Taskix errors', rate('taskix_operations_total', 'outcome="error"'), 'ops'),
    panel('Queued application work', select('agentix_queue_items', 'state="pending"')),
    panel('Workflow states', select('taskix_entities')),
  ]],
  ['process', 'Process resources', ['agentix', 'taskix'].flatMap(service => [
    panel(`${service} CPU utilization`, `rate(${select(`${service}_process_cpu_seconds_total`)}[$__rate_interval])`, 'percentunit', '100% means one fully used CPU core; multithreaded usage can exceed 100%. Only this service process, excluding child agents.'),
    panel(`${service} resident memory`, select(`${service}_process_resident_memory_bytes`), 'bytes'),
    panel(`${service} virtual memory`, select(`${service}_process_virtual_memory_bytes`), 'bytes', 'Virtual address space, not physical RAM consumption.'),
    {...panel(`${service} start time`, `${select(`${service}_process_start_time_seconds`)} * 1000`, 'dateTimeAsIso'), type: 'stat'},
    {...panel(`${service} uptime`, select(`${service}_process_uptime_seconds`), 's'), type: 'stat'},
  ])],
  ['messages', 'Messages', [
    ...operationPanels('agentix', 'component=~"telegram|feishu|slack"'),
    panel('Inbound messages', rate('agentix_events_total', 'event="inbound"', 'instance,component'), 'ops'),
    panel('Rate-limit retries', rate('agentix_events_total', 'event="retry"', 'instance,component'), 'ops'),
    panel('Dispatch rejections', rate('agentix_events_total', 'event="rejected"', 'instance,component'), 'ops'),
  ]],
  ['agents', 'Agents and dispatch', [
    ...operationPanels('agentix', 'component=~"codex|pi|omp|claude|opencode|engine|control"'),
    panel('Live backend connections', select('agentix_backend_connections')),
    panel('First output p95', quantile('agentix', 'operation="first_output"', 0.95), 's', 'Measured once per observed turn. No per-fragment histogram updates.'),
    panel('First output p99', quantile('agentix', 'operation="first_output"', 0.99), 's', 'Measured once per observed turn. No per-fragment histogram updates.'),
    panel('Queue occupancy', select('agentix_queue_items')),
    panel('Queue capacity', select('agentix_queue_capacity')),
    panel('Oldest queued work', select('agentix_queue_oldest_age_seconds'), 's'),
    panel('Lost agent events', rate('agentix_events_total', 'event="event_lag"', 'instance,component'), 'ops'),
  ]],
  ['workflow', 'Workflow', [
    panel('Tasks by state', select('taskix_entities', 'kind="task"')),
    panel('Jobs by state', select('taskix_entities', 'kind="job"')),
    panel('Issues by state', select('taskix_entities', 'kind="issue"')),
    panel('Nonterminal entity age', select('taskix_oldest_age_seconds'), 's', 'Time since last entity update, not execution latency. Human waiting is not an incident by itself.'),
    panel('Observed transitions / second', rate('taskix_workflow_events_total', '', 'instance,event'), 'ops', 'Starts at exporter baseline; persisted history is never replayed as new activity.'),
    panel('Expired execution leases', select('taskix_expired_leases')),
    panel('Pending workflow documents', select('taskix_pending_documents')),
  ]],
  ['memory', 'Memory', [
    ...operationPanels('taskix', 'component=~"memory|model|embedding|provider"'),
    panel('Durable Memory work', select('taskix_memory_work_items')),
    panel('Ready work age', select('taskix_memory_oldest_pending_age_seconds'), 's'),
    panel('Retrying items', select('taskix_memory_retrying_items')),
    panel('Failed items', select('taskix_memory_failed_items')),
    panel('Pending document projection', select('taskix_memory_pending_projection')),
    panel('Pending embeddings', select('taskix_memory_pending_embeddings')),
    panel('Embedding failures', select('taskix_memory_embedding_failures')),
    panel('Memory lifecycle states', select('taskix_memories')),
    panel('Retrieval fallback and results', rate('taskix_events_total', 'component="memory"', 'instance,event'), 'ops'),
  ]],
  ['tokens', 'Observed token usage', [
    panel('Codex input tokens / second', rate('agentix_tokens_total', 'kind="input"', 'instance,source'), 'short', 'Codex cumulative notifications use a baseline; the first notification and reconnect history are not billed again.'),
    panel('Codex output tokens / second', rate('agentix_tokens_total', 'kind="output"', 'instance,source')),
    panel('Codex cached input / second', rate('agentix_tokens_total', 'kind="cached_input"', 'instance,source')),
    panel('Memory input tokens / second', rate('taskix_tokens_total', 'kind="input"', 'instance,source')),
    panel('Memory output tokens / second', rate('taskix_tokens_total', 'kind="output"', 'instance,source')),
    panel('Codex observed tokens in selected range', `sum by (instance,source,kind) (increase(${select('agentix_tokens_total')}[$__range]))`, 'short', 'Partial observed usage, not billing. Cached input is a subset of input. Missing usage stays missing. Never sum this with existing OTLP usage.'),
    panel('Memory observed tokens in selected range', `sum by (instance,source,kind) (increase(${select('taskix_tokens_total')}[$__range]))`),
  ]],
  ['jev', 'Jev decisions', [
    panel('Decisions / second', rate('taskix_jev_decisions_total', '', 'instance,kind,outcome'), 'ops'),
    panel('Adoption ratio', `(sum by (instance,kind) (rate(${select('taskix_jev_decisions_total','outcome="accepted"')}[$__rate_interval])) or (0 * sum by (instance,kind) (rate(${select('taskix_jev_decisions_total')}[$__rate_interval])))) / sum by (instance,kind) (rate(${select('taskix_jev_decisions_total')}[$__rate_interval]))`, 'percentunit', 'Adoption is not accuracy. Only requests persisted after the exporter baseline are counted.'),
    panel('Mean decision latency', `${rate('taskix_jev_duration_seconds_sum','','instance,kind')} / ${rate('taskix_jev_duration_seconds_count','','instance,kind')}`, 's', 'Mean of persisted request durations observed since the exporter baseline.'),
    ...[0.95, 0.99].map(q => panel('Decision latency p' + Math.round(q * 100), `histogram_quantile(${q}, sum by (instance,kind,le) (rate(${select('taskix_jev_duration_seconds_bucket')}[$__rate_interval])))`, 's', 'Estimated from fixed buckets for requests observed since the exporter baseline. Sparse traffic and wide buckets reduce tail precision.')),
    panel('Mean request confidence', `${rate('taskix_jev_request_confidence_sum','','instance,kind')} / ${rate('taskix_jev_request_confidence_count','','instance,kind')}`, 'percentunit', 'Mean of available per-request answer confidence, not calibrated accuracy.'),
    panel('Fallback reasons', rate('taskix_jev_fallbacks_total', '', 'instance,kind,reason'), 'ops'),
    panel('Human review counts', select('taskix_jev_reviewed'), 'short', 'Recomputed so late human reviews appear. Includes historical reviewed requests.'),
    panel('Reviewed accuracy', `(sum by (instance,kind) (${select('taskix_jev_reviewed','result="correct"')}) or (0 * sum by (instance,kind) (${select('taskix_jev_reviewed','result=~"correct|incorrect"')}))) / sum by (instance,kind) (${select('taskix_jev_reviewed','result=~"correct|incorrect"')})`, 'percentunit', 'Only the reviewed subset. No reviews means no data.'),
  ]],
];
let nextId = 1;
let rowY = 0;
const groupedPanels = specs.flatMap(([, title, panels]) => {
  const row = {id: nextId++, title, type: 'row', collapsed: false, panels: [], gridPos: {x: 0, y: rowY++, w: 24, h: 1}};
  const children = panels.map((p, index) => ({
    id: nextId++, title: p.title, description: p.description, type: p.type ?? 'timeseries', datasource: ds,
    gridPos: {x: index % 2 * 12, y: rowY + Math.floor(index / 2) * 8, w: 12, h: 8},
    targets: [{refId: 'A', expr: p.expr, legendFormat: '__auto', range: true, interval: '1m'}],
    fieldConfig: {defaults: {unit: p.unit, min: 0, noValue: 'No data', ...(p.type === 'stat' ? {} : {custom: {drawStyle: 'line', lineWidth: 1, fillOpacity: 8, spanNulls: false}})}, overrides: []},
    options: p.type === 'stat' ? {reduceOptions: {calcs: ['lastNotNull'], fields: '', values: false}, graphMode: 'none'} : {legend: {displayMode: 'table', placement: 'bottom', calcs: ['lastNotNull']}, tooltip: {mode: 'multi', sort: 'desc'}},
  }));
  rowY += Math.ceil(panels.length / 2) * 8;
  return [row, ...children];
});
export const dashboards = [{
  id: null, uid: 'agentix-applications', title: 'Agentix · Applications', tags: ['agentix', 'application'],
  schemaVersion: 39, version: 1, editable: true, timezone: 'browser', refresh: '1m',
  time: {from: 'now-6h', to: 'now'},
  __inputs: [{name: 'DS_CLOUD_METRICS', label: 'Grafana Cloud Metrics', type: 'datasource', pluginId: 'prometheus', pluginName: 'Prometheus'}],
  templating: {list: [
    {name: 'DS_PROMETHEUS', label: 'Metrics', type: 'datasource', query: 'prometheus', current: {text: 'Grafana Cloud Metrics', value: '${DS_CLOUD_METRICS}'}, hide: 0},
    {name: 'instance', label: 'Application instance', type: 'query', datasource: ds, query: 'label_values({__name__=~"agentix_build_info|taskix_build_info"}, instance)', refresh: 1, multi: true, includeAll: true, allValue: '.*', current: {text: 'All', value: '$__all'}},
  ]},
  links: [],
  panels: groupedPanels,
}];

export const allowlist = '(agentix|taskix)_(process_(cpu_seconds_total|resident_memory_bytes|virtual_memory_bytes|start_time_seconds|uptime_seconds)|build_info|backend_connections|operations_total|operation_duration_seconds_(bucket|sum|count)|operations_in_flight|events_total|queue_items|queue_capacity|queue_oldest_age_seconds|tokens_total)|taskix_(entities|oldest_age_seconds|expired_leases|pending_documents|workflow_events_total|memory_(work_items|oldest_pending_age_seconds|retrying_items|failed_items|pending_projection|pending_embeddings|embedding_failures)|memories|jev_(decisions_total|duration_seconds_(bucket|sum|count)|request_confidence_(sum|count)|fallbacks_total|reviewed))';
export const alloy = `// Append to the existing Alloy configuration. Keep its OTLP pipeline unchanged.
// Replace local with a stable application deployment alias on every machine.
prometheus.scrape "agentix_applications" {
  targets = [
    { "__address__" = "127.0.0.1:9464", "job" = "agentix", "instance" = "local" },
    { "__address__" = "127.0.0.1:9465", "job" = "taskix", "instance" = "local" },
  ]
  scrape_interval = "60s"
  scrape_timeout = "5s"
  scrape_protocols = ["OpenMetricsText1.0.0"]
  forward_to = [prometheus.relabel.agentix_applications.receiver]
}

// Keep after scrape: this also removes generated up and scrape_* samples.
prometheus.relabel "agentix_applications" {
  rule {
    source_labels = ["__name__"]
    regex = "${allowlist}"
    action = "keep"
  }
  forward_to = [prometheus.remote_write.default.receiver]
}
`;
const alert = (name, expr, duration, summary, description) => ({alert: name, expr, for: duration, labels: {severity: 'warning', service: 'agentix-applications'}, annotations: {summary, description}});
const alerts = [
  ...['agentix','taskix'].map(service => alert(`${service === 'agentix' ? 'Agentix' : 'Taskix'}ApplicationDataMissing`, `absent_over_time(${service}_build_info{job="${service}",instance="local"}[5m])`, '0m', `${service} application data missing`, 'No application build sample for five minutes. Check the application and delivery path locally; this does not diagnose a host or collector fault. Customize the instance selector per deployment.')),
  ...['agentix','taskix'].map(service => alert(`${service === 'agentix' ? 'Agentix' : 'Taskix'}ErrorRate`, `(sum by (instance,component,operation) (increase(${service}_operations_total{outcome="error"}[10m])) / sum by (instance,component,operation) (increase(${service}_operations_total[10m])) > 0.05) and (sum by (instance,component,operation) (increase(${service}_operations_total[10m])) >= 20)`, '5m', 'Application operation error rate above 5%', 'At least 20 operations in ten minutes. Inspect application logs locally; error text is not uploaded.')),
  alert('AgentixQueuePressure', 'sum by (instance,queue) (agentix_queue_items{state="pending"}) / sum by (instance,queue) (agentix_queue_capacity) > 0.8', '5m', 'Application queue above 80%', 'Pending work is approaching the admission capacity.'),
  alert('AgentixQueueNoProgress', '(agentix_queue_items{state="pending"} > 0) and on(instance,queue) (agentix_queue_oldest_age_seconds > 300) unless on(instance,queue) (label_replace(sum by (instance,component) (increase(agentix_operations_total{component=~"engine|control"}[5m])), "queue", "$1", "component", "(.*)") > 0)', '5m', 'Queued application work has no dispatch progress', 'Pending queue is older than five minutes without completed dispatch operations; this is not an agent turn duration alert.'),
  alert('TaskixExpiredLeases', 'taskix_expired_leases > 0', '5m', 'Task execution leases expired', 'Inspect Taskix task ownership and recovery; do not automatically approve or retry tasks.'),
  alert('TaskixProjectionBacklog', 'taskix_pending_documents > 0', '10m', 'Workflow document projection pending', 'Inspect Taskix document projection logs and vault availability.'),
  alert('MemoryFailedWork', 'taskix_memory_failed_items > 0', '5m', 'Memory work has failed', 'Inspect failed Memory work and provider configuration locally.'),
  alert('MemoryProjectionBacklog', 'taskix_memory_pending_projection > 0', '10m', 'Memory document projection pending', 'Inspect projection conflicts and repair logs locally.'),
  alert('MemoryEmbeddingFailures', 'taskix_memory_embedding_failures > 0', '10m', 'Memory embeddings need repair', 'Inspect provider availability and embedding configuration locally.'),
  alert('MemoryNoProgress', '(taskix_memory_oldest_pending_age_seconds{kind=~"extract|consolidate"} > 300) unless on(instance,kind) (label_replace(sum by (instance,operation) (increase(taskix_operations_total{component="memory",operation=~"extract|consolidate"}[5m])), "kind", "$1", "operation", "(.*)") > 0)', '5m', 'Ready Memory work has no progress', 'Ready work has not completed in five minutes. Future retry delays are excluded.'),
  alert('ApplicationInteractiveLatency', 'histogram_quantile(0.95, sum by (instance,component,operation,le) (rate(agentix_operation_duration_seconds_bucket{operation=~"rpc|send|update"}[10m]))) > 10', '10m', 'Interactive application operation p95 above ten seconds', 'Tune per operation. Agent turn and first-output durations are intentionally excluded.'),
  alert('MemoryProviderLatency', 'histogram_quantile(0.95, sum by (instance,component,operation,le) (rate(taskix_operation_duration_seconds_bucket{component=~"model|embedding",operation="request"}[10m]))) > 120', '10m', 'Memory provider p95 above two minutes', 'Provider requests have a separate latency threshold from interactive application operations.'),
];
export const rules = {groups: [{name: 'agentix-applications', interval: '1m', rules: alerts}]};
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  mkdirSync(new URL('./dashboards/', import.meta.url), {recursive: true});
  for (const [slug] of specs) rmSync(new URL(`./dashboards/agentix-${slug}.json`, import.meta.url), {force: true});
  for (const dashboard of dashboards) writeFileSync(new URL(`./dashboards/${dashboard.uid}.json`, import.meta.url), `${JSON.stringify(dashboard, null, 2)}\n`);
  // JSON is a YAML subset; keeping one serializer prevents quoting drift in PromQL.
  writeFileSync(new URL('./alerts.yaml', import.meta.url), `${JSON.stringify(rules, null, 2)}\n`);
  writeFileSync(new URL('./alloy-applications.alloy', import.meta.url), alloy);
}
