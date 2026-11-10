import { setTimeout as sleep } from 'node:timers/promises';
import { failure } from '../protocol/errors.mjs';

export function parseModelMenu(screen) {
    const block = /Switch Model\s+Search:\s*─{3,}\s*\n([\s\S]*?)\n\s*Effort\s/.exec(screen)?.[1];
    if (!block || !screen.includes('enter Select  esc Go Back')) return null;
    const rows = block.split('\n').filter(line => line.trim());
    const choices = [], selected = [];
    for (const row of rows) {
        const match = /^([> ]) (\S.*?)\s*$/.exec(row);
        if (!match) return null;
        if (match[1] === '>') selected.push(choices.length);
        choices.push(match[2].replace(/\s+\(current\)$/, ''));
    }
    if (!choices.length || selected.length !== 1 || new Set(choices).size !== choices.length) return null;
    return { choices, selected: selected[0] };
}

export function parseEffortMenu(screen) {
    if (!screen.includes('Set Effort') || !screen.includes('enter Select  esc Go Back')) return null;
    const lines = screen.split('\n'), index = lines.findIndex(line => line.includes('◉'));
    if (index < 0) return null;
    const names = [...(lines[index + 1] ?? '').matchAll(/\b(low|medium|high|xhigh|max)\b/g)];
    if (!names.length) return null;
    const position = lines[index].indexOf('◉');
    const distances = names.map(match => Math.abs(match.index + (match[0].length - 1) / 2 - position));
    const selected = distances.indexOf(Math.min(...distances));
    if (distances[selected] > 4) return null;
    return { choices: names.map(match => match[0]), selected };
}

/** Navigate the native menus; never substitute a one-turn /model prompt override. */
export class AntigravityControls {
    constructor(delivery, { delay = sleep } = {}) { this.delivery = delivery; this.delay = delay; }
    async command(name, value) {
        const model = name === 'model';
        if (!model && name !== 'reasoning') throw failure('unsupported_command', `Unsupported Antigravity command: ${name}`);
        const parse = model ? parseModelMenu : parseEffortMenu;
        const terminal = await this.delivery.resolve();
        await this.delivery.check(terminal);
        await terminal.sendText(model ? '/model' : '/effort');
        await terminal.sendKey('Enter');
        let owned = false;
        try {
            let menu;
            for (let attempt = 0; attempt < 20; attempt++) {
                await this.delay(50);
                await this.delivery.verify(terminal);
                menu = parse(await terminal.capture());
                if (menu) break;
            }
            if (!menu) throw failure('busy', 'Antigravity menu is unavailable or changed');
            owned = true;
            if (value == null) return { body: `Select ${model ? 'a model' : 'reasoning effort'} for subsequent turns.`,
                choices: menu.choices.map(label => ({ label, value: label })) };
            const target = menu.choices.indexOf(value);
            if (target < 0) throw failure('invalid_request', 'Selection is not available in Antigravity');
            while (menu.selected !== target) {
                await this.delivery.verify(terminal);
                const current = parse(await terminal.capture());
                if (!current || JSON.stringify(current) !== JSON.stringify(menu)) throw failure('busy', 'Antigravity selection changed; refresh the menu');
                const direction = target > menu.selected ? 1 : -1;
                await terminal.sendKey(model ? (direction > 0 ? 'Down' : 'Up') : (direction > 0 ? 'Right' : 'Left'));
                await this.delay(75);
                await this.delivery.verify(terminal);
                const next = parse(await terminal.capture());
                if (!next || JSON.stringify(next.choices) !== JSON.stringify(menu.choices) || next.selected !== menu.selected + direction) {
                    throw failure('busy', 'Antigravity menu did not acknowledge navigation');
                }
                menu = next;
            }
            await this.delivery.verify(terminal);
            if (JSON.stringify(parse(await terminal.capture())) !== JSON.stringify(menu)) throw failure('busy', 'Antigravity selection changed before confirmation');
            await terminal.sendKey('Enter');
            owned = false;
            for (let attempt = 0; attempt < 20; attempt++) {
                await this.delay(50);
                await this.delivery.verify(terminal);
                const screen = await terminal.capture(), footer = screen.trimEnd().split('\n').at(-1).trim();
                const expected = model ? value.replace(/\s*\([^)]*\)$/, '') : `· ${value}`;
                if (!parse(screen) && footer.includes(expected)) {
                    await this.delivery.check(terminal);
                    return { body: `${model ? 'Model' : 'Reasoning'}: ${value}`, choices: [] };
                }
            }
            throw failure('delivery_uncertain', 'Antigravity has not confirmed the selected setting');
        } finally {
            if (owned) await this.delivery.verify(terminal);
            if (owned && parse(await terminal.capture())) {
                await this.delivery.verify(terminal);
                await terminal.sendKey('Escape');
            }
        }
    }
}
