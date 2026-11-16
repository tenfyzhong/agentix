import test from 'node:test';
import { parseEnv } from 'node:util';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { taskEnvironment, parseEnvironment } from '../environment.mjs';

test('environment syntax is literal and rejects malformed lines without exposing secrets', () => {
    assert.deepEqual({...parseEnvironment('# comment\r\nA=one=two\nB="$HOME $(secret)"\nC=\'a\\b\'\nEMPTY=\n')}, {A:'one=two', B:'$HOME $(secret)', C:'a\\b', EMPTY:''});
    for (const input of ['BAD-KEY=secret', 'A=secret\0']) {
        assert.throws(() => parseEnvironment(input), error => !error.message.includes('secret'));
    }
});

test('file values are cached per path, inherited values override without mutating host', t => {
    const dir = mkdtempSync(join(tmpdir(), 'taskix-env-'));
    t.after(() => rmSync(dir, {recursive:true, force:true}));
    const env = {TASKIX_CONFIG: join(dir, 'custom.toml'), TASKIX_LANG: ''};
    writeFileSync(join(dir, 'taskix.env'), 'TASKIX_LANG=zh-CN\nTASKIX_MEMORY_ENABLED=true\n');
    assert.equal(taskEnvironment(env).TASKIX_LANG, '');
    assert.equal(taskEnvironment(env).TASKIX_MEMORY_ENABLED, 'true');
    writeFileSync(join(dir, 'taskix.env'), 'TASKIX_MEMORY_ENABLED=false\n');
    assert.equal(taskEnvironment(env).TASKIX_MEMORY_ENABLED, 'true');
    assert.equal(env.TASKIX_MEMORY_ENABLED, undefined);
});

test('Agentix bridge loads its own file independently', async t => {
    const dir = mkdtempSync(join(tmpdir(), 'agentix-env-'));
    t.after(() => rmSync(dir, {recursive:true, force:true}));
    const { agentEnvironment } = await import('../../agentix-bridge/environment.mjs');
    writeFileSync(join(dir, 'agentix.env'), 'AGENTIX_CONTROL_ENDPOINT=tcp://127.0.0.1:7654\n');
    assert.equal(agentEnvironment({AGENTIX_CONFIG:join(dir,'config.toml')}).AGENTIX_CONTROL_ENDPOINT, 'tcp://127.0.0.1:7654');
});

test('shared Rust and plugin syntax fixtures and generated copies agree', () => {
    const cases = JSON.parse(readFileSync(new URL('../../shared/environment-fixtures.json', import.meta.url), 'utf8'));
    for (const item of cases.valid) {
        assert.deepEqual({...parseEnv(item.source)}, item.values);
        assert.deepEqual({...parseEnvironment(item.source)}, item.values);
    }
    for (const source of cases.invalid) assert.throws(() => parseEnvironment(source));
    const source = readFileSync(new URL('../../shared/environment.mjs', import.meta.url), 'utf8');
    for (const host of ['taskix-manager','agentix-bridge']) {
        assert.equal(readFileSync(new URL(`../../${host}/environment.mjs`, import.meta.url), 'utf8'), source);
    }
});

test('hook language and decision defaults use local file without exporting to host', async t => {
    const dir = mkdtempSync(join(tmpdir(), 'taskix-hook-env-'));
    const keys = ['TASKIX_CONFIG','TASKIX_LANG','TASKIX_DECISION_ENABLED'];
    const previous = Object.fromEntries(keys.map(key => [key, process.env[key]]));
    t.after(() => {
        for (const [key,value] of Object.entries(previous)) {
            if (value === undefined) delete process.env[key]; else process.env[key] = value;
        }
        rmSync(dir, {recursive:true, force:true});
    });
    process.env.TASKIX_CONFIG = join(dir, 'config.toml');
    delete process.env.TASKIX_LANG;
    delete process.env.TASKIX_DECISION_ENABLED;
    writeFileSync(join(dir, 'taskix.env'), 'TASKIX_LANG=ja\nTASKIX_DECISION_ENABLED=false\n');
    const { runHook } = await import('./support/runtime.mjs');
    const result = await runHook({hook_event_name:'SessionStart',session_id:'env-test',cwd:dir}, async () => ({result:{}}));
    assert.match(result.hookSpecificOutput.additionalContext, /"task_language":"ja"/);
    assert.equal(process.env.TASKIX_LANG, undefined);
    const { decisionConfig } = await import('../decision-provider.mjs');
    assert.equal(decisionConfig(), undefined);
});

test('Taskix subprocess receives file defaults from explicit config', {skip:process.platform==='win32'}, async t => {
    const dir = mkdtempSync(join(tmpdir(), 'taskix-child-env-'));
    const previousPath = process.env.PATH;
    t.after(() => { process.env.PATH = previousPath; rmSync(dir,{recursive:true,force:true}); });
    process.env.PATH = dir;
    writeFileSync(join(dir, 'taskix'), '#!/bin/sh\nprintf \'{"schema_version":1,"ok":true,"result":"%s"}\\n\' "$LOCAL_ENV_CHILD_TEST"\n', {mode:0o700});
    writeFileSync(join(dir, 'taskix.env'), 'LOCAL_ENV_CHILD_TEST=from-file\n');
    const { runTaskix } = await import('../taskix-cli.mjs');
    assert.equal((await runTaskix(['--config',join(dir,'config.toml'),'context'])).result, 'from-file');
    assert.equal((await runTaskix(['--config','config.toml','context'], {cwd:dir})).result, 'from-file');
    assert.equal(process.env.LOCAL_ENV_CHILD_TEST, undefined);
});
