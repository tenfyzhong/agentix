import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';

// Verify the published artifact, rather than only the source checkout.
test('packed bridge includes native session control runtime', () => {
    const [archive] = JSON.parse(execFileSync('npm', ['pack', '--dry-run', '--json', '--ignore-scripts'], {
        cwd: new URL('..', import.meta.url), encoding: 'utf8', shell: process.platform === 'win32',
    }));
    assert.ok(archive.files.some(file => file.path === 'new-session.mjs'));
});
