import { execFile } from "node:child_process";
import { promisify } from "node:util";

const executeFile = promisify(execFile);
const reserved = new Set([
    "--session",
    "--executor",
    "--lease-token",
    "--actor",
    "--json",
]);

export function buildArgs(args, options = {}) {
    if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) {
        throw new Error("taskix args must be an array of strings");
    }
    for (const arg of args) {
        if (reserved.has(arg.split("=")[0]))
            throw new Error(`Identity option is managed by the host: ${arg}`);
    }
    const result = ["--json", ...args];
    for (const [flag, value] of [
        ["--session", options.session],
        ["--executor", options.executor],
        ["--lease-token", options.token],
        ["--idempotency-key", options.idempotencyKey],
    ]) {
        if (value) result.push(flag, value);
    }
    return result;
}

export async function runTaskix(args, options = {}) {
    try {
        const { stdout } = await executeFile(
            "taskix",
            buildArgs(args, options),
            {
                cwd: options.cwd,
                signal: options.signal,
                timeout: 30000,
                maxBuffer: 4 * 1024 * 1024,
                windowsHide: true,
            },
        );
        const response = JSON.parse(stdout);
        if (response.schema_version !== 1 || !response.ok)
            throw new Error(
                response.error?.message || "Unsupported taskix response",
            );
        return response;
    } catch (error) {
        if (error.stdout) {
            try {
                throw new Error(
                    JSON.parse(error.stdout).error?.message || error.message,
                );
            } catch (parsed) {
                if (!(parsed instanceof SyntaxError)) throw parsed;
            }
        }
        throw error;
    }
}
