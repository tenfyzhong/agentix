import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {spawn, spawnSync} from 'node:child_process';
import {mkdtempSync, writeFileSync, rmSync, readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {alloy, dashboards, allowlist} from '../generate.mjs';

const enabled = process.env.ALLOY_BINARY && process.env.PROMETHEUS_BINARY;
const listen = server => new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve(server.address().port)));
async function port() { const server = createServer(); const value = await listen(server); await new Promise(resolve => server.close(resolve)); return value; }

test('real Alloy remote-write sends application samples and drops all other series', {skip: !enabled, timeout: 60_000}, async t => {
  const dir = mkdtempSync(join(tmpdir(), 'agentix-pipeline-'));
  const children = [];
  const logs = [];
  const payload = process.env.METRICS_FIXTURE_FILE ? readFileSync(process.env.METRICS_FIXTURE_FILE, 'utf8').replace('# EOF\n', '') : '# TYPE agentix_build info\nagentix_build_info{version="test"} 1\n';
  const fixture = createServer((_req, response) => {
    response.setHeader('content-type', 'application/openmetrics-text; version=1.0.0; charset=utf-8');
    response.end(payload + '# TYPE agentix_secret gauge\nagentix_secret 1\n# TYPE node_cpu_seconds counter\nnode_cpu_seconds_total 99\n# TYPE process_resident_memory_bytes gauge\nprocess_resident_memory_bytes 999\n# EOF\n');
  });
  const sourcePort = await listen(fixture);
  t.after(async () => {
    for (const child of children) child.kill('SIGTERM');
    await Promise.all(children.map(child => child.exitCode === null ? new Promise(resolve => child.once('exit', resolve)) : Promise.resolve()));
    await new Promise(resolve => fixture.close(resolve));
    rmSync(dir, {recursive: true, force: true});
  });
  const receiverPort = await port();
  const adminPort = await port();
  const prometheusConfig = join(dir, 'prometheus.json');
  writeFileSync(prometheusConfig, JSON.stringify({scrape_configs: []}));
  const start = (binary, args) => {
    const child = spawn(binary, args, {stdio: ['ignore', 'pipe', 'pipe']});
    child.stdout.on('data', data => logs.push(data.toString()));
    child.stderr.on('data', data => logs.push(data.toString()));
    children.push(child);
  };
  start(process.env.PROMETHEUS_BINARY, [`--config.file=${prometheusConfig}`, `--storage.tsdb.path=${join(dir, 'tsdb')}`, `--web.listen-address=127.0.0.1:${receiverPort}`, '--web.enable-remote-write-receiver']);
  const config = alloy.replaceAll('127.0.0.1:9464', `127.0.0.1:${sourcePort}`).replaceAll('127.0.0.1:9465', `127.0.0.1:${sourcePort}`).replace('"60s"', '"1s"').replace('"5s"', '"1s"') + `\nprometheus.remote_write "default" {\n  endpoint {\n    url = "http://127.0.0.1:${receiverPort}/api/v1/write"\n    queue_config {\n      batch_send_deadline = "1s"\n    }\n  }\n}\n`;
  const configPath = join(dir, 'config.alloy');
  writeFileSync(configPath, config);
  const validation = spawnSync(process.env.ALLOY_BINARY, ['validate', configPath], {encoding: 'utf8'});
  assert.equal(validation.status, 0, validation.stderr);
  start(process.env.ALLOY_BINARY, ['run', '--disable-reporting', `--server.http.listen-addr=127.0.0.1:${adminPort}`, `--storage.path=${join(dir, 'alloy')}`, configPath]);
  const query = async expr => (await fetch(`http://127.0.0.1:${receiverPort}/api/v1/query?query=${encodeURIComponent(expr)}`)).json();
  let result;
  for (let attempt = 0; attempt < 80; attempt++) {
    await delay(500);
    try { result = await query(payload.includes('taskix_build_info') ? 'taskix_build_info' : 'agentix_build_info'); if (result.data?.result?.length === 2) break; } catch {}
  }
  assert.equal(result?.data?.result?.length, 2, logs.join('').slice(-8000));
  await delay(1500);
  const all = await query('{__name__=~".+"}');
  const keep = new RegExp(`^(?:${allowlist})$`);
  const names = [...new Set(all.data.result.map(series => series.metric.__name__))];
  assert.ok(names.every(name => keep.test(name)), names.join(','));
  assert.ok(!names.includes('agentix_secret'));
  if (process.env.METRICS_FIXTURE_FILE) {
    assert.ok(names.includes('taskix_entities'));
    assert.ok(names.includes('taskix_process_resident_memory_bytes'));
    assert.ok(names.includes('taskix_process_cpu_seconds_total'));
    assert.ok(names.includes('taskix_process_start_time_seconds'));
  }
  if (payload.includes('taskix_jev_duration_seconds_bucket')) {
    assert.ok(names.includes('taskix_jev_duration_seconds_bucket'));
    const result = await query('histogram_quantile(0.95, sum by (kind,le) (taskix_jev_duration_seconds_bucket))');
    assert.ok(result.data.result.length > 0);
    assert.ok(result.data.result.every(series => Number.isFinite(Number(series.value[1]))));
  }
  // Ask the actual PromQL engine to parse and evaluate every dashboard expression.
  for (const dashboard of dashboards) for (const panel of dashboard.panels) for (const target of panel.targets ?? []) {
    const expression = target.expr.replaceAll('$instance', '.*').replaceAll('$__rate_interval', '5m').replaceAll('$__range', '1h');
    const response = await query(expression);
    assert.equal(response.status, 'success', `${panel.title}: ${response.error}`);
  }
});
