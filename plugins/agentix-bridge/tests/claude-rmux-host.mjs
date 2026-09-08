// A deterministic terminal host for exercising real rmux input without a model.
import { writeFileSync } from 'node:fs';
process.title = 'claude';
process.stdin.setRawMode(true);
process.stdin.setEncoding('utf8');
let input = process.argv[3] ?? '', pasting = false, pending = '';
function render() {
    const rows = input.split('\n');
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
            else if (!pasting && (char === '\r' || char === '\n')) writeFileSync(process.argv[2], input);
            else input += char;
        }
    }
});
