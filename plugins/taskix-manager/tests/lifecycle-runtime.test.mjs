import assert from "node:assert/strict";
import { test } from "node:test";
import { runHook, registerExtension } from "../runtime.mjs";
const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://jev.test", TASKIX_JEV_API_KEY: "secret" };
const job = { id: "job_a", project_id: "p", status: "PENDING_REVIEW", revision: 3, title: "Login", prompt: "Fix login", review_policy: "required" };
const task = { id: "task_a", job_id: job.id, project_id: "p", revision: 2, status: "WAITING_USER", title: "Fix login", reason: "Need region" };
function fixture(choice = "resume") {
    const calls = [];
    const runner = async args => {
        calls.push(args);
        if (args[0] === "routing" && args[1] === "snapshot") return { result: { project_id: "p", routing: { complete: true, candidates: [{ job, tasks: [task] }] } } };
        if (args[0] === "routing" && args[1] === "revision") return { result: job };
        if (args[0] === "task" && args[1] === "show") return { result: task };
        assert.fail(`Unexpected command ${args}`);
    };
    const settings = { env, fetch: async (_url, init) => {
        const request = JSON.parse(init.body);
        return { ok: true, json: async () => ({ answers: Object.fromEntries(Object.entries(request.questions).map(([id, q]) => {
            const selected = id === "intent" ? choice : id === "route" ? "followup:job_a" : id === "review_policy" ? "not_applicable" : choice;
            return [id, { type: "choice", choice: selected, confidence: .99,
                probabilities: Object.fromEntries(Object.keys(q.criteria).map(k => [k, k === selected ? 1 : 0])) }];
        })) }) };
    } };
    return { runner, calls, settings };
}
test("lifecycle_helper_returns_guarded_read_only_task_command", async () => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = fixture();
    const result = await assessLifecycle({ kind: "recovery", job_id: job.id, task_id: task.id, prompt: "Use us-east-1 and continue" }, { session: "s" }, f.runner, f.settings);
    assert.equal(result.status, "selected");
    assert.deepEqual(result.args, ["task", "claim", task.id, "--expect-revision", "2"]);
    assert.ok(f.calls.every(args => ["routing", "task"].includes(args[0])));
});
test("disabled_helper_reads_nothing_and_preserves_agent_workflow", async () => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const result = await assessLifecycle({}, {}, () => assert.fail("No CLI"), { env: {}, fetch: () => assert.fail("No HTTP") });
    assert.equal(result.status, "agent");
    assert.equal(result.reason, "disabled");
});
for (const action of ["approve", "reject", "cancel", "task_action"]) test(`prompt_hook_renders_${action}_without_followup`, async () => {
    const f = fixture(action);
    const output = await runHook({ session_id: "s", hook_event_name: "UserPromptSubmit", prompt: "Explicit user request" }, f.runner, f.settings);
    const content = output.hookSpecificOutput.additionalContext;
    assert.match(content, new RegExp(`Taskix route: ${action}`));
    assert.doesNotMatch(content, /undefined|Run job followup/);
    if (action !== "task_action") assert.match(content, new RegExp(`job ${action} job_a --expect-revision 3`));
});
for (const host of ["pi", "omp"]) test(`${host}_structured_lifecycle_classification_uses_shared_helper`, async () => {
    const f = fixture();
    let tool;
    registerExtension({ on() {}, registerTool: t => { tool = t; } }, host, f.runner, globalThis, undefined, f.settings);
    const output = await tool.execute("call", { args: ["lifecycle", "classify", JSON.stringify({ kind: "recovery", job_id: job.id, task_id: task.id, prompt: "Use us-east-1 and continue" })] }, undefined, undefined, { cwd: "/work", sessionManager: { getSessionId: () => "s" } });
    assert.equal(output.details.status, "selected");
});
for (const patch of [
    { kind: "unknown" }, { job_id: "foreign" }, { prompt: "" },
]) test(`helper_rejects_invalid_assessment_${JSON.stringify(patch)}`, async () => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = fixture();
    const result = await assessLifecycle({ kind: "recovery", job_id: job.id, task_id: task.id, prompt: "Continue", ...patch }, { session: "s" }, f.runner, f.settings);
    assert.equal(result.status, "agent");
});
test("helper_checks_named_terminal_task_scope_before_provider_call", async () => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = fixture("reopen");
    const original = f.runner;
    const runner = async args => args[0] === "task" ? { result: { ...task, id: "terminal", job_id: "foreign", status: "CANCELLED" } } : original(args);
    const result = await assessLifecycle({ kind: "recovery", job_id: job.id, task_id: "terminal", prompt: "Restore it" }, { session: "s" }, runner, { ...f.settings, fetch: () => assert.fail("Foreign data must not be sent") });
    assert.equal(result.status, "agent");
    assert.equal(result.reason, "missing_target");
});
test("helper_covers_http_errors_without_exposing_provider_text", async () => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = fixture();
    const result = await assessLifecycle({ kind: "recovery", job_id: job.id, task_id: task.id, prompt: "Continue" }, { session: "s" }, f.runner,
        { ...f.settings, fetch: () => { throw new Error("private provider response secret"); } });
    assert.equal(result.status, "agent");
    assert.doesNotMatch(JSON.stringify(result), /private|secret/);
});
test("completion_helper_returns_policy_update_before_final_task_done", async () => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = fixture("completed");
    const runner = async (...args) => {
        const result = structuredClone(await f.runner(...args));
        if (result.result.routing) result.result.routing.candidates[0].job.status = "ACTIVE";
        else result.result.status = "ACTIVE";
        return result;
    };
    const result = await assessLifecycle({ kind: "completion", job_id: job.id, prompt: "Create the requested release tag" }, { session: "s" }, runner, f.settings);
    assert.equal(result.status, "selected");
    assert.equal(result.decision.action, "completed");
    assert.deepEqual(result.args, ["job", "update", job.id, "--review-policy", "none", "--expect-revision", "3"]);
    assert.match(result.instruction, /before.*task done/);
    assert.ok(f.calls.every(args => args[0] === "routing"));
});
