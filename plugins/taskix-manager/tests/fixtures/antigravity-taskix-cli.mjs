#!/usr/bin/env node
import { appendFileSync } from 'node:fs';
const args = process.argv.slice(2);
appendFileSync(process.env.TASKIX_TEST_LOG, JSON.stringify(args) + '\n');
const result = args.includes('context') ? { task_id: 'task_fixture', lease: { token: 'lease_fixture' } } : { args };
process.stdout.write(JSON.stringify({ schema_version: 1, ok: true, result }) + '\n');
