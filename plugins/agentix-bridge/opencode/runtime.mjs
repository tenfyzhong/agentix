import { randomUUID } from 'node:crypto';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { BridgeTransport } from '../transport.mjs';
import { failure } from '../protocol/errors.mjs';
import { messageId, openQueue } from './state.mjs';
import { NativeTurns, normalizeMessages } from './turns.mjs';

const capabilities = [
    'prompt',
    'history',
    'stop',
    'queue',
    'queue_control',
    'new',
    'status',
    'rename',
    'model',
    'compact',
    'diff',
    'skills',
];
const eventTypes = [
    'session.inbox.enqueued',
    'session.inbox.delivered',
    'session.step.started',
    'session.text.started',
    'session.text.delta',
    'session.text.ended',
    'session.tool.input.started',
    'session.tool.called',
    'session.tool.success',
    'session.tool.failed',
    'session.execution.succeeded',
    'session.execution.failed',
    'session.execution.interrupted',
    'session.compaction.started',
    'session.compaction.ended',
    'session.compaction.failed',
    'session.deleted',
    'session.renamed',
    'permission.asked',
    'permission.replied',
    'form.created',
    'form.replied',
    'form.cancelled',
];

export class OpenCodeBridge {
    constructor(api, options = {}) {
        this.api = api;
        this.options = options;
        this.stateDir =
            options.stateDir ??
            process.env.AGENTIX_OPENCODE_STATE_DIR ??
            join(homedir(), '.local/share/agentix/opencode');
        this.clientId = randomUUID();
        this.native = new NativeTurns();
        this.aliases = this.native.aliases;
        this.messages = new Map();
        this.parts = new PartCache();
        this.announced = new Set();
        this.pendingUsers = new Set();
        this.pending = new Map();
        this.activeTurns = new Set();
        this.requests = new Set();
        this.disposers = [];
        this.events = Promise.resolve();
        this.chain = Promise.resolve();
        this.closed = false;
        const factory =
            options.transportFactory ??
            ((handlers) => new BridgeTransport(handlers));
        this.transport = factory({
            ...options,
            snapshot: () => this.info(),
            handle: (method, params) => this.command(method, params),
            dispatch: (work, method) => {
                if (
                    [
                        'info',
                        'history',
                        'snapshot',
                        'stop',
                        'interaction',
                        'queue_state',
                    ].includes(method)
                )
                    return work();
                const result = this.chain.then(work);
                this.chain = result.catch(() => {});
                return result;
            },
            registered: () =>
                setImmediate(() =>
                    this.replayInteractions().catch((error) =>
                        this.report(error),
                    ),
                ),
        });
    }
    get client() {
        return this.api.client;
    }
    report(error) {
        this.api.ui.toast?.show({
            title: 'Agentix',
            message: error.message,
            variant: 'error',
            sessionID: this.session?.id,
        });
    }
    emit(event) {
        this.prune();
        if (!this.closed) this.transport.event(event);
    }
    prune() {
        this.native.prune();
        for (const map of [this.messages, this.announced, this.pendingUsers])
            for (const id of map.keys()) {
                if (map.size <= 4096) break;
                if (!this.activeTurns.has(id)) map.delete(id);
            }
    }
    async sdk(method, input = {}, timeout = this.options.sdkTimeoutMs ?? 5000) {
        const controller = new AbortController();
        this.requests.add(controller);
        let timer, aborted;
        const cancelled = new Promise((_, reject) => {
            aborted = () =>
                reject(
                    controller.signal.reason ??
                        new Error('OpenCode SDK request cancelled'),
                );
            controller.signal.addEventListener('abort', aborted, {
                once: true,
            });
        });
        try {
            return await Promise.race([
                cancelled,
                method(input, { signal: controller.signal }),
                new Promise((_, reject) => {
                    timer = setTimeout(() => {
                        const error = new Error(
                            'OpenCode SDK request timed out',
                        );
                        controller.abort(error);
                        reject(error);
                    }, timeout);
                }),
            ]);
        } finally {
            clearTimeout(timer);
            controller.signal.removeEventListener('abort', aborted);
            this.requests.delete(controller);
        }
    }
    async start() {
        const version = this.api.app.version
            .match(/(?:^|v)(\d+)\.(\d+)\.(\d+)/)
            ?.slice(1)
            .map(Number);
        if (
            !version ||
            version[0] < 2 ||
            (version[0] === 2 && version[1] === 0 && version[2] < 32)
        )
            throw new Error('Agentix requires OpenCode 2.0.32 or newer');
        for (const name of eventTypes)
            this.disposers.push(
                this.api.data.on(name, (event) => {
                    // Admission payloads can contain large inline attachments. Identity is
                    // sufficient here; the native transcript remains their only owner.
                    const snapshot =
                        name === 'session.inbox.enqueued'
                            ? {
                                  type: name,
                                  data: {
                                      sessionID: event.data.sessionID,
                                      inboxID: event.data.inboxID,
                                      item: { type: event.data.item.type },
                                  },
                              }
                            : structuredClone(event);
                    this.events = this.events
                        .then(() => this.event(snapshot))
                        .catch((error) => this.report(error));
                }),
            );
        await this.refresh();
        if (this.options.poll !== false) {
            this.timer = setInterval(() => {
                if (!this.refreshing)
                    this.refreshing = this.refresh()
                        .catch((error) => this.report(error))
                        .finally(() => {
                            this.refreshing = undefined;
                        });
            }, 250);
            this.timer.unref?.();
        }
    }
    async close() {
        this.closed = true;
        clearInterval(this.timer);
        for (const controller of this.requests) controller.abort();
        for (const dispose of this.disposers.splice(0)) dispose();
        await this.transport.close();
    }
    async refresh() {
        if (this.closed || this.switching) return;
        const route = this.api.ui.router.current();
        if (route.type === 'session' && route.sessionID !== this.session?.id)
            await this.open(route.sessionID);
        else if (route.type === 'home') {
            const previous = this.session?.id;
            const session = await this.sdk(this.client.session.create, {
                location: this.api.location ?? this.api.data.location.default(),
            });
            if (this.closed || this.api.ui.router.current().type !== 'home')
                return;
            this.api.ui.router.navigate({
                type: 'session',
                sessionID: session.id,
            });
            await this.open(session.id, previous);
        }
    }
    async open(id, previous) {
        const session = await this.sdk(this.client.session.get, {
            sessionID: id,
        });
        if (this.closed) return;
        await this.transport.close(true);
        this.session = session;
        this.instance = randomUUID();
        this.pending.clear();
        this.messages.clear();
        this.parts.clear();
        this.announced.clear();
        this.pendingUsers.clear();
        this.activeTurns.clear();
        this.native = new NativeTurns();
        this.aliases = this.native.aliases;
        this.turn = undefined;
        this.model = undefined;
        const saved = openQueue(
            this.stateDir,
            id,
            () => {
                if (this.session?.id === id)
                    this.emit({ QueueChanged: { session_id: id } });
            },
            this.options,
        );
        this.queue = saved.queue;
        this.record = {
            agent: 'opencode',
            instance: this.instance,
            pid: process.pid,
            client_id: this.clientId,
            session_id: id,
            cwd: session.location.directory,
            session_file: saved.file,
            ...(previous ? { previous_session_id: previous } : {}),
        };
        const instance = this.instance;
        try {
            const history = await this.history({});
            if (this.busy())
                for (const turn of history.turns.filter(
                    (turn) => turn.status === 'inProgress',
                )) {
                    this.activeTurns.add(turn.id);
                    this.turn = turn.id;
                    this.native.current = turn.id;
                }
        } catch (error) {
            if (this.closed || instance !== this.instance) return;
            this.report(error);
        }
        if (!this.closed && instance === this.instance)
            this.transport.open(this.record);
    }
    busy() {
        return this.api.data.session.status(this.session?.id) === 'running';
    }
    info() {
        return {
            instance: this.instance,
            seq: this.transport.sequence,
            capabilities,
            session: {
                id: this.session.id,
                name: this.session.title ?? null,
                preview: null,
                cwd: this.session.location.directory,
                updatedAt: Math.floor(
                    (this.session.time.updated ?? Date.now()) / 1000,
                ),
                status: this.busy() || this.turn ? 'active' : 'idle',
            },
        };
    }
    async history({ cursor, limit = 20 }) {
        limit = Math.min(100, Math.max(1, limit));
        const instance = this.instance,
            deadline = Date.now() + (this.options.sdkTimeoutMs ?? 5000);
        let nativeCursor,
            anchor,
            inheritedOutcome,
            found = true;
        if (cursor) {
            try {
                const decoded = JSON.parse(
                    Buffer.from(cursor, 'base64url').toString(),
                );
                if (typeof decoded.anchor !== 'string') throw new Error();
                anchor = decoded.anchor;
                nativeCursor = decoded.native;
                inheritedOutcome = decoded.outcome;
                if (
                    inheritedOutcome &&
                    !['succeeded', 'failed', 'interrupted'].includes(
                        inheritedOutcome,
                    )
                )
                    throw new Error();
                found = false;
            } catch {
                throw failure(
                    'invalid_request',
                    'Invalid OpenCode history cursor',
                );
            }
        }
        let rows = [],
            exhausted = false;
        const origins = new Map();
        for (let page = 0; page < 20; page++) {
            const start = nativeCursor;
            const result = await this.sdk(
                this.client.message.list,
                {
                    sessionID: this.session.id,
                    limit: 200,
                    ...(start ? { cursor: start } : { order: 'desc' }),
                },
                Math.max(1, deadline - Date.now()),
            );
            if (Date.now() > deadline)
                throw new Error('OpenCode history request timed out');
            let batch = result.data || [];
            if (!found) {
                const offset = batch.findIndex((row) => row.id === anchor);
                if (offset >= 0) {
                    batch = batch.slice(offset + 1);
                    found = true;
                } else batch = [];
            }
            for (const row of batch) origins.set(row.id, start);
            rows = [...batch.toReversed(), ...rows];
            nativeCursor = result.cursor.next ?? undefined;
            exhausted = result.data.length < 200 || !nativeCursor;
            if (
                found &&
                (normalizeMessages(rows).turns.length > limit || exhausted)
            )
                break;
            if (exhausted) break;
            if (nativeCursor === start)
                throw failure(
                    'history_unavailable',
                    'Native history cursor did not advance',
                );
        }
        if (!found)
            throw failure(
                'history_unavailable',
                'Native history boundary is no longer available',
            );
        if (instance !== this.instance)
            throw failure(
                'session_changed',
                'OpenCode session changed during history read',
            );
        const normalized = normalizeMessages(
                inheritedOutcome
                    ? [...rows, { type: 'idle', outcome: inheritedOutcome }]
                    : rows,
            ),
            turns = normalized.turns.slice(-limit);
        if (!turns.length && !exhausted)
            throw failure(
                'history_unavailable',
                'Native turn exceeds history read limit',
            );
        for (const [id, turn] of normalized.messageTurns)
            this.aliases.set(id, turn);
        for (const row of rows.slice(-4096)) {
            this.messages.set(row.id, { id: row.id, type: row.type });
            if (row.type === 'user') this.announced.add(row.id);
            if (row.type === 'assistant') {
                let ordinal = 0;
                for (const part of row.content || []) {
                    if (part.type !== 'text') continue;
                    const key = row.id + ':' + ordinal++;
                    if (!this.parts.has(key)) this.parts.set(key, part.text);
                }
            }
        }
        this.prune();
        const older = turns[0]?.id;
        return {
            turns,
            older_cursor:
                older && (!exhausted || rows[0]?.id !== older)
                    ? Buffer.from(
                          JSON.stringify({
                              anchor: older,
                              native: origins.get(older),
                              outcome:
                                  rows
                                      .slice(
                                          rows.findIndex(
                                              (row) => row.id === older,
                                          ) + 1,
                                      )
                                      .find((row) => row.type === 'idle')
                                      ?.outcome ?? inheritedOutcome,
                          }),
                      ).toString('base64url')
                    : null,
            newer_cursor: null,
        };
    }
    startUser(id) {
        if (this.announced.has(id) || this.activeTurns.has(id)) return;
        this.native.user(id);
        this.announced.add(id);
        this.activeTurns.add(id);
        this.turn = id;
        this.interrupted = false;
        this.emit({
            TurnStarted: { session_id: this.session.id, turn_id: id },
        });
    }
    async replayInteractions() {
        if (this.closed || !this.session) return;
        for (const request of await this.sdk(this.client.permission.list, {
            sessionID: this.session.id,
        }))
            this.interaction('permission', request);
        for (const form of await this.sdk(this.client.session.form.list, {
            sessionID: this.session.id,
        }))
            this.interaction('form', form);
    }
    interaction(kind, request) {
        if (this.pending.size >= 256 && !this.pending.has(request.id))
            throw new Error(
                'Too many pending OpenCode interactions; use the native terminal',
            );
        const supported =
            kind === 'permission' ||
            request.fields.every(
                (field) =>
                    !field.when?.length &&
                    [
                        'format',
                        'minLength',
                        'maxLength',
                        'pattern',
                        'minItems',
                        'maxItems',
                    ].every((key) => field[key] === undefined) &&
                    (field.hidden
                        ? field.default !== undefined || !field.required
                        : ['string', 'multiselect'].includes(field.type)) &&
                    new Set((field.options || []).map((option) => option.label))
                        .size === (field.options || []).length,
            );
        this.pending.set(request.id, { kind, request, supported });
        const questions =
            kind === 'form' && supported
                ? request.fields
                      .filter((field) => !field.hidden)
                      .map((field) => ({
                          id: field.key,
                          header: field.title ?? request.title,
                          question:
                              field.description ?? field.title ?? request.title,
                          multiple: field.type === 'multiselect',
                          custom: field.custom ?? !field.options?.length,
                          options: (field.options || []).map((option) => ({
                              label: option.label,
                              description: option.description,
                          })),
                      }))
                : [];
        const detail =
            kind === 'permission'
                ? [request.action, ...request.resources].join('\n')
                : supported
                  ? request.title
                  : 'This OpenCode form requires the native terminal because its field types or conditions cannot be represented in IM. You may reject it here.';
        this.emit({
            InteractionRequested: {
                rpc_id: request.id,
                method: 'opencode.' + kind,
                session_id: this.session.id,
                turn_id: this.turn ?? '',
                item_id: request.source?.id ?? null,
                kind:
                    kind === 'form'
                        ? 'user_input'
                        : request.action === 'edit'
                          ? 'file_approval'
                          : 'command_approval',
                title: kind === 'form' ? request.title : 'OpenCode permission',
                detail,
                available_decisions:
                    kind === 'permission' ? ['once', 'always', 'reject'] : [],
                payload:
                    kind === 'form'
                        ? {
                              rejectable: true,
                              questions: supported
                                  ? questions
                                  : [
                                        {
                                            id: 'native_terminal',
                                            header: 'OpenCode form',
                                            question: detail,
                                            custom: false,
                                            options: [],
                                        },
                                    ],
                          }
                        : request,
                auto_resolution_ms: null,
            },
        });
    }
    async event({ type, data: p }) {
        if (this.closed || !this.session) return;
        const sid = p.sessionID ?? p.form?.sessionID;
        if (sid !== this.session.id) return;
        if (type === 'session.inbox.enqueued') {
            if (p.item.type === 'user') this.pendingUsers.add(p.inboxID);
            this.prune();
        } else if (type === 'session.inbox.delivered') {
            if (this.pendingUsers.has(p.inboxID)) {
                this.pendingUsers.delete(p.inboxID);
                this.startUser(p.inboxID);
            } else if (
                this.api.data.session.message.get(sid, p.inboxID)?.type ===
                'user'
            )
                this.startUser(p.inboxID);
        } else if (type === 'session.step.started') {
            this.native.assistant(p.assistantMessageID);
            this.messages.set(p.assistantMessageID, {
                id: p.assistantMessageID,
                type: 'assistant',
            });
            this.prune();
        } else if (type === 'session.text.started') {
            const key = p.assistantMessageID + ':' + p.ordinal;
            if (!this.parts.has(key)) this.parts.set(key, '');
        } else if (
            type === 'session.text.delta' ||
            type === 'session.text.ended'
        ) {
            const root = this.aliases.get(p.assistantMessageID);
            if (!root) return;
            const key = p.assistantMessageID + ':' + p.ordinal,
                previous = this.parts.get(key) ?? '';
            let delta;
            if (type.endsWith('.delta')) {
                delta = p.delta;
                this.parts.set(key, previous + delta);
            } else {
                delta = p.text.startsWith(previous)
                    ? p.text.slice(previous.length)
                    : '';
                this.parts.set(key, p.text);
            }
            if (delta)
                this.emit({
                    AgentMessageDelta: {
                        session_id: sid,
                        turn_id: root,
                        item_id: root + ':assistant',
                        delta,
                    },
                });
        } else if (type.startsWith('session.tool.')) {
            const root = this.aliases.get(p.assistantMessageID);
            if (!root) return;
            const terminal =
                    type.endsWith('.success') || type.endsWith('.failed'),
                key =
                    p.assistantMessageID +
                    ':' +
                    p.id +
                    ':' +
                    (terminal ? 'completed' : 'started');
            if (this.parts.has(key)) return;
            this.parts.set(key, true);
            if (terminal)
                this.emit({
                    ItemCompleted: {
                        session_id: sid,
                        turn_id: root,
                        item: {
                            id: p.id,
                            kind: 'tool',
                            text: p.metadata?.title ?? p.id,
                            status: type.endsWith('.failed')
                                ? 'error'
                                : 'completed',
                        },
                    },
                });
            else
                this.emit({
                    ItemStarted: {
                        session_id: sid,
                        turn_id: root,
                        item_id: p.id,
                        kind: 'tool',
                        label: p.name ?? p.id,
                    },
                });
        } else if (type === 'session.execution.succeeded')
            await this.finish('completed');
        else if (type === 'session.execution.failed')
            await this.finish('failed', p.error);
        else if (
            type === 'session.execution.interrupted' &&
            p.reason !== 'shutdown'
        )
            await this.finish('interrupted');
        else if (type === 'session.deleted') {
            this.emit({ SessionExited: { session_id: sid } });
            await this.transport.close();
        } else if (type === 'session.renamed') this.session.title = p.title;
        else if (type === 'permission.asked') this.interaction('permission', p);
        else if (type === 'form.created') this.interaction('form', p.form);
        else if (
            ['permission.replied', 'form.replied', 'form.cancelled'].includes(
                type,
            )
        ) {
            const id = p.requestID ?? p.id;
            this.pending.delete(id);
            this.emit({
                InteractionResolved: { session_id: sid, request_id: id },
            });
        }
    }
    async finish(status, error) {
        if (!this.activeTurns.size) return;
        const instance = this.instance,
            active = [...this.activeTurns];
        let history = { turns: [] };
        try {
            history = await this.history({
                limit: Math.max(20, Math.min(100, active.length)),
            });
        } catch (failure) {
            if (instance !== this.instance) return;
            error ??= failure;
            status = 'failed';
            this.queue.pause();
        }
        if (instance !== this.instance) return;
        for (const turn of active) {
            const summary = history.turns.find((item) => item.id === turn);
            this.emit({
                ItemCompleted: {
                    session_id: this.session.id,
                    turn_id: turn,
                    item: {
                        id: turn + ':assistant',
                        kind: 'agentMessage',
                        text: summary?.agent_text ?? '',
                        status,
                    },
                },
            });
            this.emit({
                TurnCompleted: {
                    session_id: this.session.id,
                    turn_id: turn,
                    status,
                    error: error ? JSON.stringify(error) : null,
                },
            });
            this.activeTurns.delete(turn);
        }
        this.turn = undefined;
        this.native.current = undefined;
        this.parts.clear();
        this.queue.finish(status === 'interrupted');
        setImmediate(() => this.pump().catch((error) => this.report(error)));
    }
    async pump() {
        if (this.closed || this.busy() || this.turn) return;
        const item = this.queue.peek();
        if (!item) return;
        this.queue.claim();
        await this.send(item.id, item.text, true);
    }
    async send(id, text, queued = false) {
        const turn = messageId(),
            instance = this.instance;
        if (!queued)
            this.queue.remember(
                id,
                { turn_id: turn },
                { id, text },
                { method: 'prompt', text },
            );
        this.startUser(turn);
        try {
            const result = await this.sdk(this.client.session.prompt, {
                sessionID: this.session.id,
                id: turn,
                text,
            });
            if (result.id !== turn)
                throw new Error('Native prompt identity changed');
        } catch (error) {
            if (instance === this.instance) this.queue.markUncertain();
            throw failure(
                'delivery_uncertain',
                'Prompt delivery is uncertain: ' + error.message,
            );
        }
        if (instance !== this.instance)
            throw failure(
                'session_changed',
                'OpenCode session changed during delivery',
            );
        return { turn_id: turn };
    }
    async command(method, params = {}) {
        if (this.closed || !this.session)
            throw failure(
                'session_changed',
                'Original OpenCode session is unavailable',
            );
        if (method === 'info') return this.info();
        if (method === 'history') return this.history(params);
        if (method === 'snapshot')
            return {
                ...this.info(),
                ...(await this.history(params)),
                queue: this.queue.view(),
            };
        if (method === 'queue_state') return this.queue.view();
        if (method === 'queue') {
            const item = this.queue.enqueue(params.request_id, params.text);
            setImmediate(() =>
                this.pump().catch((error) => this.report(error)),
            );
            return item;
        }
        if (method === 'queue_resume') {
            this.queue.resume();
            await this.pump();
            return { message: 'Queue resumed' };
        }
        if (method === 'queue_clear') {
            if (!this.busy() && this.queue.view().uncertain) {
                this.turn = undefined;
                this.activeTurns.clear();
            }
            this.queue.clear(!this.busy());
            return { message: 'Queue cleared' };
        }
        if (method === 'prompt') {
            if (
                !params.request_id ||
                typeof params.text !== 'string' ||
                !params.text.trim()
            )
                throw failure(
                    'invalid_request',
                    'Prompt and request_id are required',
                );
            const receipt = this.queue.receipt(params.request_id, {
                method,
                text: params.text,
            });
            if (receipt) return receipt;
            if (this.queue.view().uncertain)
                throw failure(
                    'delivery_uncertain',
                    'Resolve the uncertain delivery before sending again',
                );
            if (this.busy() || this.turn)
                throw failure('busy', 'Session is busy; queue the prompt');
            return this.send(params.request_id, params.text);
        }
        if (method === 'stop') {
            this.queue.pause();
            await this.sdk(this.client.session.interrupt, {
                sessionID: this.session.id,
                resume: false,
            });
            return {};
        }
        if (method === 'interaction') return this.reply(params);
        if (method === 'command')
            return this.control(params.name, params.value);
        throw failure(
            'unsupported_method',
            'Unsupported OpenCode operation: ' + method,
        );
    }
    async reply(params) {
        const pending = this.pending.get(params.rpc_id);
        if (!pending)
            throw failure(
                'invalid_request',
                'Interaction is no longer pending',
            );
        const { kind, request, supported } = pending,
            sessionID = this.session.id;
        // Native rejection feedback denies the request while allowing a normal
        // assistant response. Stop is the separate whole-execution interrupt.
        if (kind === 'permission') {
            const decision = params.response.decision;
            if (!['once', 'always', 'reject'].includes(decision))
                throw failure('invalid_request', 'Invalid permission decision');
            await this.sdk(this.client.permission.reply, {
                sessionID,
                requestID: request.id,
                decision,
                ...(decision === 'reject'
                    ? { message: 'Rejected by user from Agentix.' }
                    : {}),
            });
        } else if (
            params.response.decision === 'reject' ||
            params.response.cancelled
        )
            await this.sdk(this.client.session.form.cancel, {
                sessionID,
                formID: request.id,
                message: 'Rejected by user from Agentix.',
            });
        else {
            if (!supported)
                throw failure(
                    'unsupported_method',
                    'Complete this form in the native terminal or reject it',
                );
            const answer = {};
            for (const field of request.fields) {
                if (field.hidden) {
                    if (field.default !== undefined)
                        answer[field.key] = field.default;
                    continue;
                }
                const values = params.response.answers?.[field.key]?.answers;
                if (
                    !Array.isArray(values) ||
                    values.some((v) => typeof v !== 'string') ||
                    (field.type === 'string' && values.length !== 1)
                )
                    throw failure('invalid_request', 'Invalid form answer');
                const mapped = values.map((value) => {
                    const option = field.options?.find(
                        (option) => option.label === value,
                    );
                    if (option) return option.value;
                    if (field.options?.length && !field.custom)
                        throw failure('invalid_request', 'Invalid form option');
                    return value;
                });
                answer[field.key] =
                    field.type === 'multiselect' ? mapped : mapped[0];
            }
            await this.sdk(this.client.session.form.reply, {
                sessionID,
                formID: request.id,
                answer,
            });
        }
        this.pending.delete(request.id);
        this.emit({
            InteractionResolved: {
                session_id: sessionID,
                request_id: request.id,
            },
        });
        return {};
    }
    async control(name, value) {
        const sessionID = this.session.id,
            result = (body, choices = []) => ({ body, choices });
        if (name === 'status')
            return result(
                'Session: ' +
                    sessionID +
                    '\nState: ' +
                    (this.busy() ? 'running' : 'idle') +
                    '\nQueue: ' +
                    this.queue.view().items.length,
            );
        if (name === 'rename') {
            if (typeof value !== 'string' || !value.trim())
                throw failure('invalid_request', 'A session name is required');
            await this.sdk(this.client.session.update, {
                sessionID,
                title: value,
            });
            this.session.title = value;
            return result('Renamed to ' + value);
        }
        if (name === 'model') {
            const data = await this.sdk(this.client.model.list, {
                location: this.session.location,
            });
            const choices = data.data
                .filter((model) => model.enabled !== false)
                .map((model) => ({
                    label: model.providerID + '/' + model.id,
                    value: model.providerID + '/' + model.id,
                }));
            if (value == null)
                return result(
                    'Select the model for this native session.',
                    choices,
                );
            if (!choices.some((choice) => choice.value === value))
                throw failure('invalid_request', 'Model is unavailable');
            const slash = value.indexOf('/'),
                model = {
                    providerID: value.slice(0, slash),
                    id: value.slice(slash + 1),
                };
            await this.sdk(this.client.session.switchModel, {
                sessionID,
                model,
            });
            this.model = model;
            this.session.model = model;
            return result('Model: ' + value);
        }
        if (name === 'compact') {
            if (this.busy() || this.compacting)
                throw failure(
                    'busy',
                    'Wait for the active turn or compaction to finish',
                );
            this.compacting = true;
            try {
                await this.sdk(this.client.session.compact, { sessionID });
                return result('Context compaction requested');
            } finally {
                this.compacting = false;
            }
        }
        if (name === 'diff')
            return result(
                JSON.stringify(
                    await this.sdk(this.client.session.diff, { sessionID }),
                    null,
                    2,
                ),
            );
        if (name === 'skills') {
            const data = await this.sdk(this.client.skill.list, {
                location: this.session.location,
            });
            return result(
                data.data
                    .map(
                        (skill) =>
                            skill.name + ' — ' + (skill.description || ''),
                    )
                    .join('\n') || 'No skills reported by OpenCode',
            );
        }
        if (name === 'new') {
            if (this.switching)
                return result('Session switch already requested');
            this.switching = true;
            this.queue.pause();
            this.emit({
                SessionSwitchStarted: {
                    session_id: sessionID,
                    client_id: this.clientId,
                },
            });
            setImmediate(async () => {
                try {
                    await this.command('stop');
                    const session = await this.sdk(this.client.session.create, {
                        location: this.session.location,
                        model: this.session.model,
                    });
                    this.api.ui.router.navigate({
                        type: 'session',
                        sessionID: session.id,
                    });
                    await this.open(session.id, sessionID);
                } catch (error) {
                    this.emit({
                        SessionSwitchFailed: {
                            session_id: sessionID,
                            client_id: this.clientId,
                            reason: error.message,
                        },
                    });
                    this.report(error);
                } finally {
                    this.switching = false;
                }
            });
            return result('Session switch requested');
        }
        throw failure(
            'unsupported_command',
            'Unsupported OpenCode command: ' + name,
        );
    }
}
class PartCache extends Map {
    bytes = 0;
    weight(value) {
        return typeof value === 'string' ? Buffer.byteLength(value) : 0;
    }
    set(id, value) {
        if (super.has(id)) this.bytes -= this.weight(super.get(id));
        super.delete(id);
        super.set(id, value);
        this.bytes += this.weight(value);
        while (this.size > 8192 || this.bytes > 8 * 1024 * 1024)
            this.delete(this.keys().next().value);
        return this;
    }
    delete(id) {
        if (this.has(id)) this.bytes -= this.weight(this.get(id));
        return super.delete(id);
    }
    clear() {
        super.clear();
        this.bytes = 0;
    }
}
