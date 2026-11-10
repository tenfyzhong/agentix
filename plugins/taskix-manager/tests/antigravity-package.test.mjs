import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, access, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildNativePlugins } from '../../../scripts/antigravity-plugin.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
for (const name of ['taskix-manager', 'agentix-bridge']) {
    test(`${name}_ships_an_independently_loadable_native_antigravity_plugin`, async () => {
        const directory = join(root, 'plugins', name);
        const json = async file => JSON.parse(await readFile(join(directory, file), 'utf8'));
        const manifest = await json('.antigravity-plugin/plugin.json');
        assert.equal(manifest.name, name);
        assert.equal(manifest.version, (await json('package.json')).version);
        const config = await json('mcp_config.json');
        const servers = Object.values(config.mcpServers);
        assert.equal(servers.length, 1);
        assert.equal(servers[0].command, 'node');
        assert.deepEqual(servers[0].args, ['${PLUGIN_ROOT}/antigravity/server.mjs']);
        const hooks = (await json('hooks.json'))[name];
        for (const event of ['SessionStart', 'PreInvocation', 'Stop']) {
            assert.equal(hooks[event][0].command, 'node antigravity/hook.mjs');
        }
        await access(join(directory, 'antigravity/server.mjs'));
        const files = (await json('package.json')).files;
        for (const required of ['antigravity/', '.antigravity-plugin/', 'mcp_config.json', 'hooks.json']) assert.ok(files.includes(required), required);
    });
}
test('taskix_bundles_the_same_pinned_sdk_and_license_inventory', async () => {
    for (const file of ['sdk.mjs', 'LICENSES.md']) {
        assert.equal(await readFile(join(root, 'plugins/taskix-manager/antigravity/vendor', file), 'utf8'),
            await readFile(join(root, 'plugins/agentix-bridge/claude/vendor', file), 'utf8'));
    }
});
test('native_packages_exclude_foreign_configuration_and_preserve_the_complete_taskix_workflow', async t => {
    const target = await mkdtemp(join(tmpdir(), 'agy-packages-'));
    t.after(() => rm(target, { recursive: true, force: true }));
    await buildNativePlugins(root, target);
    for (const name of ['taskix-manager', 'agentix-bridge']) {
        const plugin = join(target, name);
        assert.equal(JSON.parse(await readFile(join(plugin, 'plugin.json'), 'utf8')).name, name);
        await assert.rejects(access(join(plugin, '.mcp.json')));
        await assert.rejects(access(join(plugin, 'hooks/hooks.json')));
        await access(join(plugin, 'antigravity/server.mjs'));
    }
    const skill = join(target, 'taskix-manager/skills/taskix-manager');
    const entry = await readFile(join(skill, 'SKILL.md'), 'utf8');
    assert.ok(Buffer.byteLength(entry) < 24000);
    assert.match(entry, /read.*antigravity-workflow\.md/i);
    const full = await readFile(join(skill, 'references/antigravity-workflow.md'), 'utf8');
    const original = await readFile(join(root, 'plugins/taskix-manager/skills/taskix-manager/SKILL.md'), 'utf8');
    assert.equal(full, original.replace(/^---\n[\s\S]*?\n---\n/, '').replaceAll('(references/', '('));
});
