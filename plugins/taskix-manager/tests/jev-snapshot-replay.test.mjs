import assert from "node:assert/strict";
import { test } from "node:test";

const candidate = (patch = {}, tasks = []) => ({ job: { id: "job_a", project_id: "p", status: "ACTIVE",
    title: "Work", prompt: "Implement", goal: "Deliver", conversation: [], ...patch }, tasks });
const context = candidates => ({ project_id: "p", routing: { complete: true, candidates } });

test("historical_snapshot_uses_production_sql_text_and_history_bounds", async () => {
    const { boundedSnapshot } = await import("./jev-snapshot-replay.mjs");
    const messages = Array.from({ length: 10 }, (_, i) => ({ role: "assistant", text: `${i}`.repeat(3000) }));
    const result = boundedSnapshot(context([candidate({ prompt: "界".repeat(4001), conversation: messages })]));
    assert.equal(result.routing.complete, false);
    assert.equal(result.routing.candidates[0].job.prompt.length, 4000);
    const history = result.routing.candidates[0].job.conversation;
    assert.equal(history.length, 6);
    assert.equal(history[0].text, "4".repeat(2000));
    assert.equal(history[0].excerpt, true);
});

test("historical_snapshot_separates_completed_evidence_from_unfinished_budget", async () => {
    const { boundedSnapshot } = await import("./jev-snapshot-replay.mjs");
    const tasks = Array.from({ length: 300 }, (_, i) => ({ id: `t${i}`, job_id: "job_a", title: `Done ${i}`, status: "DONE" }));
    tasks.push({ id: "current", job_id: "job_a", title: "Current", status: "TODO" });
    const result = boundedSnapshot(context([candidate({}, tasks)]));
    assert.equal(result.routing.complete, true);
    assert.equal(result.routing.candidates[0].tasks.length, 1);
    assert.equal(result.routing.candidates[0].job.completed_tasks.length, 8);
    assert.equal(result.routing.candidates[0].job.completed_tasks[0].title, "Done 292");
});

test("historical_snapshot_preserves_unknown_time_and_reports_candidate_overflow", async () => {
    const { boundedSnapshot } = await import("./jev-snapshot-replay.mjs");
    assert.equal(boundedSnapshot({ ...context([]), routing: { complete: false, candidates: [] } }).routing.complete, false);
    const result = boundedSnapshot(context(Array.from({ length: 33 }, (_, i) => candidate({ id: `job_${i}` }))));
    assert.equal(result.routing.complete, false);
    assert.equal(result.routing.candidates.length, 32);
});
