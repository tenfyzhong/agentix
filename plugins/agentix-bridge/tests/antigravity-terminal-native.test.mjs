import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { rexCandidates } from '../claude/rex.mjs';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, writeFile, rm, readdir, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { NativeSessionControl } from '../new-session.mjs';
import { AntigravityInteractions, parseInteraction } from '../antigravity/interactions.mjs';
import { AntigravityDelivery } from '../antigravity/delivery.mjs';
import { TerminalAdapter } from '../claude/terminal.mjs';
import { AntigravityControls, parseModelMenu, parseEffortMenu } from '../antigravity/controls.mjs';
const run = promisify(execFile);
const kind = process.env.AGENTIX_TEST_ANTIGRAVITY_TERMINAL;
const liveApproval = process.env.AGENTIX_TEST_ANTIGRAVITY_APPROVAL === '1';
const liveQuestions = process.env.AGENTIX_TEST_ANTIGRAVITY_QUESTIONS === '1';
const liveFile = ['1', 'edit'].includes(process.env.AGENTIX_TEST_ANTIGRAVITY_FILE);
const editFile = process.env.AGENTIX_TEST_ANTIGRAVITY_FILE === 'edit';
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";

test('native_antigravity_terminal_delivery_and_controls', { skip: !kind, timeout: 90000 }, async t => {
    assert.ok(['tmux', 'rmux', 'rex'].includes(kind));
    assert.ok(process.env.AGENTIX_TEST_ANTIGRAVITY_NEW !== '1' || liveApproval || liveQuestions, 'Native new-session acceptance requires a normally completed inference scenario');
    const root = await mkdtemp(join(tmpdir(), 'agy-terminal-'));
    const socket = join(root, 'mux.sock'), workspace = join(root, 'workspace'), data = join(root, 'bridge');
    const mux = args => run(kind, ['-S', socket, ...args], { timeout: 3000 });
    let cleanup = async () => {};
    t.after(async () => { await cleanup(); await rm(root, { recursive: true, force: true }); });
    await mkdir(join(workspace, '.agents'), { recursive: true });
    if (editFile) await writeFile(join(workspace, 'fixture.txt'), 'previous fixture\n');
    const hook = fileURLToPath(new URL('../antigravity/hook.mjs', import.meta.url));
    const terminate = fileURLToPath(new URL('./fixtures/antigravity-hook.mjs', import.meta.url));
    const command = `${quote(process.execPath)} ${quote(hook)}`;
    const record = { type: 'command', command: `${quote(process.execPath)} ${quote(terminate)} Record` };
    await writeFile(join(workspace, '.agents/hooks.json'), JSON.stringify({ bridge: {
        SessionStart: [{ type: 'command', command }, record],
        PreInvocation: [{ type: 'command', command }, record, ...((liveApproval || liveQuestions || liveFile) ? [] : [{ type: 'command', command: `${quote(process.execPath)} ${quote(terminate)} PreInvocation` }])],
        Stop: [{ type: 'command', command }, record],
    } }));
    const settings = JSON.stringify({ toolPermission: 'strict', enableTerminalSandbox: false });
    const launch = `exec env AGENTIX_ANTIGRAVITY_DATA_DIR=${quote(data)} ANTIGRAVITY_TEST_EVENTS=${quote(join(root, 'events.jsonl'))} ${quote(process.env.AGENTIX_TEST_AGY ?? 'agy')} --settings ${quote(settings)}`;
    let captureScreen, sendKey, terminalOptions;
    if (kind === 'rex') {
        const executable = rexCandidates(process.env).find(existsSync);
        assert.ok(executable, 'Rex CLI must be installed');
        const rex = async args => (await run(executable, ['--autostart=false', '--timeout', '4s', ...args], { timeout: 5000 })).stdout;
        const identity = JSON.parse(await rex(['whoami', '--json']));
        const created = JSON.parse(await rex(['-S', identity.endpoint, 'new', `agy-integration-${process.pid}`, '-c', workspace, '--json', '--shell=none', '--',
            '/usr/bin/env', `AGENTIX_ANTIGRAVITY_DATA_DIR=${data}`, `ANTIGRAVITY_TEST_EVENTS=${join(root, 'events.jsonl')}`, process.env.AGENTIX_TEST_AGY ?? '/opt/homebrew/bin/agy', '--settings', settings]));
        const session = created.session_id, block = created.initial_windows[0].block_ids[0];
        cleanup = () => rex(['-S', identity.endpoint, 'kill', session]);
        captureScreen = () => rex(['-S', identity.endpoint, 'capture', '-s', session, '-b', block, '--trim']);
        sendKey = key => rex(['-S', identity.endpoint, 'send-key', '-s', session, '-b', block, key]);
        terminalOptions = async () => {
            const state = JSON.parse(await rex(['-S', identity.endpoint, 'block', 'call', '-s', session, '-b', block, 'process']));
            return { env: { ...process.env, REX_SERVER: identity.endpoint, REX_SESSION: session, REX_BLOCK: block }, pid: state.foreground.pid, command: executable };
        };
    } else {
        await mux(['-f', '/dev/null', 'new-session', '-d', '-x', '100', '-y', '35', '-c', workspace, launch]);
        cleanup = () => mux(['kill-server']);
        captureScreen = async () => (await mux(['capture-pane', '-p'])).stdout;
        sendKey = key => mux(['send-keys', key]);
        terminalOptions = async () => {
            const [pane, pid] = (await mux(['display-message', '-p', '#{pane_id}|#{pane_pid}'])).stdout.trim().split('|');
            return { env: { TMUX: socket + ',1,0', TMUX_PANE: pane }, pid: Number(pid) };
        };
    }
    async function waitFor(action) {
        for (let i = 0; i < 600; i++) {
            const result = await action(); if (result) return result;
            await new Promise(resolve => setTimeout(resolve, 50));
        }
        t.diagnostic((await captureScreen()).split('\n').filter(line => !line.includes('@')).join('\n'));
        throw new Error('Native terminal did not become ready');
    }
    await waitFor(async () => {
        const screen = await captureScreen();
        if (screen.includes('Do you trust the contents of this project?')) { await sendKey('Enter'); return false; }
        if (screen.includes('Terms of Service & Data Use')) throw new Error('Complete agy onboarding before running native terminal acceptance');
        return /^(?:-- INSERT -- )?\? for shortcuts[^\S\n]{2,}\S[^\n]*$/m.test(screen);
    });
    const terminal = new TerminalAdapter(kind, await terminalOptions());
    const delivery = new AntigravityDelivery({ resolve: async () => terminal });
    const readDraft = delivery.draft.bind(delivery);
    delivery.draft = async (...args) => {
        try { return await readDraft(...args); }
        catch (error) { t.diagnostic(JSON.stringify(await delivery.verify(terminal))); t.diagnostic((await terminal.capture()).split('\n').filter(line => !line.includes('@')).join('\n')); throw error; }
    };
    await delivery.check(terminal);
    await terminal.sendText('Agentix draft fixture');
    await new Promise(resolve => setTimeout(resolve, 150));
    assert.equal(await delivery.draft(), 'Agentix draft fixture');
    assert.equal(await delivery.draft('outdated draft'), 'Agentix draft fixture');
    try { assert.equal(await delivery.draft('Agentix draft fixture'), null); }
    catch (error) { await delivery.verify(terminal); t.diagnostic((await terminal.capture()).split('\n').filter(line => !line.includes('@')).join('\n')); throw error; }
    const controls = new AntigravityControls(delivery);
    const commandControl = controls.command.bind(controls);
    controls.command = async (...args) => {
        try { return await commandControl(...args); }
        catch (error) { await delivery.verify(terminal); t.diagnostic((await terminal.capture()).split('\n').filter(line => !line.includes('@')).join('\n')); throw error; }
    };
    const capture = terminal.capture.bind(terminal);
    let originalModel, originalEffort;
    terminal.capture = async (...args) => {
        const screen = await capture(...args), model = parseModelMenu(screen), effort = parseEffortMenu(screen);
        if (model) originalModel ??= model.choices[model.selected];
        if (effort) originalEffort ??= effort.choices[effort.selected];
        return screen;
    };
    const modelChoices = (await controls.command('model')).choices;
    assert.ok(modelChoices.length > 0);
    await new Promise(resolve => setTimeout(resolve, 150));
    assert.ok(originalModel);
    assert.match((await controls.command('model', originalModel)).body, /Model:/);
    assert.ok((await controls.command('reasoning')).choices.length > 0);
    await new Promise(resolve => setTimeout(resolve, 150));
    assert.ok(originalEffort);
    assert.equal((await controls.command('reasoning', originalEffort)).body, `Reasoning: ${originalEffort}`);
    if (process.env.AGENTIX_TEST_ANTIGRAVITY_CHANGE_MODEL === '1') {
        const alternate = modelChoices.find(choice => choice.value !== originalModel)?.value;
        assert.ok(alternate, 'Native model acceptance requires two available models');
        try {
            assert.equal((await controls.command('model', alternate)).body, `Model: ${alternate}`);
            const efforts = (await controls.command('reasoning')).choices;
            for (const effort of efforts) {
                assert.equal((await controls.command('reasoning', effort.value)).body, `Reasoning: ${effort.value}`);
            }
        } finally {
            await controls.command('model', originalModel);
            await controls.command('reasoning', originalEffort);
        }
    }
    await delivery.check(terminal);
    const prompt = liveFile
        ? `For this isolated UI integration test, use your file ${editFile ? 'editing tool to replace the contents of' : 'writing tool to create'} fixture.txt ${editFile ? 'with' : 'containing'} exactly agentix-file-fixture. Do not run terminal commands or modify any other file.`
        : liveQuestions
        ? 'This is an isolated UI integration test. Use the ask_question tool with two questions: Pick one color, options Red and Blue, is_multi_select false; Pick features, options Logs and Metrics, is_multi_select true. Do not run commands or modify files. Wait for my tool answers.'
        : liveApproval
        ? 'For this isolated integration test, use your terminal tool to execute /usr/bin/printf agentix-approval-fixture. Do not modify files.'
        : 'Agentix isolated native terminal fixture';
    await delivery.send({ text: prompt });
    if (liveFile) {
        let lastFingerprint, acceptedWrite = false;
        await waitFor(async () => {
            const hosts = await readdir(join(data, 'hosts')).catch(() => []);
            for (const host of hosts) {
                const identity = JSON.parse(await readFile(join(data, 'hosts', host, 'identity.json'), 'utf8'));
                if (identity.hookEventName === 'Stop' && identity.fullyIdle) {
                    assert.equal(acceptedWrite, true);
                    assert.equal((await readFile(join(workspace, 'fixture.txt'), 'utf8')).trim(), 'agentix-file-fixture');
                    return true;
                }
            }
            await delivery.verify(terminal);
            const screen = await terminal.capture();
            if (!/esc to cancel/.test(screen) || !/^> 1\. /m.test(screen)) { lastFingerprint = null; return false; }
            const approval = parseInteraction(screen);
            if (!approval) t.diagnostic(screen.split('\n').filter(line => !line.includes('@')).join('\n'));
            assert.equal(approval?.kind, 'file_approval');
            if (approval.fingerprint !== lastFingerprint) {
                await new AntigravityInteractions(delivery).respond(approval, { decision: approval.choices[0] });
                lastFingerprint = approval.fingerprint;
                acceptedWrite ||= ['Allow creation of this file?', 'Accept this file edit?'].includes(approval.title);
            }
            return false;
        });
    }
    if (liveApproval) {
        const interactions = new AntigravityInteractions(delivery);
        const approval = await waitFor(() => interactions.inspect());
        if (approval.kind !== 'command_approval') t.diagnostic(JSON.stringify(approval));
        assert.equal(approval.kind, 'command_approval');
        assert.match(approval.detail, /printf agentix-approval-fixture/);
        await interactions.respond(approval, { decision: 'Yes, run command' });
    }
    if (liveQuestions) {
        const interactions = new AntigravityInteractions(delivery);
        const first = await waitFor(() => interactions.inspect());
        assert.equal(first.kind, 'user_input');
        assert.equal(first.multiple, false);
        await interactions.respond(first, { answers: { [first.questionId]: { answers: ['Blue'] } } });
        const second = await waitFor(async () => {
            const current = await interactions.inspect();
            return current?.kind === 'user_input' && current.questionId !== first.questionId ? current : null;
        });
        assert.equal(second.multiple, true);
        try { await interactions.respond(second, { answers: { [second.questionId]: { answers: ['Metrics', 'Traces'] } } }); }
        catch (error) { await delivery.verify(terminal); const screen = await terminal.capture(); t.diagnostic(screen.slice(screen.lastIndexOf('\nQuestion\n'))); throw error; }
    }
    const events = await waitFor(async () => {
        // The installed sidecar consumes mailbox files; keep an independent hook witness.
        const contents = await readFile(join(root, 'events.jsonl'), 'utf8').catch(() => '');
        const values = contents.trim().split('\n').filter(Boolean).map(line => JSON.parse(line).event);
        const completed = liveApproval || liveQuestions || liveFile
            ? values.some(value => value.hookEventName === 'Stop' && value.fullyIdle)
            : values.some(value => value.hookEventName === 'PreInvocation');
        return completed ? values : null;
    });
    t.diagnostic(JSON.stringify(events.map(({ hookEventName, terminationReason, fullyIdle, initialNumSteps, invocationNum }) => ({ hookEventName, terminationReason, fullyIdle, initialNumSteps, invocationNum }))));
    const pre = events.find(event => event.hookEventName === 'PreInvocation');
    assert.equal(pre.lastUserInput, prompt);
    assert.equal(pre.conversationId, events.find(event => event.hookEventName === 'SessionStart').conversationId);
    if (process.env.AGENTIX_TEST_ANTIGRAVITY_NEW === '1') {
        try { await waitFor(async () => { try { await delivery.check(terminal); return true; } catch { return false; } }); }
        catch (error) { await delivery.verify(terminal); t.diagnostic((await terminal.capture()).split('\n').filter(line => !line.includes('@')).join('\n')); throw error; }
        await new NativeSessionControl('antigravity', { resolve: async () => terminal, ready: value => delivery.check(value), stop: () => delivery.stop() }).newSession(() => false);
        // agy allocates the next conversation lazily, on its first input.
        await waitFor(async () => { try { await delivery.check(terminal); return true; } catch { return false; } });
        await delivery.send({ text: 'Reply with ACK only. Do not call tools.' });
        await waitFor(async () => {
            for (const host of await readdir(join(data, 'hosts'))) {
                const identity = JSON.parse(await readFile(join(data, 'hosts', host, 'identity.json'), 'utf8'));
                if (identity.conversationId !== pre.conversationId && identity.hookEventName === 'Stop' && identity.fullyIdle === true) return true;
            }
            return false;
        });
        t.diagnostic('Verified native new session identity on its first input');
    }
    t.diagnostic(`Verified ${kind} menus, paste, submit, native hook identity${liveFile ? ', and live file approval' : liveQuestions ? ', and live single/multiple/custom answers' : liveApproval ? ', and live command approval' : ' without model inference'}`);
});
