// Native agy fixture for the Rust Telegram/Engine/Bridge workspace launch path.
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
const [root, endpoint] = process.argv.slice(2);
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const bin = join(root, 'bin'), project = join(root, 'project');
mkdirSync(bin, { recursive: true }); mkdirSync(project, { recursive: true });
writeFileSync(join(bin, 'taskix'), readFileSync(new URL('../../taskix-manager/tests/fixtures/antigravity-taskix-cli.mjs', import.meta.url)), { mode: 0o700 });
const env = { PATH: `${bin}:${process.env.PATH}`, AGENTIX_CONTROL_ENDPOINT: endpoint,
    AGENTIX_ANTIGRAVITY_DATA_DIR: join(root, 'bridge'), TASKIX_ANTIGRAVITY_DATA_DIR: join(root, 'taskix'),
    TASKIX_TEST_LOG: join(root, 'taskix.jsonl') };
const agy = process.env.AGENTIX_TEST_AGY ?? execFileSync('which', ['agy'], { encoding: 'utf8' }).trim();
writeFileSync(join(root, 'launch'), `#!/bin/sh\nexec env ${Object.entries(env).map(([key, value]) => `${key}=${quote(value)}`).join(' ')} ${quote(agy)}\n`, { mode: 0o700 });
const tmux = process.env.AGENTIX_TEST_TMUX ?? 'tmux';
const mux = args => execFileSync(tmux, ['-S', join(root, 'tmux.sock'), ...args], { encoding: 'utf8', timeout: 2000, stdio: ['ignore', 'pipe', 'pipe'] });
mux(['-f', '/dev/null', 'new-session', '-d', '-s', 'fixture', '-x', '120', '-y', '50', '/bin/sh']);
const timer = setInterval(() => {
    try {
        for (const pane of mux(['list-panes', '-a', '-F', '#{pane_id}']).trim().split('\n')) {
            const screen = mux(['capture-pane', '-p', '-t', pane]);
            if (screen.includes('Do you trust the contents of this project?')) mux(['send-keys', '-t', pane, 'Enter']);
        }
    } catch { /* The host cleanup can close the owned server at any time. */ }
}, 200);
process.stdin.resume(); process.stdin.once('end', () => { clearInterval(timer); });
console.log('ready');
