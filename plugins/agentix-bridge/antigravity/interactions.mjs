import { createHash, randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { failure } from '../protocol/errors.mjs';

function parseQuestion(screen) {
    const match = /(?:^|\n)Question\n─{3,}\s*\nQuestion (\d+)\/(\d+): ([^\n]+)\n\n([\s\S]+?)\nesc to cancel[^\n]*\s*$/.exec(screen);
    if (!match || +match[1] < 1 || +match[1] > +match[2]) return null;
    const content = match[4], writing = content.includes('\n  Your answer:\n');
    const parts = writing ? content.split('\n  Your answer:\n') : content.split(/\n\s*↑\/↓ Navigate · /);
    if (parts.length !== 2 || !/enter (?:Select|Write-in|Submit(?: All)?) · esc (?:Skip|Back)\s*$/.test(parts[1])) return null;
    const choices = [], checked = [], selected = [];
    let multiple = false;
    for (const line of parts[0].split('\n').filter(line => line.trim())) {
        const row = /^([> ]) (\d+)\. (?:\[([ x])\] )?(\S.*?)\s*$/.exec(line);
        if (!row || +row[2] !== choices.length + 1) return null;
        if (row[1] === '>') selected.push(choices.length);
        if (row[3] !== undefined) multiple = true;
        if (row[3] === 'x') checked.push(choices.length);
        choices.push(row[4]);
    }
    if (selected.length !== 1 || !choices.length || choices.length > 20 || new Set(choices).size !== choices.length) return null;
    const custom = choices.at(-1) === 'Write-in...';
    if (writing && (!custom || selected[0] !== choices.length - 1)) return null;
    const questionId = `question:${match[1]}`, title = match[3], kind = 'user_input';
    const fingerprint = createHash('sha256').update(JSON.stringify({ kind, questionId, total: match[2], title, choices, multiple, custom })).digest('hex');
    return { kind, title, detail: title, questionId, choices, selected: selected[0], multiple, custom, checked,
        writing, draft: writing ? parts[1].replace(/\n\s*enter (?:Submit(?: All)?) · esc Back\s*$/, '').trim() : '', fingerprint };
}

/** Recognize only complete native cards; conversation examples are not requests. */
export function parseInteraction(screen) {
    const question = parseQuestion(screen);
    if (question) return question;
    const match = /(?:^|\n)Command\n─{3,}\s*\nRequesting permission for:\n([\s\S]+?)\n\nRun this command\?\n([\s\S]+?)\n\s*↑\/↓ Navigate · tab Amend · ctrl\+g edit\/expand command\n-- INSERT -- esc to cancel[^\n]*\s*$/.exec(screen);
    const file = /(?:^|\n)Create file\n─{3,}\s*\n([\s\S]+?)\n\nAllow creation of this file\?\n([\s\S]+?)\n\s*↑\/↓ Navigate · f full diff\n-- INSERT -- esc to cancel[^\n]*\s*$/.exec(screen);
    const access = /(?:^|\n)File access\n─{3,}\s*\n([\s\S]+?)\n\nAllow access to this file\?\n([\s\S]+?)\n\s*↑\/↓ Navigate\n-- INSERT -- esc to cancel[^\n]*\s*$/.exec(screen);
    const edit = /(?:^|\n)Pending edit\n─{3,}\s*\n([\s\S]+?)\n\n\s*shift\+tab to auto-approve file edits\nAccept this file edit\?\n([\s\S]+?)\n\s*↑\/↓ Navigate · f full diff\n-- INSERT -- esc to cancel[^\n]*\s*$/.exec(screen);
    const fileCard = file ?? access ?? edit;
    if (!match && !fileCard) return null;
    const detail = fileCard ? fileCard[1].trim() : match[1].split('\n').map(line => line.trim()).join('\n');
    const rows = (fileCard ?? match)[2].split('\n').filter(line => line.trim());
    const choices = [], selection = [];
    for (const line of rows) {
        const row = /^([> ]) (\d+)\. (\S.*?)\s*$/.exec(line);
        if (!row || Number(row[2]) !== choices.length + 1) return null;
        if (row[1] === '>') selection.push(choices.length);
        choices.push(row[3]);
    }
    if (!detail || detail.length > 32768 || choices.length < 2 || choices.length > 20 || selection.length !== 1 || new Set(choices).size !== choices.length) return null;
    const kind = fileCard ? 'file_approval' : 'command_approval';
    const title = file ? 'Allow creation of this file?' : access ? 'Allow access to this file?' : edit ? 'Accept this file edit?' : 'Run this command?';
    const fingerprint = createHash('sha256').update(JSON.stringify({ kind, title, detail, choices })).digest('hex');
    return { kind, title, detail, choices, selected: selection[0], fingerprint };
}

export class AntigravityInteractions {
    constructor(delivery, { delay = sleep } = {}) { this.delivery = delivery; this.delay = delay; }
    async inspect() {
        const terminal = await this.delivery.resolve();
        await this.delivery.verify(terminal);
        return parseInteraction(await terminal.capture());
    }
    async respond(expected, response) {
        if (expected.kind === 'user_input') return this.answerQuestion(expected, response);
        const cancel = response?.cancelled === true || response?.decision === 'cancel';
        const index = expected.choices.indexOf(response?.decision);
        if (!cancel && index < 0) throw failure('invalid_request', 'Invalid Antigravity approval decision');
        const terminal = await this.delivery.resolve();
        let ready = false;
        for (let attempt = 0; attempt < 40; attempt++) {
            await this.delivery.verify(terminal);
            const current = parseInteraction(await terminal.capture());
            if (!current || current.fingerprint !== expected.fingerprint) throw failure('invalid_request', 'Antigravity interaction changed; refresh before answering');
            if (cancel) { await terminal.sendKey('Escape'); return null; }
            if (current.selected === index) {
                if (ready) { await terminal.sendKey('Enter'); return null; }
                ready = true;
            } else {
                ready = false;
                await terminal.sendKey(current.selected < index ? 'Down' : 'Up');
            }
            await this.delay(100);
        }
        throw failure('busy', 'Antigravity did not select the requested answer');
    }
    async answerQuestion(expected, response) {
        const terminal = await this.delivery.resolve();
        let checked = [...expected.checked], writing = expected.writing;
        const read = async () => {
            await this.delivery.verify(terminal);
            const current = parseInteraction(await terminal.capture());
            if (!current || current.fingerprint !== expected.fingerprint || current.writing !== writing ||
                JSON.stringify(current.checked) !== JSON.stringify(checked)) {
                throw failure('invalid_request', 'Antigravity question changed; refresh before answering');
            }
            return current;
        };
        let current = await read();
        if (response?.cancelled || ['cancel', 'reject'].includes(response?.decision)) {
            await terminal.sendKey('Escape');
            if (writing) {
                writing = false; await this.delay(100); await read(); await terminal.sendKey('Escape');
            }
            return null;
        }
        if (current.draft) throw failure('busy', 'Finish or clear the native question draft before answering from IM');
        const answers = response?.answers?.[expected.questionId]?.answers;
        if (!Array.isArray(answers) || !answers.length || (!expected.multiple && answers.length !== 1) ||
            answers.some(value => typeof value !== 'string' || !value.trim() || Buffer.byteLength(value) > 4096 || /[\x00-\x08\x0b-\x1f\x7f]/.test(value)) ||
            new Set(answers).size !== answers.length) throw failure('invalid_request', 'Invalid Antigravity question answer');
        const options = expected.custom ? expected.choices.slice(0, -1) : expected.choices;
        const custom = answers.filter(value => !options.includes(value));
        if (custom.length > 1 || (custom.length && !expected.custom)) throw failure('invalid_request', 'Invalid custom Antigravity answer');
        const navigate = async target => {
            current = await read();
            for (let attempt = 0; current.selected !== target && attempt < 20; attempt++) {
                const direction = target > current.selected ? 1 : -1, next = current.selected + direction;
                await terminal.sendKey(direction > 0 ? 'Down' : 'Up');
                await this.delay(100);
                current = await read();
                if (current.selected !== next) throw failure('busy', 'Antigravity did not acknowledge question navigation');
            }
            if (current.selected !== target) throw failure('busy', 'Antigravity question selection unavailable');
        };
        if (writing && !custom.length) throw failure('busy', 'Return to native question choices before selecting an option');
        if (expected.multiple) {
            if (writing && options.some((label, index) => answers.includes(label) !== checked.includes(index))) {
                throw failure('busy', 'Return to native question choices before changing selections');
            }
            for (let index = 0; index < options.length; index++) {
                if (answers.includes(options[index]) === checked.includes(index)) continue;
                await navigate(index);
                await terminal.sendKey('Space');
                checked = checked.includes(index) ? checked.filter(value => value !== index) : [...checked, index].sort((a, b) => a - b);
                await this.delay(100); current = await read();
            }
        }
        if (custom.length) {
            if (!writing) {
                await navigate(expected.choices.length - 1);
                await terminal.sendKey('Enter'); writing = true;
                await this.delay(100); current = await read();
            }
            if (current.draft) throw failure('busy', 'Native question draft changed before typing');
            const buffer = `agentix-agy-question-${randomUUID()}`;
            try {
                await terminal.loadPaste(buffer, custom[0]);
                current = await read();
                if (current.draft) throw failure('busy', 'Native question draft changed before typing');
                await terminal.pasteLoaded(buffer);
                await this.delay(100); current = await read();
                const normalized = value => value.trim().replace(/\s+/g, ' ');
                if (normalized(current.draft) !== normalized(custom[0])) throw failure('delivery_uncertain', 'Antigravity question input was not confirmed');
            } finally { await terminal.clearPaste(buffer).catch(() => {}); }
        } else if (!expected.multiple) await navigate(options.indexOf(answers[0]));
        await this.delay(100); await read();
        await terminal.sendKey('Enter');
        return null;
    }
}
