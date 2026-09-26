import { realpath } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { classifyLifecycle, jevConfig } from "./jev.mjs";

export function lifecycleNotice() {
    return `Before Task recovery or outcome transitions, use the read-only Jev helper: node ${JSON.stringify(fileURLToPath(import.meta.url))} SESSION_ID with JSON {"kind":"recovery|outcome|review_policy","job_id":"JOB_ID","task_id":"TASK_ID","prompt":"current verbatim user request","history":[{"role":"assistant","text":"visible execution evidence"}]} on stdin. Pi/OMP may use taskix args ["lifecycle","classify",JSON.stringify(input)]. Reuse routed review_policy; reassess if scope changes. On status=agent use the existing main-Agent workflow. ready still requires actual acceptance verification; it is never Job approval.`;
}

// Read-only: the caller retains lease, Plan, dependency and authorization duties.
export async function assessLifecycle(input, options, runner, settings = {}) {
    const fallback = reason => ({ status: "agent", reason });
    if (!jevConfig(settings.env)) return fallback("disabled");
    if (!options.session || !input?.prompt?.trim() || !input.job_id ||
        !["outcome", "recovery", "review_policy"].includes(input.kind)) return fallback("invalid_input");
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 8000);
    const scoped = { ...options, signal: options.signal ? AbortSignal.any([options.signal, controller.signal]) : controller.signal };
    try {
        const context = (await runner(["routing", "snapshot"], scoped)).result;
        const target = context?.routing?.candidates?.find(c => c.job.id === input.job_id);
        if (!target || context.routing.complete !== true) return fallback("incomplete_context");
        if (input.kind === "outcome" && !input.task_id) return fallback("missing_target");
        if (input.kind !== "review_policy" && input.task_id) {
            // DONE/CANCELLED Tasks are normally omitted from prompt candidates.
            // Read only an explicitly named target, retaining the same fact fields
            // and SQL-equivalent text bounds; do not enumerate terminal history.
            if (!target.tasks.some(t => t.id === input.task_id)) {
                const task = (await runner(["task", "show", input.task_id], scoped)).result;
                if (task?.id !== input.task_id || task.job_id !== input.job_id || task.project_id !== context.project_id) return fallback("missing_target");
                if (task.title.length > 300 || (task.reason?.length || 0) > 1000) return fallback("incomplete_context");
                target.tasks.push(task);
            }
        }
        const result = await classifyLifecycle({ prompt: input.prompt, history: input.history || [],
            assessment: { kind: input.kind, job_id: input.job_id, task_id: input.task_id }, context,
            options: scoped, runner, ...settings });
        const decision = result.decision;
        if (decision.action === "agent") return fallback(decision.reason);
        scoped.signal.throwIfAborted();
        let args = [];
        if (input.kind === "review_policy") args = ["job", "update", decision.job_id, "--review-policy", decision.review_policy, "--expect-revision", String(decision.job_revision)];
        else {
            const command = { resume: "claim", retry: "retry", reopen: "reopen", cancel: "cancel", release: "release", wait: "wait", block: "block", fail: "fail" }[decision.action];
            if (command) args = ["task", command, decision.task_id, "--expect-revision", String(decision.task_revision)];
        }
        return { status: "selected", decision, args,
            required_arguments: ["wait", "block", "fail", "release"].includes(decision.action) ? ["--reason"] : [],
            ...(decision.requires_verification ? { instruction: "Verify all Task acceptance criteria against actual outputs before task done; retain the current lease and expected revision. Do not approve the Job." } : {}) };
    } catch { return fallback("lifecycle_unavailable"); }
    finally { clearTimeout(timer); }
}

const main = import.meta.main ?? (process.argv[1] && import.meta.url === pathToFileURL(await realpath(process.argv[1]).catch(() => "")).href);
if (main) {
    try {
        const session = process.argv[2];
        if (!session || process.argv.length !== 3) throw new Error("Session required");
        let input = "";
        for await (const chunk of process.stdin) {
            input += chunk;
            if (Buffer.byteLength(input) > 1024 * 1024) throw new Error("Input too large");
        }
        const { runTaskix } = await import("./taskix-cli.mjs");
        process.stdout.write(JSON.stringify(await assessLifecycle(JSON.parse(input), { session, cwd: process.cwd() }, runTaskix)) + "\n");
    } catch {
        process.stdout.write(JSON.stringify({ status: "agent", reason: "invalid_input" }) + "\n");
        process.exitCode = 1;
    }
}
