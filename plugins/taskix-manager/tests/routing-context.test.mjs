import assert from "node:assert/strict";
import { test } from "node:test";
import { agentFallback } from "../routing-context.mjs";

const routed = { decision: { reason: "uncertain_or_conflicting" } };
const decode = content => JSON.parse(content.slice(content.indexOf("\n") + 1));
const candidate = id => ({ job: { id, status: "PENDING_REVIEW", title: "Login", prompt: "Fix login" } });

test("fallback_is_compact_without_candidates_and_retains_ownership_rules", () => {
    const content = agentFallback({}, routed), facts = decode(content);
    assert.ok(content.length < 1200);
    assert.match(content, /discussion.*no.*lifecycle/i);
    assert.match(content, /taskix context/);
    assert.match(content, /--expect-revision/);
    assert.match(content, /Preserve.*assignment/);
    assert.equal(facts.candidate_count, 0);
    assert.equal(facts.inbox_count, 0);
    assert.equal(facts.candidates_complete, false);
    assert.equal(facts.full_sources_required, true);
    assert.deepEqual(facts.summaries, []);
});

test("fallback_preserves_assignment_and_uses_snapshot_candidates_without_leaking_secrets", () => {
    const context = {
        project_id: "prj_one", job_id: "job_owned", task_id: "task_owned", previous_job: { id: "job_previous" },
        task: { status: "WAITING_USER", reason: "Choose a region", lease: "secret_task" },
        routing: { complete: true, candidates: [candidate("job_candidate")] },
        inbox_todos: [{ id: "inbox_one", content: "Add search", lease: "secret_inbox" }],
        lease: { token: "secret_lease" }, api_key: "secret_key",
    };
    const before = structuredClone(context);
    const content = agentFallback(context, routed), facts = decode(content);
    assert.deepEqual(context, before, "formatting must not mutate routing evidence");
    assert.equal(facts.job_id, "job_owned");
    assert.equal(facts.task_id, "task_owned");
    assert.equal(facts.previous_job_id, "job_previous");
    assert.equal(facts.project_id, "prj_one");
    assert.equal(facts.candidates_complete, true);
    assert.deepEqual(facts.candidate_ids, ["job_candidate"]);
    assert.deepEqual(facts.inbox_ids, ["inbox_one"]);
    assert.deepEqual(facts.summaries, [
        { task_id: "task_owned", status: "WAITING_USER", reason: "Choose a region" },
        { job_id: "job_candidate", status: "PENDING_REVIEW", title: "Login", prompt: "Fix login" },
        { inbox_id: "inbox_one", content: "Add search" },
    ]);
    assert.doesNotMatch(content, /secret_/);
    assert.deepEqual(decode(agentFallback(context, { ...routed, candidates: [] })).candidate_ids, []);
});

for (const text of ["\u0000".repeat(256), "\"\\\n".repeat(256), "\u{1f600}".repeat(256)]) {
    test(`fallback_accounts_for_json_escaping_and_large_candidate_sets_${text.codePointAt(0)}`, () => {
        const context = {
            job_id: "job_owned", task_id: "task_owned", task: { status: "WAITING_USER", reason: text.repeat(100) },
            routing: { complete: false, candidates: Array.from({ length: 10000 }, (_, i) => ({
                job: { id: `job_${i}`, status: "PENDING_REVIEW", title: text, prompt: text },
            })) },
            inbox_todos: Array.from({ length: 10000 }, (_, i) => ({ id: `inbox_${i}`, content: text })),
        };
        const content = agentFallback(context, routed), facts = decode(content);
        assert.ok(content.length <= 12000, `${content.length} exceeds budget`);
        assert.equal(facts.candidate_count, 10000);
        assert.equal(facts.inbox_count, 10000);
        assert.equal(facts.candidates_complete, false);
        assert.equal(facts.candidate_ids.length, 32);
        assert.equal(facts.inbox_ids.length, 32);
        assert.equal(facts.task_id, "task_owned");
        assert.ok(facts.summaries.some(summary => summary.task_id === "task_owned"));
        for (const summary of facts.summaries) {
            for (const key of ["title", "prompt", "reason", "content"]) {
                if (summary[key]) assert.ok(summary[key].length <= 256);
            }
        }
    });
}

test("fallback_budget_includes_escaped_references_and_invalid_fields", () => {
    const id = "\u0000".repeat(128);
    const context = {
        project_id: id, job_id: id, task_id: id, previous_job: { id },
        task: { status: id.repeat(100), reason: null },
        routing: { candidates: Array.from({ length: 33 }, () => candidate(id)) },
        inbox_todos: Array.from({ length: 33 }, () => ({ id, content: "text" })),
    };
    const content = agentFallback(context, routed), facts = decode(content);
    assert.ok(content.length <= 12000);
    assert.equal(facts.job_id, id);
    assert.ok(facts.candidate_ids.length < 32);
    assert.ok(facts.inbox_ids.length < 32);
    const invalid = decode(agentFallback({ project_id: 3, job_id: "x".repeat(129),
        routing: { candidates: [candidate(null)] }, inbox_todos: [{ id: "", content: false }],
    }, routed));
    assert.equal(invalid.project_id, undefined);
    assert.equal(invalid.job_id, undefined);
    assert.deepEqual(invalid.candidate_ids, []);
    assert.deepEqual(invalid.inbox_ids, []);
});

// Opt-in diagnostic samples, never a timing threshold. No provider or task database.
test("fallback_rendering_benchmark", { skip: process.env.TASKIX_ROUTING_BENCH !== "1" }, t => {
    for (const count of [0, 1, 32, 10000]) {
        const context = { routing: { complete: count <= 32, candidates: Array.from({ length: count }, (_, i) => candidate(`job_${i}`)) } };
        const iterations = 1000, start = performance.now();
        let content;
        for (let i = 0; i < iterations; i++) content = agentFallback(context, routed);
        assert.ok(content.length <= 12000);
        t.diagnostic(JSON.stringify({ candidates: count, iterations, meanMs: (performance.now() - start) / iterations, contextChars: content.length }));
    }
});
