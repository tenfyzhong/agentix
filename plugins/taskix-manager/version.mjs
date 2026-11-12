import { execFileSync } from "node:child_process";
import { readFile, realpath } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repository = "tenfyzhong/agentix";
const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
function git(path, ...args) {
    try { return execFileSync("git", ["-C", path, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 10000 }).trim(); }
    catch { return null; }
}

export async function inspectPlugin(path) {
    path = await realpath(path);
    let manifest;
    for (const file of ["package.json", "manifest.json"]) {
        try { manifest = JSON.parse(await readFile(resolve(path, file), "utf8")); break; }
        catch (error) { if (error.code !== "ENOENT") throw error; }
    }
    if (!["agentix-plugins", "taskix-manager", "agentix-bridge", "taskix-sync"].includes(manifest?.name) && manifest?.id !== "taskix-sync") {
        throw new Error("Path is not an Agentix plugin directory");
    }
    // Do not inherit an unrelated parent repository around a copied cache.
    const tracked = git(path, "ls-files", "--", "package.json", "manifest.json");
    const commit = tracked ? git(path, "rev-parse", "HEAD") : null;
    const branch = commit ? git(path, "symbolic-ref", "--quiet", "--short", "HEAD") : null;
    const tags = commit ? git(path, "tag", "--points-at", "HEAD") : null;
    const tag = tags?.split("\n").find(value => value === manifest.version) ?? null;
    const worktreeStatus = commit ? git(path, "status", "--porcelain") : null;
    return { name: manifest.name ?? manifest.id, version: manifest.version, path,
        source: commit ? git(path, "remote", "get-url", "origin") : null,
        branch, tag, commit, dirty: worktreeStatus === null ? null : Boolean(worktreeStatus),
        latest: null, update: "unchecked" };
}

export function compareUpdate(installed, latest, remoteCommit = null) {
    if (installed.dirty) return "modified";
    if (installed.dirty !== false) return "unknown";
    if (installed.branch && installed.commit && remoteCommit) return installed.commit === remoteCommit ? "current" : "available";
    if (installed.tag !== installed.version || !stable.test(installed.version) || !stable.test(latest)) return "unknown";
    const a = installed.version.split(".").map(BigInt), b = latest.split(".").map(BigInt);
    for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i] ? "available" : "ahead";
    return "current";
}

async function github(path, fetcher) {
    const response = await fetcher(`https://api.github.com/repos/${repository}/${path}`, {
        headers: { Accept: "application/vnd.github+json" }, signal: AbortSignal.timeout(10000),
    });
    if (!response.ok) throw new Error(`GitHub HTTP ${response.status}`);
    return response.json();
}

export async function latestStable(fetcher = fetch) {
    const release = await github("releases/latest", fetcher);
    if (!stable.test(release.tag_name)) throw new Error("Latest release has no numeric stable tag");
    return release.tag_name;
}

export async function checkUpdates(installed, fetcher = fetch) {
    const result = { ...installed, update: "unknown" };
    try {
        result.latest = await latestStable(fetcher);
        let remoteCommit = null;
        if (installed.branch && installed.commit) {
            // Never compare another repository's branch against Agentix.
            if (!/^(?:https:\/\/github\.com\/|git@github\.com:)tenfyzhong\/agentix(?:\.git)?$/.test(installed.source ?? "")) return result;
            remoteCommit = (await github(`commits/${encodeURIComponent(installed.branch)}`, fetcher)).sha;
            if (!/^[a-f0-9]{40}$/.test(remoteCommit ?? "")) throw new Error("Missing remote commit");
        }
        result.update = compareUpdate(installed, result.latest, remoteCommit);
        result.remoteCommit = remoteCommit;
    } catch (error) { result.error = error.message; }
    return result;
}

async function main() {
    let path = dirname(fileURLToPath(import.meta.url)), check = false, json = false;
    const args = process.argv.slice(2);
    for (let i = 0; i < args.length; i++) {
        if (args[i] === "--path" && args[i + 1]) path = resolve(args[++i]);
        else if (args[i] === "--check") check = true;
        else if (args[i] === "--json") json = true;
        else throw new Error("Usage: version.mjs [--path PLUGIN_DIRECTORY] [--check] [--json]");
    }
    let status = await inspectPlugin(path);
    if (check) status = await checkUpdates(status);
    if (json) console.log(JSON.stringify(status, null, 2));
    else for (const [key, value] of Object.entries(status)) console.log(`${key}: ${value ?? "unknown"}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
    main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
