import assert from "node:assert/strict";
import { test } from "node:test";
import { routePrompt } from "../jev.mjs";
import { choiceAnswers } from "./support/jev.mjs";

const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://example.test/jev", TASKIX_JEV_API_KEY: "test" };
const item = { id: "issue_a", project_id: "p", status: "TODO", revision: 2, content_hash: "requirement-hash",
    title: "Login", requirement: "Repair login", acceptance: "Valid users can log in", milestone_id: "milestone_a" };
const job = { id: "job_a", project_id: "p", status: "ACTIVE", revision: 3, title: "Login round",
    prompt: "Repair login", review_policy: "required", conversation: [], issue_id: null };
async function run({ choice = "issue_a", linked, owned = true, explicit, explicitJob, fresh = item, route = "resume:job_a", intent = "work" } = {}) {
    const calls = [], selected = { ...job, issue_id: linked ?? null };
    const context = { project_id: "p", issue_candidates: [item], requested_issue_id: explicit,
        requested_job_id:explicitJob,
        routing: { complete: true, candidates: [{ job: selected, tasks: [] }] },
        ...(linked && owned ? { job_id: job.id, issue_id: linked } : {}) };
    let request;
    const result = await routePrompt({ prompt: "Repair login", context, options: { session: "s" }, env,
        runner: async args => {
            calls.push(args);
            if (args[0] === "routing" && args[1] === "revision") return { result: selected };
            if (args[0] === "obsidian" && args[1] === "sync") return { result: { id: args[2] } };
            if (args[0] === "issue" && args[1] === "show") return { result: { issue: fresh, milestone: { id: "milestone_a", goal: "Ship login" } } };
            throw new Error("Unexpected write");
        },
        fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => choiceAnswers(request, { route, intent, issue: choice }) };
        } });
    return { result, calls, request };
}

test("one_semantic_issue_is_reread_before_guarded_association", async () => {
    const { result, calls, request } = await run();
    assert.equal(result.decision.issue_id, item.id);
    assert.equal(result.decision.issue_revision, item.revision);
    assert.equal(result.decision.issue_content_hash, item.content_hash);
    assert.equal(result.context.issue.requirement, item.requirement);
    assert.equal(result.context.milestone.goal, "Ship login");
    assert.deepEqual(calls.filter(a => a[0] !== "routing"), [["obsidian", "sync", item.id], ["issue", "show", item.id]]);
    assert.deepEqual(Object.keys(request.questions.issue.criteria), ["none", "uncertain", item.id]);
    assert.equal(request.state.issues[0].acceptance, item.acceptance);
    assert.ok(!JSON.stringify(request).includes("inbox"));
});

test("explicit_im_job_round_is_preserved_when_the_model_proposes_a_new_job",async()=>{
    const {result}=await run({explicit:item.id,explicitJob:job.id,linked:item.id,owned:false,route:"new_job"});
    assert.equal(result.decision.action,"resume");
    assert.equal(result.decision.job_id,job.id);
});

test("current_job_issue_is_preserved_without_semantic_reselection", async () => {
    const { result, request } = await run({ linked: item.id, choice: "none" });
    assert.equal(result.decision.issue_id, item.id);
    assert.equal(request.questions.issue, undefined);
});

test("unmatched_job_remains_independent_and_does_not_read_issue", async () => {
    const { result, calls } = await run({ choice: "none", route: "new_job" });
    assert.equal(result.decision.action, "new_job");
    assert.equal(result.decision.issue_id, undefined);
    assert.ok(calls.every(a => a[0] === "routing"));
});

test("ambiguous_or_changed_issue_defers_without_a_link", async () => {
    for (const input of [{ choice: "uncertain" }, { fresh: { ...item, revision: 3 } },
        { fresh: { ...item, status: "CANCELLED" } }, { fresh: { ...item, project_id: "other" } }]) {
        const { result } = await run(input);
        assert.equal(result.decision.action, "agent");
        assert.equal(result.decision.issue_id, undefined);
    }
});

test("explicit_issue_has_priority_and_conflicting_owned_job_defers", async () => {
    const direct = await run({ explicit: item.id, choice: "none" });
    assert.equal(direct.result.decision.issue_id, item.id);
    assert.equal(direct.request.questions.issue, undefined);
    const conflict = await run({ linked: "issue_other", explicit: item.id });
    assert.equal(conflict.result.decision.action, "agent");
});

test("discussion_never_proposes_a_new_issue_link", async () => {
    const { result, calls } = await run({ intent: "question", choice: "uncertain" });
    assert.equal(result.decision.action, "discussion");
    assert.equal(result.decision.issue_id, undefined);
    assert.ok(calls.every(a => a[0] === "routing"));
});

test("selected_job_existing_issue_overrides_an_inapplicable_match_answer", async () => {
    const { result }=await run({ linked: item.id, owned:false, choice:"uncertain" });
    assert.equal(result.decision.issue_id,item.id);
});
