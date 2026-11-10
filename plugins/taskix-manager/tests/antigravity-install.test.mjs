import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm, cp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { installNativePlugins, removeNativePlugins } from '../../../scripts/antigravity-plugin.mjs';
const repository = fileURLToPath(new URL('../../../', import.meta.url));

for (const fail of [false, true]) test(`native_install_${fail ? 'rolls_back_partial_failure' : 'updates_only_owned_plugins'}`, async t => {
    const root = await mkdtemp(join(tmpdir(), 'agy-installer-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const installed = new Map(), calls = [];
    for (const name of ['taskix-manager', 'agentix-bridge', 'unrelated']) {
        const path = join(root, name); await mkdir(path);
        await writeFile(join(path, 'old'), name);
        installed.set(name, { name, path, enabled: name !== 'taskix-manager' });
    }
    let failed = false;
    const run = async (_binary, args) => {
        calls.push(args);
        if (args[1] === 'list') return JSON.stringify([...installed.values()]);
        if (args[1] === 'install') {
            const name = basename(args[2]), path = join(root, name);
            await rm(path, { recursive: true, force: true }); await cp(args[2], path, { recursive: true });
            installed.set(name, { name, path, enabled: true });
            if (fail && name === 'agentix-bridge' && !failed) { failed = true; throw new Error('install failure'); }
        }
        if (args[1] === 'disable') installed.get(args[2]).enabled = false;
        if (args[1] === 'uninstall') { await rm(installed.get(args[2]).path, { recursive: true }); installed.delete(args[2]); }
        return '';
    };
    if (fail) {
        await assert.rejects(installNativePlugins({ repository, run }), /install failure/);
        for (const name of ['taskix-manager', 'agentix-bridge']) assert.equal(await readFile(join(root, name, 'old'), 'utf8'), name);
    } else {
        await installNativePlugins({ repository, run });
        for (const name of ['taskix-manager', 'agentix-bridge']) await readFile(join(root, name, 'plugin.json'));
        assert.equal(calls.filter(args => args[1] === 'validate').length, 2);
    }
    assert.equal(installed.get('taskix-manager').enabled, false);
    assert.equal(await readFile(join(root, 'unrelated/old'), 'utf8'), 'unrelated');
    await removeNativePlugins({ run });
    await removeNativePlugins({ run });
    assert.deepEqual([...installed.keys()], ['unrelated']);
});

test('remote_clone_failure_never_changes_installed_plugins', async () => {
    const calls = [];
    await assert.rejects(installNativePlugins({ branch: 'feature/agy', run: async (binary, args) => {
        calls.push({ binary, args }); throw new Error('clone failed');
    } }), /clone failed/);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].binary, 'git');
    assert.deepEqual(calls[0].args.slice(0, 8), ['clone', '--depth', '1', '--single-branch', '--branch', 'feature/agy', '--', 'https://github.com/tenfyzhong/agentix.git']);
});
