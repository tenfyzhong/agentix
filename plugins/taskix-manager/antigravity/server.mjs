import { randomUUID } from 'node:crypto';
import { Server, StdioServerTransport, ListToolsRequestSchema, CallToolRequestSchema } from './vendor/sdk.mjs';
import { createAntigravityHooks, createAntigravityTool } from './runtime.mjs';
import { runHook } from '../runtime.mjs';
import { runTaskix } from '../taskix-cli.mjs';

const hooks = createAntigravityHooks(), tool = createAntigravityTool(), instance = randomUUID();
const mcp = new Server({ name: 'taskix-manager', version: '0.2.2' }, { capabilities: { tools: {} } });
const report = error => console.error(`Taskix Antigravity: ${error.message}`);
const active = new Set();
let closing = false;
mcp.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: [{ name: 'taskix',
    description: 'Manage Taskix tasks, jobs, plans, issues and discussions. Pass argument strings, not a shell command. Session, executor and lease identity are supplied by the host. Use the taskix-manager skill.',
    inputSchema: { type: 'object', properties: { args: { type: 'array', items: { type: 'string' } } }, required: ['args'], additionalProperties: false },
}] }));
mcp.setRequestHandler(CallToolRequestSchema, async (request, extra) => {
    if (closing || request.params.name !== 'taskix') throw new Error('Taskix tool is unavailable');
    const work = (async () => {
        const result = await tool(`${instance}:${extra.requestId}`, request.params.arguments, await hooks.identity(), extra.signal);
        return { content: result.content };
    })();
    active.add(work);
    try { return await work; }
    catch (error) { return { isError: true, content: [{ type: 'text', text: error.message }] }; }
    finally { active.delete(work); }
});
const controller = new AbortController();
let heartbeat;
const timer = setInterval(() => {
    if (heartbeat || closing) return;
    heartbeat = (async () => {
        const identity = await hooks.identity();
        if (!identity?.ready || identity.interrupted || closing) return;
        await runTaskix(['hook', 'heartbeat'], { cwd: identity.cwd, session: identity.session_id,
            executor: `agent:antigravity:${identity.session_id}`, signal: controller.signal });
    })().catch(error => { if (!closing) report(error); }).finally(() => { heartbeat = undefined; });
}, 60000);
timer.unref();
async function close() {
    if (closing) return;
    closing = true;
    clearInterval(timer); controller.abort();
    await Promise.allSettled([...active, heartbeat]);
    try {
        const identity = await hooks.identity();
        if (identity) await runHook({ ...identity, hook_event_name: 'SessionEnd' }, (args, options) =>
            runTaskix(args, { ...options, executor: `agent:antigravity:${identity.session_id}` }));
    } catch (error) { report(error); }
    await mcp.close();
}
process.stdin.once('end', close);
process.once('SIGTERM', close);
process.once('SIGINT', close);
await mcp.connect(new StdioServerTransport());
