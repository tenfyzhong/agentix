import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const plugin = fileURLToPath(new URL('../', import.meta.url));
const manifest = JSON.parse(await readFile(new URL('../hooks.json', import.meta.url)));
const events = ['SessionStart', 'PreInvocation', 'PostInvocation', 'PreToolUse', 'PostToolUse', 'Stop'];

async function fixture(t) {
    const root = await mkdtemp(join(tmpdir(), 'taskix-app-hooks-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const bin = join(root, 'bin');
    await mkdir(bin);
    await writeFile(join(bin, 'taskix'), await readFile(new URL('./fixtures/antigravity-taskix-cli.mjs', import.meta.url)), { mode: 0o700 });
    const transcriptPath = join(root, 'transcript.jsonl');
    await writeFile(transcriptPath, '');
    // App 2.31.2 bundled hook contract: common fields have no hookEventName.
    const payload = { conversationId: 'app-fixture', workspacePaths: [root], transcriptPath,
        toolCall: { name: 'view_file', args: { AbsolutePath: join(root, 'README.md') } }, fullyIdle: true };
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, TASKIX_BIN: join(bin, 'taskix'),
        TASKIX_TEST_LOG: join(root, 'calls.jsonl'), TASKIX_ANTIGRAVITY_DATA_DIR: join(root, 'identity'),
        XDG_STATE_HOME: join(root, 'state'), TASKIX_DECISION_ENABLED: 'false', TASKIX_JEV_ENABLED: 'false' };
    async function run(command, input = payload) {
        const child = spawn(command, { cwd: plugin, env, shell: true, stdio: ['pipe', 'pipe', 'pipe'] });
        let stdout = '', stderr = '';
        child.stdout.on('data', data => stdout += data);
        child.stderr.on('data', data => stderr += data);
        const done = new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
        child.stdin.end(JSON.stringify(input));
        return { code: await done, stdout, stderr };
    }
    return { root, payload, run };
}

for (const host of ['app', 'cli']) {
    test(`${host}_manifest_commands_accept_all_native_events`, { skip: process.platform === 'win32', timeout: 20000 }, async t => {
        const f = await fixture(t);
        for (const event of events) {
            const group = manifest['taskix-manager'][event][0];
            const command = (group.hooks?.[0] ?? group).command;
            const result = await f.run(command, { ...f.payload, ...(host === 'cli' ? { hookEventName: event } : {}) });
            assert.equal(result.code, 0, `${event}: ${result.stderr}`);
            const output = JSON.parse(result.stdout);
            if (event === 'PreToolUse') assert.equal(output.decision, 'allow');
            if (event === 'SessionStart') assert.match(output.injectSteps[0].ephemeralMessage, /Task session: app-fixture/);
        }
    });
}

test('native_entrypoint_rejects_conflicting_event_sources', { skip: process.platform === 'win32' }, async t => {
    const f = await fixture(t);
    const result = await f.run('node antigravity/hook.mjs PreToolUse', { ...f.payload, hookEventName: 'Stop' });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /Conflicting Antigravity hook event/);
});

test('native_entrypoint_logs_explicit_event_on_invalid_identity', { skip: process.platform === 'win32' }, async t => {
    const f = await fixture(t);
    const result = await f.run('node antigravity/hook.mjs PreToolUse', { ...f.payload, workspacePaths: [] });
    assert.equal(result.code, 1);
    assert.match(result.stderr, /Invalid Antigravity hook identity/);
    const log = JSON.parse((await readFile(join(f.root, 'state/taskix/hooks.jsonl'), 'utf8')).trim());
    assert.equal(log.event, 'PreToolUse');
});
