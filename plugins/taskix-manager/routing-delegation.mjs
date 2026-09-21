import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, writeFile, rename, rm, readdir, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { sameDirectory } from "./routing-decision.mjs";
import { routingReceipt } from "./routing-state.mjs";

const defaultDirectory = join(tmpdir(), `taskix-routing-${process.getuid?.() ?? "user"}`);
const lifetime = 3600000;
const pick = (value, keys) => Object.fromEntries(keys.filter(key => value?.[key] !== undefined).map(key => [key, value[key]]));

export async function delegationContext(prompt, context, routed, options, routing, history) {
    const directory = routing.cacheDir ?? defaultDirectory;
    const key = createHash("sha256").update(JSON.stringify([options.session, options.cwd, randomUUID(), process.env.TASKIX_CONFIG || ""])).digest("hex");
    const path = join(directory, `${key}.snapshot.json`);
    const packet = {
        version: 1, parent_session: options.session, cwd: options.cwd,
        expires: Date.now() + lifetime, reason: routed.decision.reason, prompt,
        assignment: { ...pick(context, ["project_id", "job_id", "task_id"]), previous_job_id: context.previous_job?.id },
        complete: context.routing?.complete === true,
        candidates: (routed.candidates ?? context.routing?.candidates ?? []).map(candidate => ({
            job: pick(candidate.job, ["id", "project_id", "revision", "status", "archived_at", "title", "prompt", "goal", "review_policy", "conversation"]),
            tasks: (candidate.tasks ?? []).map(task => pick(task, ["id", "title", "status", "reason", "revision"])),
        })),
        inbox: (context.inbox_todos ?? []).map(entry => pick(entry, ["id", "content", "revision", "status", "content_pending"])),
        history: history.slice(-4).map(message => pick(message, ["role", "text"])),
    };
    // Keep facts off the main thread. Oversized packets fall back to discovery;
    // never silently present truncated evidence as complete.
    if (Buffer.byteLength(JSON.stringify(packet)) > 1024 * 1024) {
        packet.candidates = []; packet.inbox = []; packet.history = []; packet.complete = false;
    }
    if (Buffer.byteLength(JSON.stringify(packet)) > 1024 * 1024) throw new Error("Routing snapshot exceeds budget");
    await mkdir(directory, { recursive: true, mode: 0o700 });
    for (const name of (await readdir(directory)).filter(name => /^[a-f0-9]{64}\.snapshot\.json$/.test(name))) {
        const old = join(directory, name);
        try { if ((await stat(old)).mtimeMs < Date.now() - lifetime) await rm(old, { force: true }); } catch { /* Concurrent cleanup is harmless. */ }
    }
    const temporary = `${path}.${randomUUID()}`;
    try {
        await writeFile(temporary, JSON.stringify(packet), { flag: "wx", mode: 0o600 });
        await rename(temporary, path);
    } finally { await rm(temporary, { force: true }); }
    return `Jev deferred (${routed.decision.reason}). Main Agent: use taskix-manager references/routing-classifier.md. Delegate once to a fresh-context subagent (reasoning_effort=low; do not inherit this conversation or downgrade the current model). Pass only that reference path and this snapshot path. Start its prompt with TASKIX_ROUTING_CLASSIFIER followed by the JSON-quoted snapshot path on its own line. Do not read candidates into the main context first. The classifier must not delegate or modify task state. Return compact JSON (action, job_id, inbox_ids, revision, reason, questions); uncertain is valid. Main Agent must run the packaged routing-decision.mjs validator as described in the reference before writes; use its revision-guarded followup_args. Use taskix context if facts are missing. If subagents are unavailable, read the snapshot and judge locally using the same rules.\nSnapshot: ${JSON.stringify(path)}`;
}

// Only a child holding a live snapshot can enter classifier mode. This is a
// workflow recursion guard, not an authorization or sandbox boundary.
export async function classifierHook(event, directory = defaultDirectory) {
    const roleEvent = { ...event, session_id: `classifier:${event.session_id}`, turn_id: null };
    if (event.hook_event_name === "UserPromptSubmit") {
        const firstLine = typeof event.prompt === "string" ? event.prompt.split("\n", 1)[0] : "";
        if (firstLine.startsWith("TASKIX_ROUTING_CLASSIFIER ")) {
            try {
                const path = JSON.parse(firstLine.slice("TASKIX_ROUTING_CLASSIFIER ".length));
                if (typeof path !== "string" || dirname(resolve(path)) !== resolve(directory) || !/[a-f0-9]{64}\.snapshot\.json$/.test(path)) return false;
                const packet = JSON.parse(await readFile(path, "utf8"));
                if (packet.version !== 1 || !Number.isFinite(packet.expires) || packet.expires <= Date.now() || packet.parent_session === event.session_id || !(await sameDirectory(packet.cwd, event.cwd))) return false;
                return await routingReceipt(roleEvent, "write", directory);
            } catch { return false; }
        }
        await routingReceipt(roleEvent, "clear", directory);
        return false;
    }
    const active = await routingReceipt(roleEvent, "read", directory);
    if (event.hook_event_name === "SessionEnd") await routingReceipt(roleEvent, "clear", directory);
    return active;
}
