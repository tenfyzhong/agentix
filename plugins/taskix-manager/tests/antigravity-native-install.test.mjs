import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildNativePlugins, pluginNames, installNativePlugins, removeNativePlugins } from '../../../scripts/antigravity-plugin.mjs';
const run = promisify(execFile);

test('native_plugin_install_update_remove_preserves_unrelated_plugins', {
    skip: process.env.AGENTIX_TEST_ANTIGRAVITY_INSTALL !== '1', timeout: 60000,
}, async t => {
    const root = await mkdtemp(join(tmpdir(), 'agy-install-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const home = join(root, 'home'), packages = join(root, 'packages'), other = join(root, 'other');
    await mkdir(home); await mkdir(other);
    await writeFile(join(other, 'plugin.json'), JSON.stringify({ name: 'unrelated-fixture', version: '1.0.0', description: 'Isolated test fixture' }));
    await buildNativePlugins(fileURLToPath(new URL('../../../', import.meta.url)), packages);
    const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: join(root, 'config'), XDG_CACHE_HOME: join(root, 'cache') };
    const agy = async args => (await run(process.env.AGENTIX_TEST_AGY ?? 'agy', ['plugin', ...args], { env, timeout: 15000 })).stdout;
    await agy(['install', other]);
    for (const name of pluginNames) {
        await agy(['validate', '--strict', join(packages, name)]);
        await agy(['install', join(packages, name)]);
        await agy(['install', join(packages, name)]);
    }
    const installed = JSON.parse(await agy(['list', '--json']));
    t.diagnostic(JSON.stringify(installed));
    for (const name of [...pluginNames, 'unrelated-fixture']) assert.ok(JSON.stringify(installed).includes(name));
    await agy(['disable', 'taskix-manager']);
    const options = { repository: fileURLToPath(new URL('../../../', import.meta.url)), agy: process.env.AGENTIX_TEST_AGY ?? 'agy',
        run: async (binary, args) => (await run(binary, args, { env, timeout: 15000 })).stdout };
    await installNativePlugins(options);
    assert.equal(JSON.parse(await agy(['list', '--json'])).find(plugin => plugin.name === 'taskix-manager').enabled, false);
    await removeNativePlugins(options);
    await removeNativePlugins(options);
    const remaining = JSON.stringify(JSON.parse(await agy(['list', '--json'])));
    assert.ok(remaining.includes('unrelated-fixture'));
    for (const name of pluginNames) assert.ok(!remaining.includes(name));
});
