import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync, readdirSync} from 'node:fs';
import {dashboards, rules, allowlist, alloy} from '../generate.mjs';

test('one dashboard preserves sections as rows and includes process resources', () => {
  assert.equal(dashboards.length, 1);
  assert.deepEqual(readdirSync(new URL('../dashboards/', import.meta.url)).filter(name => name.endsWith('.json')), ['agentix-applications.json']);
  assert.equal(dashboards[0].panels.filter(panel => panel.type === 'row').length, 8);
  assert.equal(dashboards[0].panels.filter(panel => panel.type !== 'row').length, 78);
  assert.equal(new Set(dashboards[0].panels.map(panel => panel.id)).size, dashboards[0].panels.length);
  const uids = new Set();
  for (const dashboard of dashboards) {
    assert.ok(dashboard.panels.length >= 5);
    assert.ok(!uids.has(dashboard.uid));
    uids.add(dashboard.uid);
    assert.ok(dashboard.templating.list.some(v => v.name === 'instance'));
    assert.equal(dashboard.__inputs[0].name, 'DS_CLOUD_METRICS');
    assert.equal(dashboard.panels.find(panel => panel.type !== 'row').datasource.uid, '${DS_PROMETHEUS}');
    for (const panel of dashboard.panels) {
      for (const target of panel.targets ?? []) {
        assert.equal(target.interval, '1m', 'rate windows must match the 60-second scrape cadence');
        assert.match(target.expr, /(?:agentix|taskix)_/);
        assert.match(target.expr, /instance=~"\$instance"/);
        assert.doesNotMatch(target.expr, /\b(?:up|scrape_\w+|process_\w+|node_\w+|alloy_\w+)\b|or vector\(0\)/);
      }
    }
    assert.deepEqual(JSON.parse(readFileSync(new URL(`../dashboards/${dashboard.uid}.json`, import.meta.url))), dashboard);
  }
});
test('exact allowlist rejects infrastructure and arbitrary application names', () => {
  const keep = new RegExp(`^(?:${allowlist})$`);
  for (const name of ['agentix_process_cpu_seconds_total','taskix_process_resident_memory_bytes','agentix_process_start_time_seconds','taskix_process_uptime_seconds','taskix_process_virtual_memory_bytes','agentix_build_info','taskix_entities','taskix_memory_pending_embeddings','agentix_operation_duration_seconds_bucket']) assert.ok(keep.test(name), name);
  for (const name of ['up','scrape_duration_seconds','node_cpu_seconds_total','process_resident_memory_bytes','alloy_build_info','prometheus_remote_storage_samples_total','agentix_secret','taskix_collector_errors_total']) assert.ok(!keep.test(name), name);
  assert.match(alloy, /prometheus\.relabel\.agentix_applications\.receiver/);
  assert.match(alloy, /action\s*=\s*"keep"/);
});
test('alerts retain volume guards and do not impose agent or human wait deadlines', () => {
  const all = rules.groups.flatMap(group => group.rules);
  assert.ok(all.length >= 8);
  assert.ok(all.some(rule => rule.expr.includes('absent_over_time')));
  for (const rule of all) {
    assert.doesNotMatch(rule.expr, /\bup\b|scrape_|process_|node_/);
    if (rule.alert.endsWith('ErrorRate')) assert.match(rule.expr, />= 20/);
    if (rule.alert.endsWith('Latency')) assert.doesNotMatch(rule.expr, /operation="turn"|waiting_user/);
  }
});

test('every p95 has a matching p99 and Jev uses allowed histogram buckets', () => {
  const panels = dashboards[0].panels;
  let row = [];
  const check = () => {
    for (const p of row.filter(p => p.title.endsWith('p95'))) {
      const partner = row.find(q => q.title === p.title.replace(/p95$/, 'p99'));
      assert.ok(partner, p.title);
      assert.equal(partner.targets[0].expr, p.targets[0].expr.replace('0.95', '0.99'));
    }
  };
  for (const p of panels) {
    if (p.type === 'row') { check(); row = []; } else row.push(p);
  }
  check();
  for (const q of ['95', '99']) {
    const p = panels.find(p => p.title === 'Decision latency p' + q);
    assert.ok(p);
    assert.match(p.targets[0].expr, /taskix_jev_duration_seconds_bucket/);
  }
  assert.ok(new RegExp('^(?:' + allowlist + ')$').test('taskix_jev_duration_seconds_bucket'));
});
