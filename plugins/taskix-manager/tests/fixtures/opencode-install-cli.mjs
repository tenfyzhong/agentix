#!/usr/bin/env node
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const args = process.argv.slice(2);
const config = process.env.OPENCODE_CONFIG_DIR;
appendFileSync(process.env.OPENCODE_TEST_CALLS, JSON.stringify({ args, config, cwd: process.cwd(), customConfig: process.env.OPENCODE_CONFIG, configContent: process.env.OPENCODE_CONFIG_CONTENT }) + '\n');
if (args[0] !== 'plugin' || args[1] !== 'add') throw new Error('Unexpected native CLI invocation');
if (args[2] === process.env.OPENCODE_TEST_FAIL_PACKAGE) {
    console.error('Fixture native install failed');
    process.exit(7);
}
const name = args[2].endsWith('taskix-manager') ? 'opencode.json' : 'cli.json';
writeFileSync(join(config, name), JSON.stringify({ plugins: [args[2]] }));
