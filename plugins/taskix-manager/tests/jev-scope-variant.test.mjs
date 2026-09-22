import assert from "node:assert/strict";
import { test } from "node:test";
import { scopeReplayRequest } from "./jev-scope-variant.mjs";

test("scope_experiment_preserves_choices_and_does_not_mutate_the_production_request", () => {
    const request = { state: { candidates: [] }, questions: { route: {
        type: "choice", instructions: "Resolve references from dialogue.",
        criteria: { new_job: "Independent work", uncertain: "Unresolved referent" },
    } } };
    const result = scopeReplayRequest(request);
    assert.deepEqual(Object.keys(result.questions.route.criteria), Object.keys(request.questions.route.criteria));
    assert.equal(result.state.candidate_scope.complete, true);
    assert.equal(request.state.candidate_scope, undefined);
    assert.match(result.questions.route.instructions, /COMPLETED/);
    assert.equal(result.questions.route.criteria.uncertain, request.questions.route.criteria.uncertain);
});
