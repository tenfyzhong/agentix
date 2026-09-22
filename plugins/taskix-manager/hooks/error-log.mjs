import { appendFile, mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export async function reportHookError(error, event) {
    const message = error?.message || String(error);
    process.stderr.write(`Task board hook: ${message}\n`);
    try {
        const path = join(process.env.XDG_STATE_HOME || join(homedir(), ".local", "state"), "taskix", "hooks.jsonl");
        // Do not persist the raw event, transcript, environment or CLI arguments.
        const entry = {
            timestamp: new Date().toISOString(),
            event: typeof event?.hook_event_name === "string" ? event.hook_event_name : "unknown",
            session_id: typeof event?.session_id === "string" ? event.session_id : undefined,
            cwd: typeof event?.cwd === "string" ? event.cwd : undefined,
            command: error?.command,
            exit_code: error?.exitCode,
            code: error?.code,
            signal: error?.signal,
            message,
        };
        await mkdir(dirname(path), { recursive: true, mode: 0o700 });
        await appendFile(path, `${JSON.stringify(entry)}\n`, { mode: 0o600 });
        process.stderr.write(`Hook error log: ${path}\n`);
    } catch (loggingError) {
        process.stderr.write(`Could not write hook error log: ${loggingError.message}\n`);
    }
}
