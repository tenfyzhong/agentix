import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { validate } from '../protocol/validate.mjs';
const sample = () => JSON.parse(readFileSync(new URL('../protocol/snapshot.json', import.meta.url)));
test('shared snapshot contract preserves time, history and queue', () => {
    assert.equal(validate('Snapshot', sample()), true);
    const missing = sample(); delete missing.session.updatedAt;
    assert.equal(validate('Snapshot', missing), false);
    const invalid = sample(); invalid.session.status = 'invented';
    assert.equal(validate('Snapshot', invalid), false);
    invalid.session.status = 'idle'; invalid.seq = -1;
    assert.equal(validate('Snapshot', invalid), false);
});
test('generated protocol bindings are current', () => {
    execFileSync(process.execPath, [new URL('../protocol/generate.mjs', import.meta.url).pathname, '--check']);
});
