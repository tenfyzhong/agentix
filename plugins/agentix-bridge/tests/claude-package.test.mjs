import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
const root = new URL('../', import.meta.url);
const json = path => JSON.parse(readFileSync(new URL(path, root)));
test('agentix marketplace distributes a standalone Claude bridge with exec-form hooks', () => {
    const market = json('../../.claude-plugin/marketplace.json');
    assert.equal(market.name, 'agentix');
    assert.equal(market.plugins.find(p => p.name === 'agentix-bridge')?.source, './plugins/agentix-bridge');
    assert.equal(json('.claude-plugin/plugin.json').name, 'agentix-bridge');
    const server = json('.mcp.json').mcpServers.bridge;
    assert.equal(server.command, 'node');
    assert.ok(existsSync(new URL(server.args[0].replace('${CLAUDE_PLUGIN_ROOT}/', ''), root)));
    const hooks = json('hooks/hooks.json').hooks;
    for (const name of ['SessionStart', 'UserPromptSubmit', 'Stop', 'StopFailure', 'SessionEnd']) {
        assert.equal(hooks[name][0].hooks[0].command, 'node');
        assert.deepEqual(hooks[name][0].hooks[0].args, ['${CLAUDE_PLUGIN_ROOT}/claude/hook.mjs']);
    }
});
