import { withDeadline } from "./jev-io.mjs";
import { realpath } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { classifyLifecycle, jevConfig } from "./jev.mjs";

export function lifecycleNotice() {
    return `Before the final Task completes an ACTIVE Job, classify kind=completion to decide pending_review versus completed from the whole Job. Execute its guarded atomic completion command after acceptance verification. Before Task recovery or outcome transitions, use the read-only Jev helper: node ${JSON.stringify(fileURLToPath(import.meta.url))} SESSION_ID with JSON {"kind":"recovery|outcome|review_policy|completion","job_id":"JOB_ID","task_id":"TASK_ID","prompt":"current verbatim user request","history":[{"role":"assistant","text":"visible execution evidence"}]} on stdin. Pi/OMP may use taskix args ["lifecycle","classify",JSON.stringify(input)]. Reuse routed review_policy; reassess if scope changes. On status=agent use the existing main-Agent workflow. ready still requires actual acceptance verification; it is never Job approval.`;
}

// Read-only: the caller retains lease, Plan, dependency and authorization duties.
export async function assessLifecycle(input, options, runner, settings = {}) {
    const fallback = reason => ({ status: "agent", reason });
    if (!jevConfig(settings.env)) return fallback("disabled");
    if (!options.session || (typeof input?.prompt !== "string" || !input.prompt.trim()) || !input.job_id ||
        !["outcome", "recovery", "review_policy", "completion"].includes(input.kind)) return fallback("invalid_input");
    try {
        return await withDeadline(options.signal, async signal => {
            const scoped = { ...options, signal };
            const rawRunner = runner;
            const checkedRunner = async (args, opts) => {
                signal.throwIfAborted();
                const result = await rawRunner(args, opts);
                signal.throwIfAborted();
                return result;
            };
            const context = (await checkedRunner(["routing", "snapshot"], scoped)).result;
            const target = context?.routing?.candidates?.find(c => c.job.id === input.job_id);
            if (!target || context.routing.complete !== true) return fallback("incomplete_context");
            const transition = input.transition || "done";
            const completionTask = target.tasks.find(t => t.id === input.task_id);
            if (input.kind === "completion") {
                if (!["done", "cancel", "submit"].includes(transition)) return fallback("invalid_input");
                const remaining = target.tasks.filter(t => !["DONE", "CANCELLED"].includes(t.status));
                if (transition === "submit" ? input.task_id || remaining.length :
                    !completionTask || remaining.length !== 1 || remaining[0].id !== input.task_id) return fallback("not_ready");
            }
            if (input.kind === "outcome" && !input.task_id) return fallback("missing_target");
            if (["outcome", "recovery"].includes(input.kind) && input.task_id) {
                // DONE/CANCELLED Tasks are normally omitted from prompt candidates.
                // Read only an explicitly named target, retaining the same fact fields
                // and SQL-equivalent text bounds; do not enumerate terminal history.
                if (!target.tasks.some(t => t.id === input.task_id)) {
                    const task = (await checkedRunner(["task", "show", input.task_id], scoped)).result;
                    if (task?.id !== input.task_id || task.job_id !== input.job_id || task.project_id !== context.project_id) return fallback("missing_target");
                    if (task.title.length > 300 || (task.reason?.length || 0) > 1000) return fallback("incomplete_context");
                    target.tasks.push(task);
                }
            }
            const result = await classifyLifecycle({ prompt: input.prompt, history: input.history || [],
                assessment: { kind: input.kind, job_id: input.job_id, task_id: input.task_id }, context,
                options: scoped, runner: checkedRunner, ...settings });
            const decision = result.decision;
            if (decision.action === "agent") return fallback(decision.reason);
            scoped.signal.throwIfAborted();
            let args = [];
            if (input.kind === "review_policy") args = ["job", "update", decision.job_id, "--review-policy", decision.review_policy, "--expect-revision", String(decision.job_revision)];
            else if (input.kind === "completion") {
                args = transition === "submit" ? ["job", "submit", decision.job_id] : ["task", transition, input.task_id];
                args.push("--review-policy", decision.review_policy, "--expect-job-revision", String(decision.job_revision),
                    "--expect-revision", String(transition === "submit" ? decision.job_revision : completionTask.revision));
            } else {
                const command = { resume: "claim", retry: "retry", reopen: "reopen", cancel: "cancel", release: "release", wait: "wait", block: "block", fail: "fail" }[decision.action];
                if (command) args = ["task", command, decision.task_id, "--expect-revision", String(decision.task_revision)];
            }
            return { status: "selected", decision, args,
                ...(input.kind === "completion" ? { instruction: "Verify actual Task acceptance, then execute this atomic completion command with the current lease. On conflict refresh and reassess. This does not approve an already pending Job." } : {}),
                required_arguments: ["wait", "block", "fail", "release"].includes(decision.action) ? ["--reason"] : [],
                ...(decision.requires_verification ? { instruction: "Verify all Task acceptance criteria against actual outputs before task done; retain the current lease and expected revision. Do not approve the Job." } : {}) };
        });
    } catch { return fallback("lifecycle_unavailable"); }
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
