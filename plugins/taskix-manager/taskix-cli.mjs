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
        const status = error.signal ? `signal ${error.signal}`
            : typeof error.code === "number" ? `exit code ${error.code}` : error.code;
        let message = error.stderr?.trim() || (status ? `Taskix failed (${status})` : error.message);
        if (error.stdout) {
            try {
                message = JSON.parse(error.stdout).error?.message || message;
            } catch {
                // Keep the subprocess error for non-JSON output.
            }
        }
        // Keep only command words, never user-supplied arguments or lease tokens.
        const command = `taskix ${args.slice(0, 2).filter(arg => /^[a-z][a-z-]*$/.test(arg)).join(" ")}`.trim();
        const failure = new Error(`${command}: ${message}`, { cause: error });
        failure.command = command;
        failure.exitCode = typeof error.code === "number" ? error.code : undefined;
        failure.code = error.code;
        failure.signal = error.signal;
        throw failure;
    }
}
