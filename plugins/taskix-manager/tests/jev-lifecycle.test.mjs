import assert from "node:assert/strict";
import { test } from "node:test";
import { routePrompt } from "../jev.mjs";

const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://jev.test/evaluate", TASKIX_JEV_API_KEY: "secret" };
const job = { id: "job_a", project_id: "p", status: "PENDING_REVIEW", revision: 3,
    title: "Login", prompt: "Fix login", review_policy: "required", conversation: [] };
export function answers(request, choices) {
    return { answers: Object.fromEntries(Object.entries(request.questions).map(([id, question]) => {
        const choice = choices[id] ?? (id.startsWith("inbox_") ? "unrelated" : "uncertain");
        return [id, { type: "choice", choice, confidence: .99,
            probabilities: Object.fromEntries(Object.keys(question.criteria).map(key => [key, key === choice ? 1 : 0])) }];
    })) };
}
async function run(choices, patch = {}) {
    let calls = 0, request;
    const candidate = patch.job ?? job;
    const result = await routePrompt({ prompt: "User request", context: { project_id: "p", routing: {
        complete: true, candidates: patch.empty ? [] : [{ job: candidate, tasks: patch.tasks ?? [] }],
    } }, options: { session: "s" }, env, ...patch,
    runner: async args => { assert.deepEqual(args.slice(0, 2), ["routing", "revision"]); return { result: candidate }; },
    fetch: async (_url, init) => { calls++; request = JSON.parse(init.body); return { ok: true, json: async () => answers(request, choices) }; } });
    return { result, calls, request };
}
for (const policy of ["required", "none"]) test(`new_job_classifies_review_policy_${policy}_in_one_request`, async () => {
    const { result, calls } = await run({ intent: "work", route: "new_job", review_policy: policy }, { empty: true });
    assert.equal(result.decision.action, "new_job");
    assert.equal(result.decision.review_policy, policy);
    assert.equal(calls, 1);
});
test("existing_required_review_cannot_be_downgraded_by_classifier", async () => {
    const { result } = await run({ intent: "work", route: "followup:job_a", review_policy: "none" });
    assert.equal(result.decision.review_policy, "required");
});
test("code_supplement_upgrades_investigation_review", async () => {
    const { result } = await run({ intent: "work", route: "followup:job_a", review_policy: "required" }, { job: { ...job, review_policy: "none" } });
    assert.equal(result.decision.review_policy, "required");
});
for (const action of ["approve", "reject", "cancel"]) test(`explicit_job_${action}_is_not_followup`, async () => {
    const { result } = await run({ intent: action, route: "followup:job_a", review_policy: "not_applicable" });
    assert.equal(result.decision.action, action);
    assert.equal(result.decision.job_id, job.id);
    assert.equal(result.decision.job_revision, 3);
});
test("approval_without_existing_job_defers", async () => {
    const { result } = await run({ intent: "approve", route: "new_job", review_policy: "not_applicable" }, { empty: true });
    assert.equal(result.decision.action, "agent");
});
test("discussion_does_not_require_a_policy_decision", async () => {
    const { result } = await run({ intent: "question", route: "followup:job_a", review_policy: "uncertain" });
    assert.equal(result.decision.action, "discussion");
});
test("uncertain_work_policy_defers_without_guessing", async () => {
    const { result } = await run({ intent: "work", route: "new_job", review_policy: "uncertain" }, { empty: true });
    assert.equal(result.decision.action, "agent");
});
test("disabled_routing_keeps_existing_workflow_without_requests", async () => {
    const { result, calls } = await run({}, { env: { ...env, TASKIX_JEV_ENABLED: "false" } });
    assert.equal(result, undefined);
    assert.equal(calls, 0);
});
test("deadline_expiring_during_revision_check_cannot_accept_a_route", async () => {
    const controller = new AbortController();
    const result = await routePrompt({ prompt: "Create PR", context: { project_id: "p", routing: { complete: true, candidates: [{ job, tasks: [] }] } }, options: { session: "s", signal: controller.signal }, env,
        runner: async args => { controller.abort(); return { result: job }; },
        fetch: async (_url, init) => ({ ok: true, json: async () => answers(JSON.parse(init.body), { intent: "work", route: "followup:job_a", review_policy: "required" }) }) });
    assert.equal(result.decision.action, "agent");
});
test("review_scope_separates_delivery_operations_from_taskix_lifecycle_commands", async () => {
    const { request } = await run({ intent: "work", route: "new_job", review_policy: "none" }, { empty: true });
    const criteria = request.questions.review_policy.criteria;
    assert.match(criteria.none, /commit.*push.*tag.*release.*PR/i);
    assert.match(criteria.not_applicable, /Taskix Job\/Task/);
    assert.doesNotMatch(criteria.not_applicable, /without new implementation/);
});
