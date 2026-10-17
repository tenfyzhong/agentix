import { readFile, writeFile } from 'node:fs/promises';
const source = await readFile(
    new URL('../plugins/shared/opencode-turns.mjs', import.meta.url),
    'utf8',
);
for (const host of ['agentix-bridge', 'taskix-manager']) {
    const target = new URL(
        `../plugins/${host}/opencode/turns.mjs`,
        import.meta.url,
    );
    if (process.argv.includes('--check')) {
        if ((await readFile(target, 'utf8')) !== source)
            throw new Error(`${host} native turn contract is stale`);
    } else await writeFile(target, source);
}
