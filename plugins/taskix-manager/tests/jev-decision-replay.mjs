import assert from "node:assert/strict";
import { routePrompt, classifyLifecycle } from "../jev.mjs";

export function replaySummary(results) {
    const matches = r => Object.entries(r.expected || {}).every(([key, value]) => r.decision[key] === value);
    const accepted = results.filter(r => r.decision.action !== "agent");
    const durations = results.map(r => r.duration_ms).sort((a, b) => a - b);
    return { total: results.length, accepted: accepted.length, abstained: results.length - accepted.length,
        correct: results.filter(matches).length, wrong_accepted: accepted.filter(r => !matches(r)).length,
        mean_ms: results.length ? results.reduce((sum, r) => sum + r.duration_ms, 0) / results.length : 0,
        p95_ms: durations[Math.max(0, Math.ceil(durations.length * .95) - 1)] ?? 0,
        max_request_bytes: Math.max(0, ...results.map(r => r.request_bytes)) };
}

export async function replayDecision(sample, settings = {}) {
    const start = performance.now();
    let requestBytes = 0, usage, answers, responseModel;
    const classify = sample.assessment ? classifyLifecycle : routePrompt;
    const result = await classify({ prompt: sample.prompt, history: sample.history || [], context: sample.context,
        assessment: sample.assessment, options: { session: sample.session_ref || "replay" }, telemetry: {}, ...settings,
        runner: async args => {
            assert.deepEqual(args.slice(0, 2), ["routing", "revision"], "Replay must never execute lifecycle writes");
            return { result: sample.context.routing.candidates.find(c => c.job.id === args[2])?.job };
        },
        fetch: async (url, init) => {
            requestBytes = Buffer.byteLength(init.body);
            assert.ok(requestBytes <= 30000);
            const response = await (settings.fetch || globalThis.fetch)(url, init);
            if (response.ok) {
                const data = await response.clone().json();
                usage = data.usage; answers = data.answers; responseModel = data.model;
            }
            return response;
        } });
    assert.ok(result, "Replay requires enabled Jev");
    return { id: sample.id, source_job_id: sample.source_job_id, provenance: sample.provenance,
        expected: sample.expected, decision: result.decision, usage, answers, response_model: responseModel,
        duration_ms: Math.round(performance.now() - start), request_bytes: requestBytes };
}
