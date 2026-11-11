import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const execute = promisify(execFile);
const ref = process.env.AGENTIX_TEST_NATIVE_REMOTE_REF || 'main';
for (const host of ['opencode', 'omp', 'agy']) {
    test(`${host}_installs_from_native_remote_commands_outside_a_checkout`, {
        skip: process.env.AGENTIX_TEST_NATIVE_REMOTE_INSTALL !== '1',
        timeout: 180000,
    }, async t => {
        const root = await mkdtemp(join(tmpdir(), `agentix-native-${host}-`));
        t.after(() => rm(root, { recursive: true, force: true }));
        const config = join(root, 'config/opencode');
        await mkdir(config, { recursive: true });
        const env = { ...process.env, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
            XDG_CACHE_HOME: join(root, 'cache'), XDG_DATA_HOME: join(root, 'data'),
            XDG_STATE_HOME: join(root, 'state'), OPENCODE_CONFIG_DIR: config,
            PI_CODING_AGENT_DIR: join(root, 'agent') };
        delete env.OPENCODE_CONFIG;
        delete env.OPENCODE_CONFIG_CONTENT;
        const run = async args => (await execute(host, args, { env, cwd: root, timeout: 90000 })).stdout;
        const names = ['taskix-manager', 'agentix-bridge'];
        if (host === 'omp') {
            await run(['plugin', 'marketplace', 'add', 'tenfyzhong/agentix']);
            for (const name of names) await run(['plugin', 'install', `${name}@agentix`]);
            const listing = await run(['plugin', 'list']);
            for (const name of names) assert.ok(listing.includes(name), listing);
        } else if (host === 'agy') {
            for (const name of names) await run(['plugin', 'install',
                `https://github.com/tenfyzhong/agentix/tree/${encodeURIComponent(ref)}/plugins/${name}`]);
            const plugins = JSON.parse(await run(['plugin', 'list', '--json']));
            for (const name of names) {
                const plugin = plugins.find(plugin => plugin.name === name);
                assert.ok(plugin?.enabled, JSON.stringify(plugins));
                assert.ok(plugin.mcpServers.length > 0);
                await readFile(join(plugin.path, 'antigravity/server.mjs'));
            }
        } else {
            for (const name of names) await run(['plugin', 'add',
                `github:tenfyzhong/agentix#${ref}::path:plugins/${name}`]);
            const server = await readFile(join(config, 'opencode.json'), 'utf8');
            const tui = await readFile(join(config, 'cli.json'), 'utf8');
            assert.ok(server.includes('plugins/taskix-manager'), server);
            assert.ok(tui.includes('plugins/agentix-bridge'), tui);
        }
    });
}
