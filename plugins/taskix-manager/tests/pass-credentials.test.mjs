import test from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, symlinkSync, mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { decisionConfig } from '../decision-provider.mjs';
import { taskEnvironment } from '../environment.mjs';

function fixture(t, body) {
    const dir = mkdtempSync(join(tmpdir(), 'taskix-pass-'));
    t.after(() => rmSync(dir, {recursive:true, force:true}));
    // A native executable exercises execFileSync on Windows without a shell.
    if (process.platform === 'win32') copyFileSync(process.execPath, join(dir, 'pass.exe'));
    else symlinkSync(process.execPath, join(dir, 'pass'));
    const script = join(dir, 'pass-fixture.mjs');
    writeFileSync(script, "import { writeSync } from 'node:fs';\n" + body);
    return {TASKIX_CONFIG:join(dir,'config.toml'), PATH:dir, HOME:dir,
        NODE_OPTIONS:`--import=${pathToFileURL(script).href}`,
        TASKIX_DECISION_ENABLED:'true', TASKIX_DECISION_PROVIDER:'openai',
        TASKIX_DECISION_API_KEY_PASS:'folder/key with spaces'};
}
test('decision pass is lazy, uses literal arguments, caches and does not mutate environment', t => {
    const env=fixture(t, `
        import assert from 'node:assert/strict';
        import { appendFileSync } from 'node:fs';
        import { basename, join } from 'node:path';
        assert.equal(basename(process.argv[1]), 'show');
        assert.deepEqual(process.argv.slice(2), ['--', 'folder/key with spaces']);
        appendFileSync(join(process.env.HOME, 'count'), 'call\\n');
        writeSync(1, 'test-key\\r\\nmetadata\\n');
        process.exit(0);
    `);
    assert.equal(taskEnvironment(env).TASKIX_DECISION_API_KEY, undefined);
    assert.equal(decisionConfig({...env,TASKIX_DECISION_ENABLED:'false'}), undefined);
    assert.equal(decisionConfig(env).key, 'test-key');
    assert.equal(decisionConfig(env).key, 'test-key');
    assert.equal(readFileSync(join(env.HOME,'count'),'utf8'),'call\n');
    assert.equal(env.TASKIX_DECISION_API_KEY, undefined);
});
test('direct key takes precedence without running pass', t => {
    const env=fixture(t, 'process.exit(1);');
    assert.equal(decisionConfig({...env,TASKIX_DECISION_API_KEY:'direct'}).key,'direct');
});
for (const [mode,body] of Object.entries({
    failure:'process.stderr.write("secret-output"); process.exit(1);',
    empty:'writeSync(1, "\\nsecret-output");',
    invalid:'writeSync(1, "secret-output\\x01");',
    large:'writeSync(1, "x".repeat(70000));',
})) {
    test(`pass ${mode} fails without exposing output or falling back`, t => {
        const env=fixture(t,body+'\nprocess.exit(0);');
        assert.throws(()=>decisionConfig({...env,OPENAI_API_KEY:'fallback'}), error => (['empty', 'invalid'].includes(mode) ? /invalid API key/ : /decryption failed/).test(error.message) && !error.message.includes('secret-output') && !error.cause);
    });
}
test('pass timeout is bounded and sanitized', async t => {
    const { passKey } = await import('../pass.mjs');
    const env=fixture(t, 'await new Promise(resolve => setTimeout(resolve, 10_000));');
    const start=Date.now();
    assert.throws(()=>passKey('timeout',env,30), /timed out/);
    assert.ok(Date.now()-start < 2000);
});
test('file pass references resolve only when decisions are requested', t => {
    const env=fixture(t,'writeSync(1, "file-key\\n"); process.exit(0);');
    delete env.TASKIX_DECISION_API_KEY_PASS;
    writeFileSync(join(env.HOME,'taskix.env'),'TASKIX_DECISION_API_KEY_PASS=folder/key\n');
    const loaded=taskEnvironment(env);
    assert.equal(loaded.TASKIX_DECISION_API_KEY,undefined);
    assert.equal(decisionConfig(loaded).key,'file-key');
});
test('invalid pass entry names are rejected before executing the command', async t => {
    const { passKey } = await import('../pass.mjs');
    const env=fixture(t,'process.exit(99);');
    for (const entry of ['', ' ', '-c', '/absolute', '../outside', 'folder/../key', 'key\nother']) {
        assert.throws(()=>passKey(entry,env),/Invalid pass entry/);
    }
});
