import { NativeTurns, promptText } from './turns.mjs';
import { registerExtension } from '../runtime.mjs';
import { runTaskix } from '../taskix-cli.mjs';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

/** Native OpenCode 2 server plugin. One Taskix extension owns each native session. */
export async function setupOpenCode(input, options = {}) {
    const sessions = new Map(),
        controller = new AbortController();
    let disposed = false;
    const report =
        options.onError ??
        ((error) => console.error('Taskix OpenCode plugin:', error));
    async function session(id) {
        if (disposed) throw new Error('Taskix plugin is disposed');
        if (!id) throw new Error('OpenCode session ID is required');
        if (!sessions.has(id)) {
            const ready = (async () => {
                const native = await input.session.get({ sessionID: id });
                const handlers = new Map(),
                    notices = [];
                let tool;
                const ctx = {
                    cwd: native.location.directory,
                    sessionManager: { getSessionId: () => id },
                    isIdle: () => true,
                };
                registerExtension(
                    {
                        on: (name, handler) => handlers.set(name, handler),
                        registerTool: (value) => {
                            tool = value;
                        },
                        sendMessage: (message) => notices.push(message.content),
                    },
                    'opencode',
                    options.runner ?? runTaskix,
                    options.timers ?? globalThis,
                    undefined,
                    { ...options.routing, turnRegistry: new Map() },
                );
                const state = {
                    ctx,
                    handlers,
                    tool,
                    notices,
                    turns: new Map(),
                    pending: new Map(),
                    pendingBytes: 0,
                    seen: new Set(),
                    texts: new Map(),
                    native: new NativeTurns(),
                    chain: Promise.resolve(),
                };
                await handlers.get('session_start')({}, ctx);
                const saved = await input.storage.get('active:' + id);
                if (saved?.version === 1 && Array.isArray(saved.turns)) {
                    if (
                        native.outcome &&
                        Number(native.time?.idle) >= saved.updated
                    )
                        await input.storage.remove('active:' + id);
                    else {
                        state.turns = new Map(
                            saved.turns.map((turn) => [turn.id, turn.text]),
                        );
                        for (const turn of saved.turns) state.seen.add(turn.id);
                        state.context = saved.context;
                        state.current = saved.current;
                        if (state.current) {
                            state.native.user(state.current);
                            state.restored = true;
                            state.ownedLease = await handlers.get(
                                'agent_restored',
                            )({ turnId: state.current }, ctx);
                        }
                    }
                }
                return state;
            })();
            sessions.set(id, ready);
            ready.catch(() => {
                if (sessions.get(id) === ready) sessions.delete(id);
            });
        }
        return sessions.get(id);
    }
    function serialized(state, work) {
        state.work = (state.work ?? 0) + 1;
        const result = state.chain.then(work).finally(() => {
            state.work--;
        });
        state.chain = result.catch(() => {});
        return result;
    }
    async function trimSessions(current) {
        let idle = 0;
        for (const ready of sessions.values()) {
            const state = await ready;
            // Heartbeat may have observed an external release since settlement.
            // Read the extension's local ownership cache, without another CLI.
            state.ownedLease = state.handlers.get('lease_owned')({}, state.ctx);
            if (!state.turns.size && !state.ownedLease && !state.work) idle++;
        }
        for (const [id, ready] of sessions) {
            if (idle <= (options.maxIdleSessions ?? 64)) break;
            const state = await ready;
            if (
                id === current ||
                state.turns.size ||
                state.ownedLease ||
                state.work
            )
                continue;
            await state.handlers.get('session_shutdown')({}, state.ctx);
            sessions.delete(id);
            idle--;
        }
    }
    async function settle(state, aborted = false) {
        if (!state.turns.size) return;
        for (const [turn, prompt] of state.turns) {
            const text = [...state.texts.values()]
                .filter((part) => part.turn === turn)
                .map((part) => part.text)
                .join('');
            const messages =
                text || aborted
                    ? [
                          {
                              role: 'assistant',
                              content: [{ type: 'text', text }],
                              stopReason: aborted ? 'aborted' : 'stop',
                          },
                      ]
                    : [];
            await state.handlers.get('agent_end')(
                {
                    messages,
                    turnId: turn,
                    ...(prompt === undefined
                        ? {}
                        : {
                              prompt: {
                                  id: `${turn}:${turn}:user`,
                                  role: 'user',
                                  text: prompt,
                              },
                          }),
                },
                state.ctx,
            );
            state.turns.delete(turn);
        }
        state.ownedLease = await state.handlers.get('agent_settled')(
            {},
            state.ctx,
        );
        state.texts.clear();
        state.native = new NativeTurns();
        state.current = undefined;
        await save(state);
        await trimSessions(state.ctx.sessionManager.getSessionId());
    }
    async function save(state) {
        const key = 'active:' + state.ctx.sessionManager.getSessionId();
        if (!state.turns.size) {
            await input.storage.remove(key);
            return;
        }
        if (state.turns.size > 4096)
            throw new Error('Taskix OpenCode active prompt limit exceeded');
        let budget = options.maxPersistedPromptBytes ?? 1024 * 1024;
        const turns = [...state.turns].map(([id, text]) => {
            const bytes = Buffer.byteLength(text ?? '');
            if (text !== undefined && bytes <= budget) {
                budget -= bytes;
                return { id, text };
            }
            // The user record was captured on delivery. Omit large prompt text
            // from recovery storage; the source ID still deduplicates the reply.
            return { id };
        });
        const context =
            Buffer.byteLength(state.context ?? '') <=
            (options.maxPersistedContextBytes ?? 256 * 1024)
                ? state.context
                : `Task session: ${state.ctx.sessionManager.getSessionId()}. Current discussion turn: "${state.current}". Use taskix context to read current task and lease state before acting.`;
        await input.storage.set(key, {
            version: 1,
            updated: Date.now(),
            current: state.current,
            turns,
            context,
        });
    }
    function forgetPending(state, id) {
        const old = state.pending.get(id);
        if (old) state.pendingBytes -= Buffer.byteLength(old.text);
        state.pending.delete(id);
    }
    function stage(state, id, text, admitted = false) {
        const now = Date.now();
        for (const [key, entry] of state.pending)
            if (
                !entry.admitted &&
                now - entry.time > (options.pendingTtlMs ?? 300000)
            )
                forgetPending(state, key);
        forgetPending(state, id);
        state.pending.set(id, { text, admitted, time: now });
        state.pendingBytes += Buffer.byteLength(text);
        while (
            state.pending.size > (options.maxPendingPrompts ?? 1024) ||
            state.pendingBytes > (options.maxPendingBytes ?? 8 * 1024 * 1024)
        )
            forgetPending(state, state.pending.keys().next().value);
    }
    async function activate(state, id, fallback) {
        const text = state.pending.get(id)?.text ?? fallback;
        if (text === undefined || state.seen.has(id)) return;
        await state.handlers.get('agent_start')({}, state.ctx);
        const result = await state.handlers.get('before_agent_start')(
            { prompt: text, turnId: id },
            state.ctx,
        );
        const previous = { current: state.current, context: state.context };
        state.current = id;
        state.turns.set(id, text);
        state.context = `Task session: ${state.ctx.sessionManager.getSessionId()}. Use the taskix-manager skill for tracked work.\n${result.message.content}`;
        try {
            await save(state);
        } catch (error) {
            state.turns.delete(id);
            state.current = previous.current;
            state.context = previous.context;
            throw error;
        }
        // A failed route or storage write must remain retryable. Mark the
        // native prompt consumed only after both operations have succeeded.
        state.native.user(id);
        state.seen.add(id);
        while (state.seen.size > 4096)
            state.seen.delete(state.seen.values().next().value);
        forgetPending(state, id);
    }
    async function reconcile(state, delivered) {
        const rows = await input.session.context({
            sessionID: state.ctx.sessionManager.getSessionId(),
        });
        const users = rows.filter((message) => message.type === 'user');
        const ids = new Set(users.map((message) => message.id));
        for (const message of users)
            if (
                message.id === delivered ||
                state.pending.has(message.id) ||
                (!state.current && message === users.at(-1))
            )
                await activate(state, message.id, promptText(message));
        // Failed preparation is never admitted. Keep durable queued entries;
        // discard only the speculative payload after an authoritative read.
        for (const [id, entry] of state.pending)
            if (!entry.admitted && !ids.has(id)) forgetPending(state, id);
    }
    async function prompt(event) {
        const { sessionID, messageID } = event,
            text = promptText(event.prompt);
        if (!text) return;
        const state = await session(sessionID);
        await serialized(state, () => {
            if (!state.seen.has(messageID)) stage(state, messageID, text);
        });
        await trimSessions(sessionID);
    }
    async function event(frame) {
        frame = structuredClone(frame);
        const data = frame.data;
        if (!data?.sessionID || !sessions.has(data.sessionID)) return;
        const state = await session(data.sessionID);
        await serialized(state, async () => {
            if (
                frame.type === 'session.inbox.enqueued' &&
                data.item?.type === 'user'
            ) {
                if (!state.seen.has(data.inboxID))
                    stage(
                        state,
                        data.inboxID,
                        promptText(data.item.payload),
                        true,
                    );
            } else if (frame.type === 'session.inbox.delivered') {
                if (state.pending.has(data.inboxID))
                    await activate(state, data.inboxID);
                else if (!state.seen.has(data.inboxID))
                    await reconcile(state, data.inboxID);
            } else if (frame.type === 'session.inbox.cancelled')
                forgetPending(state, data.inboxID);
            else if (frame.type === 'session.step.started')
                state.native.assistant(data.assistantMessageID);
            else if (
                frame.type === 'session.text.delta' ||
                frame.type === 'session.text.ended'
            ) {
                const turn = state.native.root(data.assistantMessageID);
                if (!turn || !state.turns.has(turn)) return;
                const key = data.assistantMessageID + ':' + data.ordinal;
                const part = state.texts.get(key) ?? { turn, text: '' };
                if (frame.type === 'session.text.ended') part.text = data.text;
                else part.text += data.delta;
                state.texts.set(key, part);
            } else if (
                frame.type === 'session.execution.succeeded' ||
                frame.type === 'session.execution.failed'
            )
                await settle(state);
            else if (
                frame.type === 'session.execution.interrupted' &&
                data.reason !== 'shutdown'
            )
                await settle(state, true);
            else if (frame.type === 'session.deleted') {
                try {
                    await settle(state, true);
                } finally {
                    await state.handlers.get('session_shutdown')({}, state.ctx);
                    sessions.delete(data.sessionID);
                }
            }
        });
    }
    await input.session.hook('prompt', prompt);
    await input.session.hook('context', async (event) => {
        const state = await session(event.sessionID);
        await serialized(state, async () => {
            // Reconcile only a first model context or ambiguous admission. Once
            // delivered events catch up, ordinary tool steps need no SDK reads.
            if (state.pending.size || !state.current) await reconcile(state);
        });
        if (state.context)
            event.system.push({ type: 'text', text: state.context });
        event.system.push(
            ...state.notices.splice(0).map((text) => ({ type: 'text', text })),
        );
    });
    const skillPath = fileURLToPath(
        new URL('../skills/taskix-manager/SKILL.md', import.meta.url),
    );
    const skillSource = await readFile(skillPath, 'utf8');
    const skill = {
        id: 'taskix-manager',
        name: 'taskix-manager',
        description: skillSource.match(/^description: (.+)$/m)?.[1],
        path: skillPath,
        content: skillSource
            .replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '')
            .trim(),
    };
    await input.skill.transform((editor) => editor.add(skill));
    await input.tool.transform((editor) =>
        editor.add({
            name: 'taskix',
            // CodeMode shares one parent call ID across nested calls. Taskix
            // writes require the unique ID of a direct native tool call.
            options: { codemode: false },
            description:
                'Manage Taskix jobs, tasks, plans, discussions and lifecycle. Identity and lease tokens are supplied by the host.',
            input: {
                type: 'object',
                properties: {
                    args: {
                        type: 'array',
                        items: { type: 'string' },
                        description:
                            'Taskix CLI arguments, one string per argument',
                    },
                },
                required: ['args'],
                additionalProperties: false,
            },
            async execute(params, ctx) {
                if (!ctx.id)
                    throw new Error(
                        'OpenCode tool call ID is required for safe retries',
                    );
                const state = await session(ctx.sessionID);
                const result = await state.tool.execute(
                    ctx.id,
                    params,
                    ctx.signal,
                    undefined,
                    state.ctx,
                );
                if (
                    state.restored &&
                    params.args[0] === 'task' &&
                    ['claim', 'claim-next'].includes(params.args[1])
                )
                    state.ownedLease = await state.handlers.get(
                        'agent_restored',
                    )({ turnId: state.current }, state.ctx);
                return {
                    content: result.content.map((part) => part.text).join('\n'),
                };
            },
        }),
    );
    const eventLimit = Math.max(1, options.maxConcurrentEvents ?? 16);
    const sessionEventLimit = Math.max(1, options.maxSessionEvents ?? 4);
    const inflight = new Set(),
        sessionEvents = new Map();
    // There is one serial event-stream consumer, hence at most one capacity
    // waiter. Consume its resolver on every wake; never accumulate Promise.race
    // reactions on a long-lived abort signal or a slow accepted operation.
    let capacityWaiter;
    const notifyCapacity = () => {
        const resolve = capacityWaiter;
        capacityWaiter = undefined;
        resolve?.();
    };
    controller.signal.addEventListener('abort', notifyCapacity, { once: true });
    async function dispatch(frame) {
        // A single-worker mode is also useful for deterministic host tests.
        if (eventLimit === 1) {
            try {
                await event(frame);
            } catch (error) {
                report(error);
            }
            return;
        }
        const id = frame.data?.sessionID;
        let group = sessionEvents.get(id);
        if (!group) {
            group = new Set();
            sessionEvents.set(id, group);
        }
        while (inflight.size >= eventLimit || group.size >= sessionEventLimit) {
            await new Promise((resolve) => {
                capacityWaiter = resolve;
            });
            if (controller.signal.aborted) return;
        }
        if (controller.signal.aborted) return;
        // A previous task may have removed the now-empty group while this
        // dispatcher was waiting for capacity. Reattach it before admission.
        sessionEvents.set(id, group);
        const work = event(frame)
            .catch(report)
            .finally(() => {
                inflight.delete(work);
                group.delete(work);
                if (!group.size && sessionEvents.get(id) === group)
                    sessionEvents.delete(id);
                notifyCapacity();
            });
        inflight.add(work);
        group.add(work);
    }
    const subscription = (async () => {
        try {
            for await (const frame of input.event.subscribe({
                signal: controller.signal,
            })) {
                if (controller.signal.aborted) break;
                await dispatch(frame);
            }
        } catch (error) {
            if (!controller.signal.aborted) report(error);
        } finally {
            // Backpressure limits accepted work globally and per session.
            // At a sustained flood, the shared native stream waits at capacity.
            await Promise.allSettled(inflight);
        }
    })();
    return async () => {
        disposed = true;
        controller.abort();
        await subscription;
        const results = await Promise.allSettled(
            [...sessions.values()].map(async (ready) => {
                const state = await ready;
                await state.chain;
                // Plugin shutdown is not a native terminal reply. Preserve
                // the active source record for a resumed execution, while
                // releasing this process's Taskix lease and renewal timer.
                await state.handlers.get('session_shutdown')({}, state.ctx);
            }),
        );
        sessions.clear();
        const failure = results.find((result) => result.status === 'rejected');
        if (failure) throw failure.reason;
    };
}
