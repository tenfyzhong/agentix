import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

function readJson(path) {
    try {
        return JSON.parse(readFileSync(path, "utf8"));
    } catch (error) {
        if (error.code === "ENOENT" || error.code === "ENOTDIR") return {};
        throw error;
    }
}

function expandHome(path) {
    return path.startsWith("~/") ? join(homedir(), path.slice(2)) : path;
}

// Pi stores global local-package paths relative to its agent directory, not cwd.
const agentDir = resolve(expandHome(process.env.PI_CODING_AGENT_DIR || join(homedir(), ".pi", "agent")));
const settings = readJson(join(agentDir, "settings.json"));
const removed = new Set();
for (const entry of settings.packages ?? []) {
    const source = typeof entry === "string" ? entry : entry.source;
    if (typeof source !== "string") continue;
    const path = expandHome(source);
    if (!isAbsolute(path) && (/^[a-z][a-z0-9+.-]*:/i.test(path) || path.startsWith("git@"))) continue;
    const packagePath = resolve(agentDir, path);
    if (removed.has(packagePath)) continue;
    if (readJson(join(packagePath, "package.json")).name !== "agentix-plugins") continue;
    // Let Pi update its settings; removing a local source preserves the checkout.
    const result = spawnSync("pi", ["remove", packagePath], { stdio: "inherit" });
    if (result.error) throw result.error;
    if (result.status !== 0) process.exit(result.status ?? 1);
    removed.add(packagePath);
}
