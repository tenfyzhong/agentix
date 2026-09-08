import { BridgeTransport } from './transport.mjs';
export { PROTOCOL_VERSION } from './transport.mjs';
import { randomUUID } from 'node:crypto';
import { failure } from './protocol/errors.mjs';
import { DurableQueue } from './queue.mjs';
import { createHostAdapter } from './host.mjs';
import { SessionState, textOf } from './session.mjs';

/** In-process bridge. It never launches, resumes, or terminates another agent. */
export function registerBridge(api, kind, options = {}) {
    const host = createHostAdapter(api, kind);
    let ctx, record;
    let instance, session;
    let chain = Promise.resolve();
    let lifecycle = Promise.resolve();
    const transport = new BridgeTransport({ ...options, snapshot: info, handle: command, dispatch: (work, method) => {
        if (['info', 'snapshot', 'history', 'queue_state', 'stop'].includes(method)) return work();
        chain = chain.then(work); return chain;
    } });
    let queue = new DurableQueue(), active = false;
    function persist(queueState = queue.record()) {
        session.touch();
        api.appendEntry('agentix.bridge', { session_id: ctx.sessionManager.getSessionId(), ...queueState, ...session.state() });
        if (record) event({ QueueChanged: { session_id: record.session_id } });
    }
    function restoreQueue() {
        const records = ctx.sessionManager.getEntries().filter(e => e.type === 'custom' && e.customType === 'agentix.bridge' && e.data?.session_id === ctx.sessionManager.getSessionId()).map(e => e.data);
        const saved = records.at(-1);
        session = new SessionState(ctx, saved);
        queue = new DurableQueue(DurableQueue.restore(records), persist);
    }
    async function pump() {
        const incarnation = instance;
        if (!active || !ctx.isIdle()) return;
        const item = queue.peek();
        if (!item) return;
        session.pendingText = item.text;
        beginDelivery(() => queue.claim());
        try { await api.sendUserMessage(item.text); }
        catch (error) { if (!active || instance !== incarnation) return; queue.markUncertain(); if (session.turn) session.turn.error = error.message; finish(); }
    }
    function schedulePump() {
        const incarnation = instance;
        setImmediate(() => { if (active && incarnation === instance) chain = chain.then(pump).catch(error => console.error(`Agentix queue: ${error.message}`)); });
    }

    const event = value => transport.event(value);
    function info() { return { instance, seq: transport.sequence, session: session.summary(api.getSessionName?.() ?? null), capabilities: host.capabilities(ctx) }; }
    function snapshot() { return { ...info(), turns: session.history().turns, queue: queue.view() }; }
    function start() {
        if (session.start()) event({ TurnStarted: { session_id: record.session_id, turn_id: session.turn.id } });
    }
    function beginDelivery(commit) {
        const previous = session.turn;
        const created = session.start();
        try { commit(); }
        catch (error) { session.turn = previous; throw error; }
        if (created) event({ TurnStarted: { session_id: record.session_id, turn_id: session.turn.id } });
    }
    function finish() {
        if (!session.finish()) return;
        queue.finish(session.interrupted);
        event({ ItemCompleted: { session_id: record.session_id, turn_id: session.turn.id, item: { id: `${session.turn.id}:assistant`, kind: 'agentMessage', text: session.turn.agent_text, status: session.turn.status } } });
        event({ TurnCompleted: { session_id: record.session_id, turn_id: session.turn.id, status: session.turn.status, error: session.turn.error ?? null } });
        schedulePump();
    }
    async function command(method, params) {
        const incarnation = instance;
        const ensureCurrent = () => {
            if (!active || instance !== incarnation) throw failure('session_changed', 'The original session changed before the operation completed');
        };
        ensureCurrent();
        if (method === 'info') return info();
        if (method === 'snapshot') return snapshot();
        if (method === 'history') return session.history(params.cursor, params.limit);
        if (method === 'queue_state') return queue.view();
        if (method === 'queue') {
            const item = queue.enqueue(params.request_id, params.text); schedulePump(); return item;
        }
        if (method === 'queue_resume') { queue.resume(); await pump(); return { message: 'Queue resumed' }; }
        if (method === 'queue_clear') { if (ctx.isIdle() && queue.view().uncertain) session.abandon(); queue.clear(ctx.isIdle()); return { message: 'Pending queue cleared' }; }
        if (method === 'command') {
            const result = await host.command(ctx, record.session_id, queue.view(), params.name, params.value, ensureCurrent);
            ensureCurrent();
            if (params.name === 'compact' || (params.value != null && ['model', 'reasoning', 'rename'].includes(params.name))) session.touch();
            return result;
        }

        if (method === 'prompt' || method === 'steer') {
            if (typeof params.text !== 'string' || !params.text.trim()) throw failure('invalid_request', 'A nonempty prompt is required');
            const id = params.request_id;
            if (typeof id !== 'string' || !id) throw failure('invalid_request', 'request_id is required');
            const receipt = queue.receipt(id, { method, text: params.text });
            if (receipt) return receipt;
            if (method === 'steer' && ctx.isIdle()) throw failure('invalid_request', 'No active turn to steer');
            if (method === 'prompt' && !ctx.isIdle()) throw failure('busy', 'Session is busy');
            session.pendingText = params.text;
            let result;
            const remember = () => {
                result = { turn_id: session.turn?.id ?? randomUUID() };
                queue.remember(id, result, method === 'prompt' ? { id, text: params.text } : null, { method, text: params.text });
            };
            if (method === 'prompt') beginDelivery(remember);
            else remember();
            try { await api.sendUserMessage(params.text, method === 'steer' ? { deliverAs: 'steer' } : undefined); }
            catch (error) {
                ensureCurrent();
                queue.fail(id, error, method === 'prompt');
                if (method === 'prompt') { session.interrupted = false; if (session.turn) session.turn.error = error.message; finish(); }
                throw error;
            }
            return result;
        }
        if (method === 'stop') { queue.pause(); session.interrupted = true; ctx.abort(); return {}; }
        throw failure('unsupported_method', `Unsupported bridge method: ${method}`);
    }
    async function close() { active = false; await transport.close(); }
    async function open(context) {
        await close();
        ctx = context;
        // Each session switch gets a new incarnation; reconnect preserves this identity.
        instance = randomUUID();
        chain = Promise.resolve();
        restoreQueue();
        record = { agent: kind, instance, pid: process.pid,
            session_id: ctx.sessionManager.getSessionId(), cwd: ctx.cwd, session_file: ctx.sessionManager.getSessionFile?.() ?? null };
        active = true;
        transport.open(record);
        schedulePump();
    }
    const on = (name, fn) => api.on(name, async (value, context) => {
        try { if (name !== 'session_start') session?.touch(); await fn(value, context); } catch (error) { console.error(`Agentix bridge ${name}: ${error.message}`); }
    });
    on('session_start', (_event, context) => { active = false; lifecycle = lifecycle.then(() => open(context)); return lifecycle; });
    on('session_shutdown', () => {
        lifecycle = lifecycle.catch(() => {}).then(async () => {
            if (record) event({ SessionExited: { session_id: record.session_id } });
            active = false;
            await transport.close(true);
        });
        return lifecycle;
    });
    on('input', value => { session.pendingText = value.text ?? ''; });
    on('agent_start', () => { if (record) start(); });
    on('message_start', value => {
        if (!record) return;
        if (value.message?.role === 'user') {
            session.pendingText = textOf(value.message.content); start(); session.turn.user_text = session.pendingText; session.turn.user_timestamp = value.message.timestamp; session.associate();
            event({ ItemCompleted: { session_id: record.session_id, turn_id: session.turn.id, item: { id: `${session.turn.id}:user`, kind: 'userMessage', text: session.pendingText, status: 'completed' } } });
        }
    });
    on('message_update', value => {
        if (!record) return;
        start();
        const update = value.assistantMessageEvent;
        if (update?.type === 'text_delta') {
            session.turn.agent_text += update.delta;
            event({ AgentMessageDelta: { session_id: record.session_id, turn_id: session.turn.id, item_id: `${session.turn.id}:assistant`, delta: update.delta } });
        }
    });
    on('tool_execution_start', value => {
        if (!record) return;
        start();
        const label = value.toolName ?? 'tool';
        session.turn.tools.push({ kind: label, label, status: 'inProgress', id: value.toolCallId });
        event({ ItemStarted: { session_id: record.session_id, turn_id: session.turn.id, item_id: value.toolCallId, kind: label, label } });
    });
    on('tool_execution_end', value => {
        if (!record || !session.turn) return;
        const status = value.isError ? 'failed' : 'completed';
        const tool = session.turn.tools.find(tool => tool.id === value.toolCallId);
        if (tool) tool.status = status;
        const item = { id: value.toolCallId, kind: value.toolName ?? 'tool', text: textOf(value.result?.content).slice(0, 65536), status };
        session.turn.items.push(item);
        event({ ItemCompleted: { session_id: record.session_id, turn_id: session.turn.id, item } });
        persist();
    });
    on('message_end', value => {
        if (value.message?.stopReason === 'aborted') session.interrupted = true;
        if (value.message?.stopReason === 'error' && session.turn) session.turn.error = value.message.errorMessage ?? 'Agent failed';
        session.associate();
    });
    on('agent_end', value => {
        if (value.messages?.some(m => m.stopReason === 'aborted')) session.interrupted = true;
        if (host.isSettled('agent_end', value)) finish();
    });
    if (host.settledEvent !== 'agent_end') on(host.settledEvent, finish);
    return { close, snapshot };
}
