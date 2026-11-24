import test from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, symlinkSync, mkdtempSync, writeFileSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { decisionConfig } from '../decision-provider.mjs';
import { taskEnvironment } from '../environment.mjs';
import { runHook } from '../runtime.mjs';

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
test('maintenance hooks leave the decision credential unread', async t => {
    const env = fixture(t, `
        import { writeFileSync } from 'node:fs';
        import { join } from 'node:path';
        writeFileSync(join(process.env.HOME, 'pass-ran'), '');
        writeSync(1, 'test-key\\n');
        process.exit(0);
    `);
    const calls = [];
    const runner = async args => {
        calls.push(args.join(' '));
        return {result: {}};
    };
    for (const hook_event_name of ['SessionStart', 'PreToolUse', 'PostToolUse', 'Stop', 'Interrupt', 'SessionEnd']) {
        await runHook({hook_event_name, session_id: 'maintenance'}, runner, {env, cacheDir: env.HOME});
    }
    assert.ok(calls.includes('hook session-end'));
    assert.equal(existsSync(join(env.HOME, 'pass-ran')), false);
});

function running(pid) {
    try {
        const status = execFileSync('ps', ['-o', 'stat=', '-p', String(pid)], {encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore']});
        return !status.trim().startsWith('Z');
    } catch (error) {
        if (error.status === 1) return false;
        throw error;
    }
}

for (const trigger of ['timeout', 'caller exit']) {
    test(`pass and its subprocess are stopped after ${trigger}`, {skip: process.platform === 'win32'}, async t => {
        const env = fixture(t, `
            import { spawn } from 'node:child_process';
            import { writeFileSync } from 'node:fs';
            import { join } from 'node:path';
            const child = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 15_000)'], {env: {}, stdio: 'ignore'});
            writeFileSync(join(process.env.HOME, 'pids'), JSON.stringify([process.pid, child.pid]));
            await new Promise(resolve => setTimeout(resolve, 15_000));
        `);
        const host = spawn(process.execPath, ['--input-type=module', '-e', `
            import { passKey } from ${JSON.stringify(new URL('../pass.mjs', import.meta.url).href)};
            try { passKey('slow', ${JSON.stringify(env)}, ${trigger === 'timeout' ? 1500 : 10_000}); }
            catch (error) { if (!/timed out/.test(error.message)) throw error; }
        `], {stdio: 'ignore'});
        const exited = once(host, 'exit');
        const pidFile = join(env.HOME, 'pids');
        let pids = [];
        t.after(() => {
            host.kill('SIGKILL');
            for (const pid of pids) {
                if (running(pid)) process.kill(pid, 'SIGKILL');
            }
        });
        for (let retry = 0; retry < 100 && !existsSync(pidFile); retry++) await delay(20);
        pids = JSON.parse(readFileSync(pidFile, 'utf8'));
        if (trigger === 'caller exit') host.kill('SIGKILL');
        assert.deepEqual(await exited, trigger === 'timeout' ? [0, null] : [null, 'SIGKILL']);
        for (let retry = 0; retry < 40 && pids.some(running); retry++) await delay(25);
        assert.deepEqual(pids.filter(running), []);
    });
}
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
