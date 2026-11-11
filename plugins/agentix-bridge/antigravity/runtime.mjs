import { createHash } from 'node:crypto';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { BridgeTransport } from '../transport.mjs';
import { atomicJson, readJson } from '../claude/mailbox.mjs';
import { AntigravityMailbox, dataRoot } from './mailbox.mjs';
import { AntigravitySession } from './session.mjs';

/** One bridge per native CLI process; conversation changes replace the registration. */
export function createRuntime(options = {}) {
    const root = options.root ?? dataRoot();
    const mailbox = options.mailbox ?? new AntigravityMailbox(root);
    const factory = options.transportFactory ?? (settings => new BridgeTransport(settings));
    const clientId = options.clientId ?? String(process.ppid);
    const directory = join(root, 'sessions');
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    let session, transport, closed = false, pending = Promise.resolve();
    const retiring = new Set();
    const dispatch = action => {
        const result = pending.then(action);
        pending = result.catch(() => {});
        return result;
    };
    async function activate(identity) {
        if (session?.identity.conversationId === identity.conversationId) return;
        const previous = session?.identity.conversationId ?? identity.previous_session_id;
        const prior = session, priorTransport = transport;
        const carry = (prior?.awaitingNewInput || prior?.identity.provisional) ? [...prior.receipts].filter(([, receipt]) => ['pending', 'uncertain'].includes(receipt.state)) : [];
        let nextTransport;
        const key = createHash('sha256').update(JSON.stringify([identity.conversationId, identity.transcriptPath])).digest('hex');
        const path = join(directory, `${key}.json`);
        const saved = readJson(path) ?? {};
        if (carry.length) saved.receipts = [...new Map([...carry, ...(saved.receipts ?? [])])];
        const next = new AntigravitySession({
            saved, persist: value => atomicJson(path, value),
            delivery: options.delivery,
            controls: options.controls,
            interactions: options.interactions,
            nativeNew: options.nativeNew,
            usage: (id, value) => nextTransport?.usage(id, value),
            event: event => nextTransport?.event(event), sequence: () => nextTransport?.sequence ?? 0,
        });
        next.hook({ ...identity, hook_event_name: 'SessionStart' });
        if (carry.length) next.commit(next.turns, next.active);
        if (prior) {
            const awaiting = prior.awaitingNewInput;
            prior.awaitingNewInput = false;
            try { prior.commit(prior.turns, prior.active); }
            catch (error) { prior.awaitingNewInput = awaiting; throw error; }
            priorTransport.event({ SessionSwitchStarted: { session_id: prior.identity.conversationId, client_id: clientId } });
            if (carry.length && prior.waiters.size) {
                prior.retiring = true;
                retiring.add({ session: prior, transport: priorTransport, replacement: identity.conversationId });
            } else { prior.close(); await priorTransport.close(true); }
        }
        session = next;
        transport = nextTransport = factory({
            endpoint: options.endpoint,
            registered: result => {
                options.delivery?.configure?.(result);
                setImmediate(() => { if (!closed && session === next) next.replayInteraction(); });
            },
            snapshot: () => next.info(),
            handle: (method, params) => next.request(method, params),
            // Prompt acknowledgements arrive through poll; do not hold its queue while waiting.
            dispatch: action => Promise.resolve().then(action),
            registrationError: options.onError,
        });
        transport.open({ agent: 'antigravity', instance: next.instance,
            pid: options.pid ?? process.ppid, client_id: clientId,
            session_id: identity.conversationId, cwd: identity.cwd ?? identity.workspacePaths[0],
            session_file: identity.transcriptPath,
            ...(previous ? { previous_session_id: previous } : {}),
        });
    }
    async function settleRetiring() {
        for (const entry of retiring) {
            if (entry.replacement === session?.identity.conversationId) entry.session.acknowledgeHandoff(session.receipts);
            else entry.session.close();
            if (entry.session.waiters.size) continue;
            retiring.delete(entry);
            // Let the old request handler serialize its acknowledged reply before ending its socket.
            await new Promise(resolve => setImmediate(resolve));
            entry.session.close(); await entry.transport.close(true);
        }
    }
    return {
        poll: () => dispatch(async () => {
            if (closed) return;
            await mailbox.consume(async event => {
                await activate(event);
                session.hook(event);
                await settleRetiring();
            });
            const identity = mailbox.identity();
            if (identity) await activate(identity);
            else if (!session) {
                // agy creates its native conversation only on the first input.
                // Keep an attachable process identity until that hook arrives.
                const id = `${createHash('sha256').update(clientId).digest('hex')}-pending`;
                const cwd = options.cwd ?? options.resolveCwd?.() ?? process.cwd();
                const sessionRoot = options.sessionRoot ?? process.env.AGENTIX_ANTIGRAVITY_SESSION_DIR
                    ?? join(homedir(), '.gemini/antigravity-cli/brain');
                // A virtual path within the configured native root; no trajectory is written.
                await activate({ conversationId: id, workspacePaths: [cwd], cwd,
                    transcriptPath: join(sessionRoot, '.agentix-pending', `${id}.jsonl`),
                    provisional: true, name: `New session (PID ${options.pid ?? process.ppid})` });
            }
            await session?.pollInteraction();
            await settleRetiring();
        }),
        close: () => dispatch(async () => {
            closed = true;
            session?.close();
            await transport?.close();
            for (const entry of retiring) { entry.session.close(); await entry.transport.close(); }
            retiring.clear();
        }),
    };
}
