import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { metricsDatabase, metricsEnabled } from "../decision-provider.mjs";
import { test } from "node:test";
import { jevConfig, routePrompt, classifyDiscussionTurns, classifyLifecycle } from "../jev.mjs";

const typesafe = { TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_URL: "https://typesafe.test/evaluate", TASKIX_DECISION_API_KEY: "typesafe-key" };
const openai = { TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: "openai", OPENAI_API_KEY: "openai-key" };
const custom = protocol => ({ TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: "acme", TASKIX_DECISION_PROTOCOL: protocol,
    TASKIX_DECISION_URL: "https://acme.test/evaluate", TASKIX_DECISION_API_KEY: "acme-key", TASKIX_DECISION_MODEL: "acme-model" });

test("retired_jev_variables_do_not_configure_decisions_or_metrics", () => {
    const retired = {
        TASKIX_JEV_ENABLED: "true",
        TASKIX_JEV_API_KEY: "old-key",
        TASKIX_JEV_URL: "https://old.invalid/evaluate",
        TASKIX_JEV_MODEL: "old-model",
        TASKIX_JEV_MIN_CONFIDENCE: ".95",
        TASKIX_JEV_METRICS_ENABLED: "true",
        TASKIX_JEV_METRICS_DB: "/old/jev.sqlite",
    };
    assert.equal(jevConfig(retired), undefined);
    assert.equal(jevConfig({ ...retired, TASKIX_DECISION_ENABLED: "true" }), undefined);
    assert.equal(jevConfig({ ...retired, TASKIX_DECISION_API_KEY: "new-key" }), undefined);
    assert.deepEqual(jevConfig({ ...retired, TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_API_KEY: "new-key" }),
        { provider: "typesafe", protocol: "typesafe", key: "new-key", url: "https://api.typesafe.ai/v1/systemone", model: "jev-latest", threshold: .65 });
    assert.equal(metricsEnabled(retired), false);
    assert.equal(metricsDatabase(retired), undefined);
});

test("decision_config_separates_provider_protocol_and_typesafe_models", () => {
    assert.deepEqual(jevConfig(openai), { provider: "openai", protocol: "openai_decisions", url: "https://api.openai.com/v1/decisions", key: "openai-key", model: "gpt-6-luna", threshold: .65 });
    assert.equal(jevConfig(typesafe).provider, "typesafe");
    assert.equal(jevConfig(typesafe).protocol, "typesafe");
    assert.equal(jevConfig(typesafe).model, "jev-latest");
    for (const protocol of ["typesafe", "openai_decisions", "gemini"]) {
        assert.equal(jevConfig(custom(protocol)).provider, "acme");
        assert.equal(jevConfig(custom(protocol)).protocol, protocol);
    }
});

test("built_in_decision_urls_default_and_accept_full_endpoint_overrides", () => {
    const urls = {
        typesafe: "https://api.typesafe.ai/v1/systemone",
        openai: "https://api.openai.com/v1/decisions",
        google: "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent",
    };
    const override = "https://gateway.example.test/custom/evaluate";
    for (const [provider, url] of Object.entries(urls)) {
        const env = { TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: provider, TASKIX_DECISION_API_KEY: "fixture-key" };
        assert.equal(jevConfig(env)?.url, url, provider);
        assert.equal(jevConfig({ ...env, TASKIX_DECISION_URL: " " })?.url, url, provider);
        assert.equal(jevConfig({ ...env, TASKIX_DECISION_URL: override })?.url, override, provider);
        assert.equal(jevConfig({ ...env, TASKIX_DECISION_URL: "file:///invalid" }), undefined, provider);
    }
    assert.equal(jevConfig({ ...typesafe, TASKIX_DECISION_URL: "" })?.url, urls.typesafe);
    assert.equal(jevConfig({ ...typesafe, TASKIX_DECISION_URL: override })?.url, override);
    assert.equal(jevConfig({ ...custom("gemini"), TASKIX_DECISION_URL: "" }), undefined);
});

test("google_defaults_select_the_model_endpoint_and_only_gemini_credentials", () => {
    const env = { ...openai, TASKIX_DECISION_PROVIDER: "google", GEMINI_API_KEY: "google-key" };
    const url = "https://generativelanguage.googleapis.com/v1beta/models";
    assert.deepEqual(jevConfig(env), { provider: "google", protocol: "gemini", url: `${url}/gemini-3.8-flash:generateContent`,
        key: "google-key", model: "gemini-3.8-flash", threshold: .65 });
    const override = jevConfig({ ...env, TASKIX_DECISION_MODEL: "gemini-2.5-flash", TASKIX_DECISION_API_KEY: "explicit-key" });
    assert.equal(override.url, `${url}/gemini-2.5-flash:generateContent`);
    assert.equal(override.key, "explicit-key");
    assert.equal(jevConfig({ ...env, GEMINI_API_KEY: "" }), undefined);
    assert.equal(jevConfig({ ...env, TASKIX_DECISION_MODEL: "../other" }), undefined);
    assert.equal(jevConfig({ ...env, TASKIX_DECISION_PROTOCOL: "openai_decisions" }), undefined);
    const { TASKIX_DECISION_API_KEY, ...thirdParty } = custom("gemini");
    assert.equal(jevConfig({ ...thirdParty, GEMINI_API_KEY: "google-key" }), undefined);
});

