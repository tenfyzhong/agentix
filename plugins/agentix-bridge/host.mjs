import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { failure } from './protocol/errors.mjs';
const runFile = promisify(execFile);

/** Native differences stay here; the transport and queue do not know host names. */
export function createHostAdapter(api, kind) {
    if (!['pi', 'omp'].includes(kind)) throw new Error('Unsupported bridge host');
    const settledEvent = kind === 'pi' ? 'agent_settled' : 'agent_end';
    function capabilities(ctx) {
        const values = ['prompt', 'steer', 'history', 'stop', 'queue', 'queue_control', 'status', 'diff'];
        for (const [cap, method] of [['model', 'setModel'], ['reasoning', 'setThinkingLevel'], ['rename', 'setSessionName'], ['skills', 'getCommands']]) {
            if (typeof api[method] === 'function') values.push(cap);
        }
        if (typeof ctx.compact === 'function') values.push('compact');
        return values;
    }
    const availableModels = async ctx => await (ctx.models?.list?.() ?? ctx.modelRegistry.getAvailable());
    async function command(ctx, sessionId, queued, name, value, ensureCurrent = () => {}) {
        ensureCurrent();
        if (!capabilities(ctx).includes(name)) throw failure('unsupported_command', `Unsupported command: ${name}`);
        const result = (body, choices = []) => ({ body, choices });
        if (name === 'status') {
            const usage = ctx.getContextUsage?.();
            return result(`Session: ${api.getSessionName?.() ?? sessionId}\nWorkspace: ${ctx.cwd}\nState: ${ctx.isIdle() ? 'idle' : 'running'}\nModel: ${ctx.model?.provider ?? ''}/${ctx.model?.id ?? 'not selected'}\nReasoning: ${api.getThinkingLevel?.() ?? 'not reported'}\nContext tokens: ${usage?.tokens ?? 'not reported'}\nQueue: ${queued.items.length}${queued.paused ? ' (paused)' : ''}`);
        }
        if (name === 'model') {
            const models = await availableModels(ctx);
            ensureCurrent();
            if (value == null) return result('Select a model for subsequent turns.', models.map(m => ({ label: `${m.provider}/${m.id}`, value: `${m.provider}/${m.id}` })));
            const model = models.find(m => `${m.provider}/${m.id}` === value);
            if (!model) throw new Error('Model is not available in this host');
            if (await api.setModel(model) === false) throw new Error('Model authentication is unavailable');
            return result(`Model: ${value}`);
        }
        if (name === 'reasoning') {
            if (value == null) return result(`Current reasoning: ${api.getThinkingLevel?.() ?? 'not reported'}. Use /reasoning <level> to request a level; the host applies its model limits.`);
            if (!['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(value)) throw failure('invalid_request', 'Invalid reasoning level');
            await api.setThinkingLevel(value);
            return result(`Reasoning applied by host: ${api.getThinkingLevel?.() ?? value}`);
        }
        if (name === 'rename') { if (typeof value !== 'string' || !value.trim()) throw failure('invalid_request', 'A session name is required'); await api.setSessionName(value); return result(`Renamed to ${value}`); }
        if (name === 'compact') { if (!ctx.isIdle()) throw failure('busy', 'Wait for the active turn to finish'); await ctx.compact(); return result('Context compaction requested'); }
        if (name === 'skills') {
            const skills = (await api.getCommands()).filter(c => c.source === 'skill' || c.sourceInfo?.source === 'skill');
            return result(skills.map(c => `${c.name}${c.description ? ' — ' + c.description : ''}`).join('\n') || 'No skills reported by the host.');
        }
        if (name === 'diff') {
            const opts = { cwd: ctx.cwd, maxBuffer: 4 * 1024 * 1024, timeout: 10000 };
            const changes = await runFile('git', ['diff', '--no-ext-diff', 'HEAD', '--'], opts);
            const untracked = await runFile('git', ['ls-files', '--others', '--exclude-standard'], opts);
            return result(`${changes.stdout || 'No tracked changes.'}${untracked.stdout ? '\nUntracked files:\n' + untracked.stdout : ''}`);
        }
        throw failure('unsupported_command', `Unsupported command: ${name}`);
    }
    return { capabilities, command, models: availableModels, settledEvent,
        isSettled(name, value) { return name === settledEvent && (kind === 'pi' || (value.willContinue !== true && value.isTerminal !== false)); } };
}
