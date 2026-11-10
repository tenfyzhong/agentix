import { cp, mkdir, mkdtemp, readdir, readFile, writeFile, rm } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const pluginNames = ['taskix-manager', 'agentix-bridge'];
const execute = promisify(execFile);
const runCommand = async (binary, args) => (await execute(binary, args, { timeout: 180000, maxBuffer: 4 * 1024 * 1024 })).stdout;

async function installedPlugins(run, agy) {
    const plugins = JSON.parse(await run(agy, ['plugin', 'list', '--json']));
    if (!Array.isArray(plugins)) throw new Error('Invalid agy plugin list response');
    return plugins.filter(plugin => pluginNames.includes(plugin.name));
}

export async function removeNativePlugins({ run = runCommand, agy = process.env.AGY || 'agy' } = {}) {
    for (const plugin of await installedPlugins(run, agy)) await run(agy, ['plugin', 'uninstall', plugin.name]);
}

export async function installNativePlugins({ repository, branch, run = runCommand, agy = process.env.AGY || 'agy' } = {}) {
    if (!repository && !branch) throw new Error('An explicit repository or remote branch is required');
    const stage = await mkdtemp(join(tmpdir(), 'agentix-agy-install-'));
    let preserve = false;
    try {
        if (!repository) {
            repository = join(stage, 'repository');
            await run('git', ['clone', '--depth', '1', '--single-branch', '--branch', branch, '--',
                'https://github.com/tenfyzhong/agentix.git', repository]);
        }
        const packages = join(stage, 'packages');
        await buildNativePlugins(repository, packages);
        for (const name of pluginNames) await run(agy, ['plugin', 'validate', '--strict', join(packages, name)]);
        const previous = new Map((await installedPlugins(run, agy)).map(plugin => [plugin.name, plugin]));
        for (const plugin of previous.values()) {
            if (typeof plugin.path !== 'string' || !plugin.path) throw new Error(`Missing previous plugin path for ${plugin.name}`);
            await cp(plugin.path, join(stage, 'backup', plugin.name), { recursive: true });
        }
        const attempted = [];
        try {
            for (const name of pluginNames) {
                attempted.push(name);
                await run(agy, ['plugin', 'install', join(packages, name)]);
                if (previous.get(name)?.enabled === false) await run(agy, ['plugin', 'disable', name]);
            }
        } catch (error) {
            const failures = [];
            for (const name of attempted.reverse()) {
                try {
                    if (previous.has(name)) {
                        await run(agy, ['plugin', 'install', join(stage, 'backup', name)]);
                        if (previous.get(name).enabled === false) await run(agy, ['plugin', 'disable', name]);
                    } else {
                        const current = await installedPlugins(run, agy);
                        if (current.some(plugin => plugin.name === name)) await run(agy, ['plugin', 'uninstall', name]);
                    }
                } catch (rollback) { failures.push(rollback); }
            }
            if (failures.length) {
                preserve = true;
                throw new AggregateError([error, ...failures], `Antigravity installation rollback failed; recovery files remain at ${stage}`);
            }
            throw error;
        }
    } finally { if (!preserve) await rm(stage, { recursive: true, force: true }); }
}

/** Build self-contained native packages without another host's configuration. */
export async function buildNativePlugins(repository, destination) {
    for (const name of pluginNames) {
        const source = join(repository, 'plugins', name), target = join(destination, name);
        await mkdir(target, { recursive: true });
        const copy = (from, to = from) => cp(join(source, from), join(target, to), { recursive: true });
        for (const file of ['mcp_config.json', 'hooks.json', 'README.md', 'antigravity']) await copy(file);
        await copy('.antigravity-plugin/plugin.json', 'plugin.json');
        if (name === 'agentix-bridge') {
            for (const file of ['claude', 'protocol', 'new-session.mjs', 'transport.mjs', 'framing.mjs', 'usage.mjs']) await copy(file);
        } else {
            for (const file of await readdir(source)) if (file.endsWith('.mjs') || file.endsWith('.sql')) await copy(file);
            await copy('hooks/error-log.mjs');
            await copy('skills');
            const skill = join(target, 'skills/taskix-manager');
            const original = await readFile(join(skill, 'SKILL.md'), 'utf8');
            const frontmatter = /^---\n[\s\S]*?\n---\n/.exec(original)?.[0];
            if (!frontmatter) throw new Error('Taskix skill frontmatter is missing');
            await writeFile(join(skill, 'references/antigravity-workflow.md'),
                original.slice(frontmatter.length).replaceAll('(references/', '('));
            await writeFile(join(skill, 'SKILL.md'), frontmatter + '\n# Taskix Manager\n\nBefore any tracked work, read [the complete Taskix workflow](references/antigravity-workflow.md).\n\nFollow its routing, ownership, Plan, dependency, review, document, and lifecycle rules. The full workflow is stored as a reference because Antigravity limits skill entrypoints to 24 KB. Do not substitute a shortened workflow.\n\nUse the native `taskix` MCP tool or the `taskix` CLI. The tool supplies the original conversation identity, executor, lease, and idempotency key. Read [the command reference](references/commands.md) when creating work or changing state.\n');
        }
    }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
    const [action, source, destination] = process.argv.slice(2);
    if (action === 'build' && source && destination) await buildNativePlugins(resolve(source), resolve(destination));
    else if (action === 'install' && source === '--local' && destination) await installNativePlugins({ repository: resolve(destination) });
    else if (action === 'install' && source && !destination) await installNativePlugins({ branch: source });
    else if (action === 'remove' && !source) await removeNativePlugins();
    else throw new Error('Usage: antigravity-plugin.mjs build REPOSITORY DESTINATION | install BRANCH | install --local REPOSITORY | remove');
}
