import { realpath } from "node:fs/promises";
import { pathToFileURL } from "node:url";

export async function sameDirectory(left, right) {
    if (typeof left !== "string" || typeof right !== "string") return false;
    if (left === right) return true;
    try { return await realpath(left) === await realpath(right); } catch { return false; }
}

// Deterministic validation of classifier advice. Never changes task lifecycle.
const id = (value, prefix) => typeof value === "string" && new RegExp(`^${prefix}_[a-zA-Z0-9_-]+$`).test(value) && value.length <= 128;

export async function validateDecision(decision, assignment, options, runner) {
    const selected = ["followup", "resume"].includes(decision?.action);
    if (!decision || JSON.stringify(decision).length > 1000 ||
        !["followup", "resume", "new_job", "discussion", "uncertain"].includes(decision.action) ||
        (selected ? !id(decision.job_id, "job") || !Number.isSafeInteger(decision.revision) || decision.revision < 0 : decision.job_id !== null || decision.revision !== null) ||
        !Array.isArray(decision.inbox_ids) || decision.inbox_ids.some(value => !id(value, "inbox")) || new Set(decision.inbox_ids).size !== decision.inbox_ids.length ||
        (["discussion", "uncertain"].includes(decision.action) && decision.inbox_ids.length) ||
        typeof decision.reason !== "string" || decision.reason.length > 240 ||
        !Array.isArray(decision.questions) || decision.questions.length > 2 || decision.questions.some(value => typeof value !== "string")) {
        throw new Error("Invalid classifier result");
    }
    if (assignment.job_id && !["discussion", "uncertain"].includes(decision.action) && decision.job_id !== assignment.job_id)
        throw new Error("Routing conflicts with parent assignment");
    if (selected) {
        const fresh = (await runner(["routing", "revision", decision.job_id], options)).result;
        if (!assignment.project_id || !fresh || fresh.id !== decision.job_id || fresh.project_id !== assignment.project_id ||
            fresh.revision !== decision.revision || fresh.archived_at || fresh.status !== (decision.action === "followup" ? "PENDING_REVIEW" : "ACTIVE"))
            throw new Error("Routing evidence changed; reassess current facts");
    }
    if (decision.inbox_ids.length) {
        if (!assignment.project_id) throw new Error("Missing routing Project");
        const entries = (await runner(["inbox", "list", "--project", assignment.project_id], options)).result;
        if (!Array.isArray(entries) || decision.inbox_ids.some(value => !entries.some(entry => entry.id === value && entry.status === "TODO" && entry.published && !entry.deleted && !entry.content_pending && !entry.job_id && !entry.lease)))
            throw new Error("Routing Inbox evidence changed; reassess current facts");
    }
    return { decision, followup_args: decision.action === "followup"
        ? ["job", "followup", decision.job_id, "--expect-revision", String(decision.revision), ...decision.inbox_ids.flatMap(value => ["--inbox", value])]
        : null };
}

export async function validateSnapshotDecision(packet, decision, options, runner) {
    if (packet?.version !== 1 || !Number.isFinite(packet.expires) || packet.expires <= Date.now() ||
        packet.parent_session !== options.session || !(await sameDirectory(packet.cwd, options.cwd)) || !packet.assignment)
        throw new Error("Invalid routing snapshot");
    const live = (await runner(["routing", "snapshot"], options)).result;
    if (packet.assignment.project_id && live.project_id !== packet.assignment.project_id)
        throw new Error("Routing Project changed");
    if (packet.assignment.job_id && live.job_id !== packet.assignment.job_id)
        throw new Error("Routing parent assignment changed");
    return validateDecision(decision, live, options, runner);
}

// The parent passes only the compact child JSON on stdin. Never evaluate model text.
async function main() {
    try {
        const { readFile, stat } = await import("node:fs/promises");
        const [snapshot, session] = process.argv.slice(2);
        if (!snapshot || !session || process.argv.length !== 4 || (await stat(snapshot)).size > 1024 * 1024)
            throw new Error("Invalid routing snapshot");
        let input = "";
        process.stdin.setEncoding("utf8");
        for await (const chunk of process.stdin) {
            input += chunk;
            if (input.length > 1000) throw new Error("Invalid classifier result");
        }
        const packet = JSON.parse(await readFile(snapshot, "utf8"));
        const { runTaskix } = await import("./runtime.mjs");
        const result = await validateSnapshotDecision(packet, JSON.parse(input), { session, cwd: process.cwd(), signal: AbortSignal.timeout(8000) }, runTaskix);
        process.stdout.write(`${JSON.stringify(result)}\n`);
    } catch {
        process.stderr.write("Taskix routing result rejected; inspect current facts before any ownership write.\n");
        process.exitCode = 1;
    }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) void main();
