// A deterministic terminal host for exercising real rmux input without a model.
import { writeFileSync } from 'node:fs';
const codex = process.argv[4] === 'codex';
process.title = codex ? 'codex' : 'claude';
process.stdin.setRawMode(true);
process.stdin.setEncoding('utf8');
let input = process.argv[3] ?? '', pasting = false, pending = '', burstUntil = 0, drawTimer;
function typed() {
    if (!codex) return;
    burstUntil = Date.now() + 180;
    clearTimeout(drawTimer);
    drawTimer = setTimeout(render, 180);
}
function render() {
    const rows = input.split('\n');
    if (codex) {
        process.stdout.write('\x1b[2J\x1b[H' + (process.argv[5] === 'plain' ? '' : '\x1b[48;5;235m') + ' '.repeat(80) + '\n› ' + (input || '\x1b[2mAsk Codex to do anything\x1b[22m').split('\n').join('\n  ') + '\n' + ' '.repeat(80) + '\x1b[0m\n  gpt-6-astra · Context 0% used    Vim: Insert' + `\x1b[${rows.length + 1};${rows.at(-1).length + 3}H\x1b[?2004h`);
        return;
    }
    process.stdout.write('\x1b[2J\x1b[H──────────\n❯ ' + rows.join('\n  ') + '\n──────────\n' +
        `\x1b[${rows.length + 1};${rows.at(-1).length + 3}H\x1b[?2004h`);
}
render();
process.stdin.on('data', chunk => {
    pending += chunk;
    while (pending.length) {
        if (pending.startsWith('\x1b[200~')) { pasting = true; pending = pending.slice(6); }
        else if (pending.startsWith('\x1b[201~')) { pasting = false; pending = pending.slice(6); }
        else if (pending.startsWith('\x1b') && pending.length < 6) return;
        else {
            const char = pending[0]; pending = pending.slice(1);
            if (!pasting && char === '\x03') { input = ''; render(); }
            else if (!pasting && (char === '\r' || char === '\n')) {
                if (codex && Date.now() < burstUntil) { input += '\n'; typed(); }
                else writeFileSync(process.argv[2], input);
            }
            else { input += char; typed(); }
        }
    }
});
