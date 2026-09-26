import assert from "node:assert/strict";
import { test } from "node:test";
import * as jev from "../jev.mjs";
const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://jev.test/evaluate", TASKIX_JEV_API_KEY: "secret" };
const job = { id: "job_a", project_id: "p", status: "ACTIVE", revision: 3, title: "Login", prompt: "Fix login", review_policy: "required", conversation: [] };
const task = { id: "task_a", job_id: job.id, status: "IN_PROGRESS", phase: "EXECUTING", revision: 2, title: "Fix login with regression coverage", reason: null };
function answer(request, choice) {
    return { answers: Object.fromEntries(Object.entries(request.questions).map(([id, q]) => [id, {
        type: "choice", choice, confidence: .99, probabilities: Object.fromEntries(Object.keys(q.criteria).map(key => [key, key === choice ? 1 : 0])),
    }])) };
}
async function run(kind, choice, patch = {}) {
    let request, calls = 0;
    const selected = { ...task, ...patch.task };
    const context = { project_id: "p", job_id: job.id, task_id: task.id, routing: { complete: true, candidates: [{ job, tasks: [selected] }] } };
    const result = await jev.classifyLifecycle({ prompt: "Current visible evidence", assessment: { kind, job_id: job.id, task_id: task.id },
        context, options: { session: "s" }, env,
        runner: async args => {
            assert.deepEqual(args.slice(0, 2), ["routing", "revision"]);
            return { result: patch.stale ? { ...job, revision: 4 } : job };
        },
        fetch: async (_url, init) => { calls++; request = JSON.parse(init.body); return { ok: true, json: async () => answer(request, choice) }; }, ...patch });
    return { result, request, calls };
}
for (const choice of ["continue", "wait", "block", "fail", "ready"]) test(`execution_assessment_${choice}_is_read_only_and_revision_bound`, async () => {
    const { result, calls, request } = await run("outcome", choice);
    assert.equal(result.decision.action, choice);
    assert.equal(result.decision.task_id, task.id);
    assert.equal(result.decision.task_revision, 2);
    assert.equal(result.decision.job_revision, 3);
    assert.equal(calls, 1);
    assert.deepEqual(Object.keys(request.questions), ["outcome"]);
    assert.equal(result.decision.requires_verification, choice === "ready");
});
for (const [status, choice] of [["WAITING_USER", "resume"], ["BLOCKED", "resume"], ["FAILED", "retry"], ["DONE", "reopen"], ["CANCELLED", "reopen"], ["TODO", "cancel"], ["IN_PROGRESS", "release"]]) test(`recovery_${status}_${choice}`, async () => {
    const { result } = await run("recovery", choice, { task: { status, phase: null } });
    assert.equal(result.decision.action, choice);
});
test("recovery_keeps_waiting_when_reply_does_not_resolve_reason", async () => {
    const { result } = await run("recovery", "keep", { task: { status: "WAITING_USER", phase: null, reason: "Need target region" } });
    assert.equal(result.decision.action, "keep");
});
test("stale_assessment_defers", async () => {
    assert.equal((await run("outcome", "wait", { stale: true })).result.decision.action, "agent");
});
test("planning_is_not_ready_for_completion", async () => {
    assert.equal((await run("outcome", "ready", { task: { phase: "PLANNING" } })).result.decision.action, "agent");
});
test("disabled_assessment_performs_no_io", async () => {
    const { result, calls } = await run("outcome", "wait", { env: {} });
    assert.equal(result.decision.reason, "disabled");
    assert.equal(calls, 0);
});
test("uncertain_assessment_defers", async () => {
    assert.equal((await run("outcome", "uncertain")).result.decision.action, "agent");
});
test("recovery_selects_the_waiting_task_from_existing_job_context", async () => {
    const waiting = { ...task, status: "WAITING_USER", phase: null, reason: "Need region" };
    const { result, request } = await run("recovery", "resume:task_a", {
        assessment: { kind: "recovery", job_id: job.id },
        context: { project_id: "p", routing: { complete: true, candidates: [{ job, tasks: [waiting, { ...waiting, id: "task_b", reason: "Need account" }] }] } },
    });
    assert.equal(result.decision.action, "resume");
    assert.equal(result.decision.task_id, "task_a");
    assert.ok(request.questions.recovery.criteria["resume:task_b"]);
});
test("review_policy_assessment_reports_the_effective_required_action", async () => {
    const { result } = await run("review_policy", "none");
    assert.equal(result.decision.review_policy, "required");
    assert.equal(result.decision.action, "required");
});
for (const [choice, policy] of [["pending_review", "required"], ["completed", "none"]]) test(`completion_classifies_entire_job_destination_${choice}`, async () => {
    const { result, request, calls } = await run("completion", choice);
    assert.equal(result.decision.action, choice);
    assert.equal(result.decision.review_policy, policy);
    assert.equal(result.decision.job_revision, 3);
    assert.equal(calls, 1);
    assert.deepEqual(Object.keys(request.questions), ["completion"]);
});
test("completion_does_not_approve_an_already_pending_job", async () => {
    const context = { project_id: "p", routing: { complete: true, candidates: [{ job: { ...job, status: "PENDING_REVIEW" }, tasks: [] }] } };
    const { result, calls } = await run("completion", "completed", { context, assessment: { kind: "completion", job_id: job.id } });
    assert.equal(result.decision.action, "agent");
    assert.equal(calls, 0);
});
