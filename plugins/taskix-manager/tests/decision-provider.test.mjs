import assert from "node:assert/strict";
import { test } from "node:test";
import { jevConfig, routePrompt, classifyDiscussionTurns, classifyLifecycle } from "../jev.mjs";

const legacy = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://typesafe.test/evaluate", TASKIX_JEV_API_KEY: "legacy-key" };
const openai = { TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: "openai", OPENAI_API_KEY: "openai-key" };
const custom = protocol => ({ TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: "acme", TASKIX_DECISION_PROTOCOL: protocol,
    TASKIX_DECISION_URL: "https://acme.test/evaluate", TASKIX_DECISION_API_KEY: "acme-key", TASKIX_DECISION_MODEL: "acme-model" });

test("decision_config_separates_provider_protocol_and_legacy_models", () => {
    assert.deepEqual(jevConfig(openai), { provider: "openai", protocol: "openai_decisions", url: "https://api.openai.com/v1/decisions", key: "openai-key", model: "gpt-6-luna", threshold: .65 });
    assert.equal(jevConfig(legacy).provider, "typesafe");
    assert.equal(jevConfig(legacy).protocol, "typesafe");
    assert.equal(jevConfig(legacy).model, "jev-latest");
    for (const protocol of ["typesafe", "openai_decisions"]) {
        assert.equal(jevConfig(custom(protocol)).provider, "acme");
        assert.equal(jevConfig(custom(protocol)).protocol, protocol);
    }
});

test("decision_config_new_values_override_legacy_without_cross_provider_credentials", () => {
    assert.equal(jevConfig({ ...legacy, TASKIX_DECISION_ENABLED: "false" }), undefined);
    assert.equal(jevConfig({ ...legacy, ...openai }).key, "openai-key");
    assert.equal(jevConfig({ ...legacy, TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: "openai" }), undefined);
    assert.equal(jevConfig({ OPENAI_API_KEY: "key" }), undefined);
    assert.equal(jevConfig({ ...legacy, TASKIX_DECISION_MIN_CONFIDENCE: ".8" }).threshold, .8);
    for (const field of ["PROTOCOL", "URL", "API_KEY", "MODEL"]) {
        const env = custom("openai_decisions"); delete env[`TASKIX_DECISION_${field}`];
        assert.equal(jevConfig({ ...legacy, ...env, OPENAI_API_KEY: "key" }), undefined, field);
    }
    for (const override of [{ TASKIX_DECISION_PROTOCOL: "invalid" }, { TASKIX_DECISION_PROVIDER: "bad\nprovider" },
        { TASKIX_DECISION_URL: "https://user:pass@acme.test" }, { TASKIX_DECISION_API_KEY: "bad\nkey" }, { TASKIX_DECISION_MIN_CONFIDENCE: "NaN" }]) {
        assert.equal(jevConfig({ ...custom("typesafe"), ...override }), undefined);
    }
});

