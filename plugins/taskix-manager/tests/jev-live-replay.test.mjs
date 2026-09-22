import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile, writeFile } from "node:fs/promises";
import { routePrompt, jevConfig } from "../jev.mjs";
import { scopeReplayRequest } from "./jev-scope-variant.mjs";
import { atomicReplay } from "./jev-replay-variants.mjs";

// Explicit opt-in: input contains private Job history, and this test calls the
// configured real provider. It never executes task lifecycle commands.
test("live_jev_replay_of_existing_job_conversations", {
    skip: !process.env.TASKIX_JEV_REPLAY_INPUT,
    timeout: 3600000,
}, async () => {
    const config = jevConfig();
    assert.ok(config, "Configure the existing Jev endpoint and API key");
    const output = process.env.TASKIX_JEV_REPLAY_OUTPUT;
    assert.ok(output, "Set a local output path for the replay report");
    const data = JSON.parse(await readFile(process.env.TASKIX_JEV_REPLAY_INPUT, "utf8"));
    assert.equal(data.schema_version, 1);
    assert.ok(data.cases.length > 0 && data.cases.length <= 2000);
    const results = [];
    for (const sample of data.cases) {
        const telemetry = {};
        const started = performance.now();
        let requestBytes = 0, usage, responseModel, captured;
        const variant = process.env.TASKIX_JEV_REPLAY_VARIANT;
        let result = await routePrompt({
            prompt: sample.prompt, context: sample.context, history: sample.history,
            options: { session: sample.session_ref || "jev-live-replay" }, telemetry,
            runner: async args => {
                assert.deepEqual(args.slice(0, 2), ["routing", "revision"]);
                return { result: sample.context.routing.candidates.find(c => c.job.id === args[2])?.job };
            },
            fetch: async (url, init) => {
                requestBytes = Buffer.byteLength(init.body);
                captured = JSON.parse(init.body);
                if (["atomic", "membership", "focused", "concise"].includes(variant)) return { ok: false };
                if (variant === "scope") {
                    init = { ...init, body: JSON.stringify(scopeReplayRequest(captured)) };
                    requestBytes = Buffer.byteLength(init.body);
                    if (requestBytes > 30000) {
                        telemetry.variant_context_too_large = true;
                        return { ok: false };
                    }
                }
                const response = await fetch(url, init);
                if (response.ok) { const data = await response.clone().json(); usage = data.usage; responseModel = data.model; }
                return response;
            },
        });
        if (["atomic", "membership", "focused", "concise"].includes(variant) && captured) {
            const replay = await atomicReplay(captured, config);
            result = { decision: replay.decision };
            telemetry.answers = replay.answers;
            usage = replay.usage; responseModel = replay.response_model; requestBytes = replay.request_bytes;
        }
        assert.ok(result, "Jev routing must be enabled");
        if (telemetry.variant_context_too_large) result.decision = { action: "agent", reason: "context_too_large" };
        results.push({
            id: sample.id, source_job_id: sample.source_job_id, source_title: sample.source_title,
            source_document: sample.source_document, prompt: sample.prompt, kind: sample.kind, split: sample.split,
            usage, response_model: responseModel,
            accepted: result.decision.action !== "agent", decision: result.decision,
            called: telemetry.called === true, answers: telemetry.answers || [],
            duration_ms: Math.round(performance.now() - started), request_bytes: requestBytes,
        });
        // Persist after every sample so interruption cannot discard paid calls.
        await writeFile(output, JSON.stringify({
            schema_version: 1, variant: variant || "production", model: config.model, threshold: config.threshold,
            method: data.method, requested: data.cases.length, completed: results.length,
            accepted: results.filter(r => r.accepted).length, results,
        }, null, 2), { mode: 0o600 });
        console.log(`${results.length}/${data.cases.length} ${sample.id}: ${result.decision.action}${result.decision.reason ? ` (${result.decision.reason})` : ""}`);
    }
});
