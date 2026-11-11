import { appendFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const event = JSON.parse(input);
const ancestors = execFileSync('ps', ['-A', '-o', 'pid=,ppid=,comm='], { encoding: 'utf8' });
appendFileSync(process.env.ANTIGRAVITY_TEST_EVENTS, JSON.stringify({ name: process.argv[2], event, pid: process.pid, ppid: process.ppid, ancestors }) + '\n');
process.stdout.write(JSON.stringify(process.argv[2] === 'PreInvocation' ? { terminationBehavior: 'terminate' } : {}));