function fetchAnswers(protocol, selections, captured, mutate = x => x) {
    return async (url, init) => {
        const body = JSON.parse(init.body); captured.push({ url, init, body });
        if (protocol === "typesafe") return { ok: true, json: async () => ({ answers: Object.fromEntries(Object.entries(body.questions).map(([name, q]) => {
            const choice = selections[name] || "unrelated";
            return [name, { type: "choice", choice, confidence: .99, probabilities: Object.fromEntries(Object.keys(q.criteria).map(value => [value, Number(value === choice)])) }];
        })) }) };
        assert.equal(body.state, undefined);
        assert.equal(body.input[0].content[0].type, "input_json");
        const reply = { answers: body.questions.map(q => {
            const choice = selections[q.name] || "unrelated";
            assert.ok(q.choices.every(c => typeof c.description === "string"));
            return { name: q.name, type: "choice", choice, confidence: .99, probabilities: q.choices.map(({ value }) => ({ value, probability: Number(value === choice) })) };
        }).reverse() };
        return { ok: true, json: async () => mutate(reply) };
    };
}
const job = { id: "j", project_id: "p", title: "Delivery", prompt: "Implement", status: "ACTIVE", revision: 2, review_policy: "required", conversation: [], completed_tasks_complete: true };
const task = { id: "t", job_id: "j", status: "IN_PROGRESS", phase: "EXECUTING", revision: 2, title: "Implement" };
const runner = async args => {
    if (args[0] === "routing" && args[1] === "candidates") return { result: { complete: true, candidates: [{ job, tasks: [task] }] } };
    if (args[0] === "routing" && args[1] === "revision") return { result: job };
    throw Error(`Unexpected write: ${args}`);
};
const options = { session: "s" };
for (const protocol of ["typesafe", "openai_decisions"]) {
    test(`third_party_${protocol}_routes_and_selects_discussion`, async () => {
        const captured = [], env = custom(protocol);
        const result = await routePrompt({ prompt: "Continue", context: { project_id: "p", issue_candidates: [] }, options, runner, env,
            fetch: fetchAnswers(protocol, { intent: "work", route: "resume:j", review_policy: "required" }, captured) });
        assert.equal(result.decision.action, "resume");
        assert.equal(captured[0].url, env.TASKIX_DECISION_URL);
        assert.equal(captured[0].init.headers.Authorization, "Bearer acme-key");
        assert.equal(captured[0].init.redirect, "error");
        if (protocol === "openai_decisions") {
            const description = captured[0].body.questions.find(q => q.name === "route").choices.find(c => c.value === "resume:j").description;
            assert.equal(JSON.parse(description).topic, "Delivery");
        }
        const discussion = await classifyDiscussionTurns({ target: { title: "Delivery", prompt: "Continue" }, currentTurn: "now", options, env,
            pending: { complete: true, revision: 1, turns: [{ turn_id: "before", messages: [{ role: "user", text: "Implement" }] }, { turn_id: "now", messages: [{ role: "user", text: "Continue" }] }] },
            fetch: fetchAnswers(protocol, { turn_0: "related" }, captured) });
        assert.equal(discussion.status, "selected");
        assert.deepEqual(discussion.turn_ids, ["before", "now"]);
    });
    for (const [kind, selection] of [["recovery", "keep"], ["outcome", "ready"], ["review_policy", "required"], ["completion", "pending_review"]]) {
        test(`third_party_${protocol}_${kind}_uses_same_guarded_pipeline`, async () => {
            const result = await classifyLifecycle({ prompt: "Continue", context: { project_id: "p", job_id: "j", task_id: "t" }, options, runner, env: custom(protocol),
                assessment: { kind, job_id: "j", task_id: "t" }, fetch: fetchAnswers(protocol, { [kind]: selection }, []) });
            assert.equal(result.decision.action, selection);
        });
    }
}
for (const [name, mutate] of Object.entries({
    refusal: r => { r.answers[r.answers.findIndex(a => a.name === "intent")] = { name: "intent", type: "refusal" }; return r; },
    missing: r => { r.answers = []; return r; },
    duplicate_name: r => { r.answers.push(r.answers[0]); return r; },
    duplicate_probability: r => { r.answers[0].probabilities.push(r.answers[0].probabilities[0]); return r; },
    invalid_probability: r => { r.answers.find(a => a.name === "intent").probabilities[0].probability = -1; return r; },
    low_confidence: r => { r.answers.forEach(a => a.confidence = .1); return r; },
})) test(`openai_${name}_falls_back_without_writes`, async () => {
    const result = await routePrompt({ prompt: "Continue", context: { project_id: "p" }, options, runner, env: openai,
        fetch: fetchAnswers("openai_decisions", { intent: "work", route: "resume:j", review_policy: "required" }, [], mutate) });
    assert.equal(result.decision.action, "agent");
});

test("openai_issue_matching_retains_explicit_candidates", async () => {
    const issue = { id: "i", project_id: "p", status: "TODO", title: "Feature", revision: 1, content_hash: "hash" };
    const result = await routePrompt({ prompt: "Implement feature", context: { project_id: "p", issue_candidates: [issue], issue_candidates_complete: true }, options,
        runner: async args => {
            if (args[0] === "routing" && args[1] === "candidates") return { result: { complete: true, candidates: [] } };
            if (args[0] === "obsidian" && args[1] === "sync") return { result: {} };
            if (args[0] === "issue" && args[1] === "show") return { result: { issue, requirement: "Feature", acceptance: "Works", milestone: null } };
            throw Error(`Unexpected write: ${args}`);
        }, env: openai, fetch: fetchAnswers("openai_decisions", { intent: "work", route: "new_job", review_policy: "required", issue: "i" }, []) });
    assert.equal(result.decision.action, "new_job");
    assert.equal(result.decision.issue_id, "i");
});

test("openai_transport_errors_abort_and_actual_payload_limits_defer", async () => {
    for (const fetch of [async () => { throw Error("offline"); }, async () => ({ ok: false }), async () => ({ ok: true, json: async () => { throw Error("invalid JSON"); } }),
        async () => ({ ok: true, headers: new Headers({ "content-length": String(2 ** 21) }) })]) {
        const result = await routePrompt({ prompt: "Continue", context: { project_id: "p" }, options, runner, env: openai, fetch });
        assert.equal(result.decision.action, "agent");
    }
    for (const args of [{ prompt: "界".repeat(30000), options }, { prompt: "Continue", options: { ...options, signal: AbortSignal.abort() } }]) {
        let called = false;
        const result = await routePrompt({ ...args, context: { project_id: "p" }, runner, env: openai, fetch: async () => { called = true; throw Error("unexpected request"); } });
        assert.equal(result.decision.action, "agent");
        assert.equal(called, false);
    }
});

test("shared_configuration_contract_matches_rust_memory_service", async () => {
    const { readFile } = await import("node:fs/promises");
    const cases = JSON.parse(await readFile(new URL("./fixtures/decision-config.json", import.meta.url), "utf8"));
    for (const { name, env, expected } of cases) {
        const actual = jevConfig(env);
        if (!expected) assert.equal(actual, undefined, name);
        else assert.deepEqual(actual, { ...expected, threshold: .65 }, name);
    }
});
