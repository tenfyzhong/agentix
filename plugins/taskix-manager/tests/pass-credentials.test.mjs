import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { decisionConfig } from '../decision-provider.mjs';
import { taskEnvironment } from '../environment.mjs';

function fixture(t, body) {
    const dir = mkdtempSync(join(tmpdir(), 'taskix-pass-'));
    t.after(() => rmSync(dir, {recursive:true, force:true}));
    writeFileSync(join(dir, 'pass'), '#!/bin/sh\n'+body, {mode:0o700});
    return {TASKIX_CONFIG:join(dir,'config.toml'), PATH:dir, HOME:dir,
        TASKIX_DECISION_ENABLED:'true', TASKIX_DECISION_PROVIDER:'openai',
        TASKIX_DECISION_API_KEY_PASS:'folder/key with spaces'};
}
test('decision pass is lazy, uses literal arguments, caches and does not mutate environment', t => {
    const env=fixture(t, '[ "$1" = show ] && [ "$2" = -- ] && [ "$3" = "folder/key with spaces" ] || exit 3\necho call >> "$HOME/count"\nprintf "test-key\\r\\nmetadata\\n"\n');
    assert.equal(taskEnvironment(env).TASKIX_DECISION_API_KEY, undefined);
    assert.equal(decisionConfig({...env,TASKIX_DECISION_ENABLED:'false'}), undefined);
    assert.equal(decisionConfig(env).key, 'test-key');
    assert.equal(decisionConfig(env).key, 'test-key');
    assert.equal(readFileSync(join(env.HOME,'count'),'utf8'),'call\n');
    assert.equal(env.TASKIX_DECISION_API_KEY, undefined);
});
test('direct key takes precedence without running pass', t => {
    const env=fixture(t, 'exit 1\n');
    assert.equal(decisionConfig({...env,TASKIX_DECISION_API_KEY:'direct'}).key,'direct');
});
for (const [mode,body] of Object.entries({failure:'echo secret-output >&2; exit 1',empty:'printf "\\nsecret-output"',invalid:'printf "secret-output\\001"',large:'/usr/bin/head -c 70000 /dev/zero'})) {
    test(`pass ${mode} fails without exposing output or falling back`, t => {
        const env=fixture(t,body+'\n');
        assert.throws(()=>decisionConfig({...env,OPENAI_API_KEY:'fallback'}), error => /pass/.test(error.message) && !error.message.includes('secret-output') && !error.cause);
    });
}
test('pass timeout is bounded and sanitized', async t => {
    const { passKey } = await import('../pass.mjs');
    const env=fixture(t, 'exec /bin/sleep 10\n');
    const start=Date.now();
    assert.throws(()=>passKey('timeout',env,30), /timed out/);
    assert.ok(Date.now()-start < 2000);
});
test('file pass references resolve only when decisions are requested', t => {
    const env=fixture(t,'printf "file-key\\n"\n');
    delete env.TASKIX_DECISION_API_KEY_PASS;
    writeFileSync(join(env.HOME,'taskix.env'),'TASKIX_DECISION_API_KEY_PASS=folder/key\n');
    const loaded=taskEnvironment(env);
    assert.equal(loaded.TASKIX_DECISION_API_KEY,undefined);
    assert.equal(decisionConfig(loaded).key,'file-key');
});
test('invalid pass entry names are rejected before executing the command', async t => {
    const { passKey } = await import('../pass.mjs');
    const env=fixture(t,'exit 99\n');
    for (const entry of ['', ' ', '-c', '/absolute', '../outside', 'folder/../key', 'key\nother']) {
        assert.throws(()=>passKey(entry,env),/Invalid pass entry/);
    }
});
