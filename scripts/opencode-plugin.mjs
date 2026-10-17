import {
    cp,
    copyFile,
    mkdir,
    mkdtemp,
    readFile,
    rename,
    rm,
    writeFile,
} from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const config =
    process.env.OPENCODE_CONFIG_DIR ||
    join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "opencode");
const destination = join(config, ".agentix-plugin");
const [action, source = "main"] = process.argv.slice(2);
if (
    !["install", "remove"].includes(action) ||
    !["local", "main"].includes(source)
)
    throw new Error("Usage: opencode-plugin.mjs install [local|main] | remove");

// Tokenize JSONC so edits retain comments, whitespace and unrelated values exactly.
function parse(text, key) {
    const tokens = [
        ...text.matchAll(
            /"(?:[^"\\]|\\.)*"|\/\/[^\r\n]*|\/\*[\s\S]*?\*\/|[{}\[\],:]|[^\s{}\[\],:]+/g,
        ),
    ].filter((m) => !m[0].startsWith("//") && !m[0].startsWith("/*"));
    const cleaned = tokens
        .filter(
            (m, i) => m[0] !== "," || !["]", "}"].includes(tokens[i + 1]?.[0]),
        )
        .map((m) => m[0])
        .join(" ");
    const value = JSON.parse(cleaned);
    if (!value || Array.isArray(value) || typeof value !== "object")
        throw new Error("OpenCode config must be an object");
    if (value[key] !== undefined && !Array.isArray(value[key]))
        throw new Error("OpenCode plugin setting must be an array");
    let depth = 0,
        range;
    for (let i = 0; i < tokens.length; i++) {
        const t = tokens[i];
        if (
            depth === 1 &&
            t[0] === JSON.stringify(key) &&
            tokens[i + 1]?.[0] === ":"
        ) {
            const first = tokens[i + 2];
            let j = i + 2,
                nested = 0;
            do {
                const s = tokens[j][0];
                if (["[", "{"].includes(s)) nested++;
                if (["]", "}"].includes(s)) nested--;
                j++;
            } while (nested > 0);
            range = [
                first.index,
                tokens[j - 1].index + tokens[j - 1][0].length,
            ];
        }
        if (["{", "["].includes(t[0])) depth++;
        if (["}", "]"].includes(t[0])) depth--;
    }
    return {
        value,
        range,
        end: tokens.at(-1).index,
        trailing: tokens.at(-2)?.[0] === ",",
    };
}
function edit(text, key, entry) {
    const { value, range, end, trailing } = parse(text, key);
    const before = value[key] || [];
    const plugins = before.filter((item) => {
        const ref = Array.isArray(item)
            ? item[0]
            : typeof item === "object" && item
              ? item.package
              : item;
        return (
            typeof ref !== "string" ||
            !ref.startsWith(pathToFileURL(destination).href + "/")
        );
    });
    if (!entry && plugins.length === before.length) return text;
    if (entry) plugins.push(...(Array.isArray(entry) ? entry : [entry]));
    if (range)
        return (
            text.slice(0, range[0]) +
            JSON.stringify(plugins, null, 2) +
            text.slice(range[1])
        );
    if (!entry) return text;
    return (
        text.slice(0, end) +
        (Object.keys(value).length && !trailing ? "," : "") +
        "\n  " +
        JSON.stringify(key) +
        ": " +
        JSON.stringify(plugins, null, 2) +
        "\n" +
        text.slice(end)
    );
}
async function configs(name, key, entry) {
    const files = [];
    for (const extension of ["json", "jsonc"]) {
        const path = join(config, name + "." + extension);
        try {
            files.push({ path, text: await readFile(path, "utf8") });
        } catch (error) {
            if (error.code !== "ENOENT") throw error;
        }
    }
    if (!files.length && entry)
        files.push({ path: join(config, name + ".json"), text: "{}\n" });
    return files.map((file, i) => ({
        path: file.path,
        text: edit(
            key === "plugins" ? edit(file.text, "plugin") : file.text,
            key,
            i === files.length - 1 ? entry : undefined,
        ),
    }));
}
const updates = [
    ...(await configs("tui", "plugin")),
    ...(await configs(
        "cli",
        "plugins",
        action === "install"
            ? pathToFileURL(join(destination, "plugins/agentix-bridge")).href
            : undefined,
    )),
    ...(await configs(
        "opencode",
        "plugins",
        action === "install"
            ? pathToFileURL(join(destination, "plugins/taskix-manager")).href
            : undefined,
    )),
];
await mkdir(config, { recursive: true });
const transaction = await mkdtemp(join(config, ".agentix-install-"));
const previous = join(transaction, "previous-package");
const committed = [];
let movedPackage = false;
let publishedPackage = false;
let keepRecovery = false;
try {
    // Prepare every config and its rollback copy before changing the installation.
    for (const [index, update] of updates.entries()) {
        update.staged = join(transaction, `config-${index}`);
        update.backup = join(transaction, `backup-${index}`);
        try {
            await cp(update.path, update.backup, { verbatimSymlinks: true });
            // Staging must never write through a symlink into the live config.
            await copyFile(update.path, update.staged);
        } catch (error) {
            if (error.code !== "ENOENT") throw error;
            update.backup = undefined;
        }
        await writeFile(update.staged, update.text);
    }
    if (action === "install") {
        const temporary = await mkdtemp(join(tmpdir(), "agentix-opencode-"));
        try {
            let repository = fileURLToPath(new URL("../", import.meta.url));
            if (source === "main") {
                repository = join(temporary, "repository");
                execFileSync(
                    "git",
                    [
                        "clone",
                        "--depth",
                        "1",
                        "--branch",
                        "main",
                        "https://github.com/tenfyzhong/agentix.git",
                        repository,
                    ],
                    { stdio: "inherit" },
                );
            }
            const stage = join(transaction, "package");
            await mkdir(stage);
            for (const path of [
                "plugins/taskix-manager",
                "plugins/agentix-bridge",
            ]) {
                await cp(join(repository, path), join(stage, path), {
                    recursive: true,
                    filter: (path) =>
                        !/[\\/](node_modules|tests)([\\/]|$)/.test(path),
                });
            }
            // Native V2 local directories resolve literal index/tui files, while named
            // npm packages resolve exports. Keep both install paths on the same modules.
            await writeFile(
                join(stage, "plugins/taskix-manager/index.js"),
                "export { default } from './opencode/server.mjs';\n",
            );
            await writeFile(
                join(stage, "plugins/agentix-bridge/tui.js"),
                "export { default } from './opencode/tui.mjs';\n",
            );
            const pkg = JSON.parse(
                await readFile(join(repository, "package.json"), "utf8"),
            );
            delete pkg.workspaces;
            delete pkg.devDependencies;
            await writeFile(
                join(stage, "package.json"),
                JSON.stringify(pkg, null, 2) + "\n",
            );
            // Keep the previous package until all config publication succeeds.
            try {
                await rename(destination, previous);
                movedPackage = true;
            } catch (error) {
                if (error.code !== "ENOENT") throw error;
            }
            await rename(stage, destination);
            publishedPackage = true;
        } finally {
            await rm(temporary, { recursive: true, force: true });
        }
    }
    for (const update of updates) {
        await rename(update.staged, update.path);
        committed.push(update);
    }
    if (action === "remove") {
        try {
            await rename(destination, previous);
            movedPackage = true;
        } catch (error) {
            if (error.code !== "ENOENT") throw error;
        }
    }
} catch (error) {
    const failures = [];
    for (const update of committed.reverse()) {
        try {
            if (update.backup) await rename(update.backup, update.path);
            else await rm(update.path, { force: true });
        } catch (rollback) {
            failures.push(rollback);
        }
    }
    try {
        if (publishedPackage)
            await rm(destination, { recursive: true, force: true });
        if (movedPackage) await rename(previous, destination);
    } catch (rollback) {
        failures.push(rollback);
    }
    if (failures.length) {
        keepRecovery = true;
        throw new AggregateError(
            [error, ...failures],
            `OpenCode installation rollback failed; recovery files remain at ${transaction}`,
        );
    }
    throw error;
} finally {
    if (!keepRecovery) await rm(transaction, { recursive: true, force: true });
}
