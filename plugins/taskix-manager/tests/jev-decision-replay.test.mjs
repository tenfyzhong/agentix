import assert from "node:assert/strict";
import { test } from "node:test";

test("replay_summary_separates_correct_decisions_abstentions_and_wrong_accepts", async () => {
    const { replaySummary } = await import("./jev-decision-replay.mjs");
    const summary = replaySummary([
        { decision: { action: "new_job", review_policy: "required" }, expected: { action: "new_job", review_policy: "required" }, duration_ms: 100, request_bytes: 400 },
        { decision: { action: "agent" }, expected: { action: "wait" }, duration_ms: 200, request_bytes: 500 },
        { decision: { action: "approve" }, expected: { action: "followup" }, duration_ms: 300, request_bytes: 600 },
    ]);
    assert.equal(summary.correct, 1);
    assert.equal(summary.abstained, 1);
    assert.equal(summary.wrong_accepted, 1);
    assert.equal(summary.mean_ms, 200);
    assert.equal(summary.max_request_bytes, 600);
});