function geminiReply(decision) {
    const json = JSON.stringify(decision);
    return { candidates: [{ finishReason: "STOP", content: { parts: [
        { thought: true, text: "Internal reasoning" },
        { text: json.slice(0, 5) }, { text: json.slice(5) },
    ] } }] };
}

test("decision_config_validates_options_without_cross_provider_credentials", () => {
    assert.equal(jevConfig({ ...typesafe, TASKIX_DECISION_ENABLED: "false" }), undefined);
    assert.equal(jevConfig({ TASKIX_DECISION_ENABLED: "true", TASKIX_DECISION_PROVIDER: "openai" }), undefined);
    assert.equal(jevConfig({ OPENAI_API_KEY: "key" }), undefined);
    assert.equal(jevConfig({ ...typesafe, TASKIX_DECISION_MIN_CONFIDENCE: ".8" }).threshold, .8);
    for (const field of ["PROTOCOL", "URL", "API_KEY", "MODEL"]) {
        const env = custom("openai_decisions"); delete env[`TASKIX_DECISION_${field}`];
        assert.equal(jevConfig({ ...env, OPENAI_API_KEY: "key" }), undefined, field);
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
        let questions;
        if (protocol === "gemini") {
            assert.equal(init.headers["x-goog-api-key"], "acme-key");
            assert.equal(init.headers.Authorization, undefined);
            assert.equal(body.generationConfig.responseMimeType, "application/json");
            assert.equal(body.generationConfig.responseJsonSchema.properties.answers.type, "array");
            assert.match(body.systemInstruction.parts[0].text, /untrusted/i);
            const input = JSON.parse(body.contents[0].parts[0].text);
            assert.ok(input.state);
            questions = input.questions;
        } else {
            assert.equal(body.state, undefined);
            assert.equal(body.input[0].content[0].type, "input_json");
            questions = body.questions;
        }
        const reply = { answers: questions.map(q => {
            const choice = selections[q.name] || "unrelated";
            assert.ok(q.choices.every(c => typeof c.description === "string"));
            return { name: q.name, type: "choice", choice, confidence: .99, probabilities: q.choices.map(({ value }) => ({ value, probability: Number(value === choice) })) };
        }).reverse() };
        return { ok: true, json: async () => mutate(protocol === "gemini" ? geminiReply(reply) : reply) };
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
for (const protocol of ["typesafe", "openai_decisions", "gemini"]) {
    test(`third_party_${protocol}_routes_and_selects_discussion`, async () => {
        const captured = [], env = custom(protocol);
        const result = await routePrompt({ prompt: "Continue", context: { project_id: "p", issue_candidates: [] }, options, runner, env,
            fetch: fetchAnswers(protocol, { intent: "work", route: "resume:j", review_policy: "required" }, captured) });
        assert.equal(result.decision.action, "resume");
        assert.equal(captured[0].url, env.TASKIX_DECISION_URL);
        if (protocol !== "gemini") assert.equal(captured[0].init.headers.Authorization, "Bearer acme-key");
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

for (const protocol of ["openai_decisions", "gemini"]) test(`${protocol}_issue_matching_retains_explicit_candidates`, async () => {
    const issue = { id: "i", project_id: "p", status: "TODO", title: "Feature", revision: 1, content_hash: "hash" };
    const result = await routePrompt({ prompt: "Implement feature", context: { project_id: "p", issue_candidates: [issue], issue_candidates_complete: true }, options,
        runner: async args => {
            if (args[0] === "routing" && args[1] === "candidates") return { result: { complete: true, candidates: [] } };
            if (args[0] === "obsidian" && args[1] === "sync") return { result: {} };
            if (args[0] === "issue" && args[1] === "show") return { result: { issue, requirement: "Feature", acceptance: "Works", milestone: null } };
            throw Error(`Unexpected write: ${args}`);
        }, env: custom(protocol), fetch: fetchAnswers(protocol, { intent: "work", route: "new_job", review_policy: "required", issue: "i" }, []) });
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

test("decision_configuration_contract_matches_fixtures", async () => {
    const { readFile } = await import("node:fs/promises");
    const cases = JSON.parse(await readFile(new URL("./fixtures/decision-config.json", import.meta.url), "utf8"));
    for (const { name, env, expected } of cases) {
        const actual = jevConfig(env);
        if (!expected) assert.equal(actual, undefined, name);
        else assert.deepEqual(actual, { ...expected, threshold: .65 }, name);
    }
});

test("gemini_blocked_truncated_or_unusable_answers_defer_to_agent", async () => {
    for (const mutate of [
        () => ({ promptFeedback: { blockReason: "SAFETY" } }),
        reply => { reply.candidates[0].finishReason = "MAX_TOKENS"; return reply; },
        reply => { reply.candidates.push(reply.candidates[0]); return reply; },
        () => geminiReply({ answers: [] }),
        () => geminiReply({ answers: [{ name: "intent", type: "choice", choice: "work",
            confidence: .99, probabilities: [{ value: "work", probability: 1 }, { value: "work", probability: 0 }] }] }),
        reply => { reply.candidates[0].content.parts = [{ text: "broken JSON" }]; return reply; },
    ]) {
        const result = await routePrompt({ prompt: "Continue", context: { project_id: "p" }, options, runner,
            env: custom("gemini"), fetch: fetchAnswers("gemini", { intent: "work", route: "resume:j", review_policy: "required" }, [], mutate) });
        assert.equal(result.decision.action, "agent");
    }
});

async function loopbackServer(t, handler) {
    const server = createServer(handler);
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    t.after(() => new Promise(resolve => server.close(resolve)));
    return { server, url: `http://127.0.0.1:${server.address().port}` };
}

const [nodeMajor, nodeMinor] = process.versions.node.split(".").map(Number);
const nativeProxy = nodeMajor > 24 || (nodeMajor === 24 && nodeMinor >= 5)
    || (nodeMajor === 22 && nodeMinor >= 21);
const proxyTest = { skip: !nativeProxy };

for (const protocol of ["typesafe", "openai_decisions", "gemini"]) {
    test(`${protocol} reuses the supplied proxy environment and respects NO_PROXY`, proxyTest, async t => {
        const requests = [];
        const expectedAuth = protocol === "gemini" ? ["acme-key", undefined] : [undefined, "Bearer acme-key"];
        const answer = async (request, response, via) => {
            const { authorization, "x-goog-api-key": key } = request.headers;
            requests.push({ via, url: request.url, auth: [key, authorization] });
            const chunks = [];
            for await (const chunk of request) chunks.push(chunk);
            const body = Buffer.concat(chunks).toString("utf8");
            const reply = await fetchAnswers(protocol, { intent: "work", route: "resume:j", review_policy: "required" }, [])(request.url, {
                body, headers: { "x-goog-api-key": request.headers["x-goog-api-key"] },
            });
            response.end(JSON.stringify(await reply.json()));
        };
        const origin = await loopbackServer(t, (req, res) => answer(req, res, "origin"));
        const proxy = await loopbackServer(t, (req, res) => answer(req, res, "proxy"));
        const url = origin.url + "/evaluate";
        const env = { ...custom(protocol), TASKIX_DECISION_URL: url, http_proxy: proxy.url };
        const args = { prompt: "Continue", context: { project_id: "p" }, options, runner };
        const globalFetch = globalThis.fetch;
        assert.equal((await routePrompt({ ...args, env })).decision.action, "resume");
        assert.equal((await routePrompt({ ...args, env: { ...env, NO_PROXY: "127.0.0.1" } })).decision.action, "resume");
        assert.equal(globalThis.fetch, globalFetch);
        assert.deepEqual(requests, [
            { via: "proxy", url, auth: expectedAuth },
            { via: "origin", url: "/evaluate", auth: expectedAuth },
        ]);
    });

    test(`${protocol} uses HTTPS CONNECT and defers on proxy refusal`, proxyTest, async t => {
        const proxy = await loopbackServer(t, (_req, res) => res.writeHead(500).end());
        const connects = [];
        proxy.server.on("connect", (req, socket) => {
            connects.push({ url: req.url, auth: [req.headers["x-goog-api-key"], req.headers.authorization] });
            socket.end("HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n");
        });
        const env = { ...custom(protocol), TASKIX_DECISION_URL: "https://decisions.example.invalid/evaluate", HTTPS_PROXY: proxy.url };
        const result = await routePrompt({ prompt: "Continue", context: { project_id: "p" }, options, runner, env });
        assert.equal(result.decision.action, "agent");
        assert.deepEqual(connects, [{ url: "decisions.example.invalid:443", auth: [undefined, undefined] }]);
    });
}
