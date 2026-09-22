import assert from "node:assert/strict";
import { test } from "node:test";

const answer = (question, choice, confidence) => ({ question, choice, confidence,
    probability: .8, margin: .6, valid: 1 });
const sample = { id: "a", accepted: false, decision: { action: "agent", reason: "uncertain_or_conflicting" },
    answers: [answer("intent", "work", 1), answer("route", "resume:job_a", .7)] };

test("threshold_projection_rechecks_native_scores_without_new_inference", async () => {
    const { thresholdProjection } = await import("./jev-thresholds.mjs");
    assert.equal(thresholdProjection([sample], .9).accepted, 0);
    assert.equal(thresholdProjection([sample], .65).accepted, 1);
});

test("lower_threshold_preserves_uncertainty_margin_and_non_score_fallbacks", async () => {
    const { thresholdProjection } = await import("./jev-thresholds.mjs");
    const values = [
        { ...sample, answers: [answer("intent", "work", 1), answer("route", "uncertain", 1)] },
        { ...sample, answers: [answer("intent", "work", 1), { ...answer("route", "new_job", 1), margin: .1 }] },
        { ...sample, decision: { action: "agent", reason: "assignment_conflict" } },
        { ...sample, answers: [answer("intent", "work", 1)] },
        { ...sample, answers: [answer("intent", "work", 1), { ...answer("route", "new_job", 1), valid: 0 }] },
    ];
    assert.equal(thresholdProjection(values, .5).accepted, 0);
    assert.equal(thresholdProjection(values, .5).total, values.length);
});

test("threshold_projection_reports_added_cases_and_rejects_invalid_thresholds", async () => {
    const { thresholdProjection } = await import("./jev-thresholds.mjs");
    assert.deepEqual(thresholdProjection([sample], .65).newly_accepted_ids, ["a"]);
    for (const threshold of [.49, 1.1, NaN]) assert.throws(() => thresholdProjection([sample], threshold));
});
