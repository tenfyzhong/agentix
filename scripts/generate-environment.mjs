import { readFile, writeFile } from 'node:fs/promises';
const source = await readFile(
    new URL('../plugins/shared/environment.mjs', import.meta.url),
    'utf8',
);
for (const host of ['agentix-bridge', 'taskix-manager']) {
    const target = new URL(
        `../plugins/${host}/environment.mjs`,
        import.meta.url,
    );
    if (process.argv.includes('--check')) {
        if ((await readFile(target, 'utf8')) !== source)
            throw new Error(`${host} environment parser is stale`);
    } else await writeFile(target, source);
}
