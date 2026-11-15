import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { runHook, registerExtension, trackingEnabled } from '../runtime.mjs';
import { runTaskix, buildArgs } from '../taskix-cli.mjs';
import { transcriptConversation } from '../conversation.mjs';

export function hostKey(pid = process.ppid) {
    const started = execFileSync('ps', ['-p', String(pid), '-o', 'lstart='], { encoding: 'utf8' }).trim();
    if (!started) throw new Error('Antigravity parent process is unavailable');
    return `${pid}-${started}`;
}
export const dataRoot = () => process.env.TASKIX_ANTIGRAVITY_DATA_DIR ?? join(homedir(), '.local/share/taskix/antigravity');

export function createAntigravityTool(options = {}) {
    let tool;
    registerExtension({ on() {}, registerTool: value => { tool = value; } }, 'antigravity', options.runner ?? runTaskix,
        undefined, undefined, options.routing);
    return async (requestId, params, identity, signal) => {
        if (!identity?.session_id || !identity.cwd || identity.ready !== true) throw new Error('Antigravity identity is not available yet');
        buildArgs(params?.args);
        return tool.execute(requestId, params, signal, undefined, {
            cwd: identity.cwd, turnId: identity.turn_id,
            sessionManager: { getSessionId: () => identity.session_id },
        });
    };
}

/** Native hook processes share only identity and routing receipts with their MCP child. */
export function createAntigravityHooks(options = {}) {
    const root = options.root ?? dataRoot();
    const path = join(root, `${createHash('sha256').update(options.key ?? hostKey()).digest('hex')}.json`);
    const hookRunner = options.hookRunner ?? runHook, runner = options.runner ?? runTaskix;
    let pending = Promise.resolve();
    const identity = async () => {
        try { return JSON.parse(await readFile(path, 'utf8')); }
        catch (error) { if (error.code === 'ENOENT') return null; throw error; }
    };
    async function save(value) {
        await mkdir(root, { recursive: true, mode: 0o700 });
        const temporary = `${path}.${randomUUID()}.tmp`;
        try {
            await writeFile(temporary, JSON.stringify(value), { mode: 0o600, flag: 'wx' });
            await rename(temporary, path);
        } finally { await rm(temporary, { force: true }); }
    }
    async function handle(value) {
        const receivedAt = Date.now();
        if (typeof value.conversationId !== 'string' || !value.conversationId ||
            !Array.isArray(value.workspacePaths) || !value.workspacePaths.length ||
            value.workspacePaths.some(path => typeof path !== 'string' || !isAbsolute(path)) ||
            typeof value.transcriptPath !== 'string' || !isAbsolute(value.transcriptPath)) {
            throw new Error('Invalid Antigravity hook identity');
        }
        const names = ['SessionStart', 'PreInvocation', 'PostInvocation', 'PreToolUse', 'PostToolUse', 'Stop'];
        if (!names.includes(value.hookEventName)) throw new Error('Unsupported Antigravity hook event');
        const event = { session_id: value.conversationId, cwd: value.workspacePaths[0], transcript_path: value.transcriptPath };
        const scopedRunner = (args, scoped) => runner(args, { ...scoped, executor: `agent:antigravity:${scoped.session}` });
        const invoke = (name, extra = {}) => hookRunner({ ...event, hook_event_name: name, ...extra }, scopedRunner, options.routing);
        const previous = await identity();
        const same = previous?.session_id === event.session_id;
        const state = { ...(same ? previous : {}), ...event, ready: false };
        if (previous && !same) state.previous_session = {
            session_id: previous.session_id, cwd: previous.cwd, transcript_path: previous.transcript_path,
        };
        await save(state);
        if (state.previous_session) {
            await hookRunner({ ...state.previous_session, hook_event_name: 'SessionEnd' }, scopedRunner, options.routing);
            delete state.previous_session;
        }
        const contexts = [];
        // Allow delegates to native permission policy; it does not auto-approve tools.
        let decision = value.hookEventName === 'PreToolUse' ? { decision: 'allow' } : undefined;
        const collect = result => {
            const content = result?.hookSpecificOutput?.additionalContext;
            if (content) contexts.push(content);
        };
        if (!await trackingEnabled({cwd:event.cwd,session:event.session_id}, scopedRunner)) {
            const reason = String(value.terminationReason ?? '').toLowerCase().replace(/^terminal_/, '');
            const interrupted = value.fullyIdle === true && ['cancelled', 'interrupted', 'user_canceled', 'user_cancelled'].includes(reason);
            const lifecycle = value.hookEventName === 'Stop' ? (interrupted ? 'Interrupt' : 'Stop') : value.hookEventName === 'SessionStart' ? 'SessionStart' : 'PostToolUse';
            await invoke(lifecycle, { willContinue: value.fullyIdle === false });
            state.ready = true;
            await save(state);
            return decision ?? {};
        }
        if (value.hookEventName === 'SessionStart') collect(await invoke('SessionStart'));
        else if (value.hookEventName === 'PreInvocation') {
            const capture = await transcriptConversation(event.transcript_path, { currentOnly: true, maxBytes: 1024 * 1024, sessionId: event.session_id });
            state.turn_id = capture.turn_id;
            state.interrupted = false;
            collect(await invoke('PreToolUse', { turn_id: state.turn_id,
                ...(state.routed_turn !== state.turn_id ? { submitted_at_ms: receivedAt } : {}) }));
            const prompt = capture.messages.find(message => message.role === 'user')?.text;
            if (state.turn_id && prompt && state.routed_turn !== state.turn_id) {
                collect(await invoke('UserPromptSubmit', { turn_id: state.turn_id, prompt }));
                state.routed_turn = state.turn_id;
            }
        } else if (value.hookEventName === 'Stop') {
            const reason = String(value.terminationReason ?? '').toLowerCase().replace(/^terminal_/, '');
            state.interrupted = value.fullyIdle === true && ['cancelled', 'interrupted', 'user_canceled', 'user_cancelled'].includes(reason);
            await invoke(state.interrupted ? 'Interrupt' : 'Stop', { turn_id: state.turn_id, willContinue: value.fullyIdle === false });
        } else {
            const result = await invoke(value.hookEventName === 'PostInvocation' ? 'PostToolUse' : value.hookEventName, { turn_id: state.turn_id });
            const notice = result?.hookSpecificOutput?.additionalContext;
            if (value.hookEventName === 'PreToolUse' && /^Human Issue work has been cancelled\./m.test(notice ?? '')) {
                decision = { decision: 'deny', reason: notice };
            }
        }
        state.ready = true;
        await save(state);
        return decision ?? (contexts.length ? { injectSteps: contexts.map(ephemeralMessage => ({ ephemeralMessage })) } : {});
    }
    return { identity, handle(value) {
        const result = pending.then(() => handle(value));
        pending = result.catch(() => {});
        return result;
    } };
}
