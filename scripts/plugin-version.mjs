import { latestStable } from "../plugins/taskix-manager/version.mjs";
import { readFile, writeFile } from "node:fs/promises";

// VERSION is committed before tagging so native Git installs see release metadata.
const manifests = ["package.json", "plugins/taskix-manager/package.json", "plugins/agentix-bridge/package.json",
    "plugins/taskix-manager/.codex-plugin/plugin.json", "plugins/taskix-manager/.claude-plugin/plugin.json",
    "plugins/taskix-manager/.antigravity-plugin/plugin.json", "plugins/agentix-bridge/.claude-plugin/plugin.json",
    "plugins/agentix-bridge/.antigravity-plugin/plugin.json", "plugins/taskix-manager/obsidian/taskix-sync/manifest.json"];
const locks = ["package-lock.json", "plugins/taskix-manager/package-lock.json"];
const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

async function main() {
    const [action, requested, ...extra] = process.argv.slice(2);
    if (action === "latest" && !requested) { console.log(await latestStable()); return; }
    if (!["prepare", "check"].includes(action) || extra.length) throw new Error("Usage: plugin-version.mjs prepare VERSION | check [TAG]");
    const current = (await readFile("VERSION", "utf8")).trim();
    const version = requested ?? current;
    if (!(stable.test(version) || version === "0.0.0-dev")) throw new Error("Version must be X.Y.Z or 0.0.0-dev (no v prefix)");
    if (action === "prepare" && !requested) throw new Error("prepare requires an explicit version");
    if (action === "check" && current !== version) throw new Error(`VERSION ${current} does not match ${version}`);
    const updates = new Map();
    const setVersion = (value, label) => {
        if (typeof value.version !== "string") throw new Error(`Missing version: ${label}`);
        if (action === "check" && value.version !== version) throw new Error(`${label}: ${value.version} != ${version}`);
        value.version = version;
    };
    for (const path of [...manifests, ...locks]) {
        const value = JSON.parse(await readFile(path, "utf8"));
        setVersion(value, path);
        if (locks.includes(path)) {
            const keys = path === "package-lock.json" ? ["", "plugins/taskix-manager", "plugins/agentix-bridge"] : [""];
            for (const key of keys) setVersion(value.packages[key], `${path}:${key}`);
        }
        updates.set(path, `${JSON.stringify(value, null, 2)}\n`);
    }
    const cargo = await readFile("Cargo.toml", "utf8");
    const match = cargo.match(/(\[workspace\.package\][\s\S]*?\nversion = ")([^"]+)(")/);
    if (!match) throw new Error("Missing workspace package version");
    if (action === "check" && match[2] !== version) throw new Error(`Cargo.toml: ${match[2]} != ${version}`);
    updates.set("Cargo.toml", cargo.replace(match[0], `${match[1]}${version}${match[3]}`));
    const lock = await readFile("Cargo.lock", "utf8");
    let count = 0;
    const updatedLock = lock.split(/(?=\[\[package\]\])/).map(block => {
        // Workspace packages have no registry/git source; leave dependencies alone.
        if (!/^\[\[package\]\]/.test(block) || /^source = /m.test(block)) return block;
        const found = block.match(/^version = "([^"]+)"/m);
        if (!found) throw new Error("Missing Cargo lock package version");
        if (action === "check" && found[1] !== version) throw new Error(`Cargo.lock workspace version ${found[1]} != ${version}`);
        count++;
        return block.replace(/^version = "[^"]+"/m, `version = "${version}"`);
    }).join("");
    if (!count) throw new Error("No workspace packages in Cargo.lock");
    updates.set("Cargo.lock", updatedLock);
    updates.set("VERSION", `${version}\n`);
    // Validate every input before writing any file.
    if (action === "prepare") for (const [path, content] of updates) await writeFile(path, content);
    console.log(`${action === "check" ? "Verified" : "Prepared"} all component versions: ${version}`);
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });
