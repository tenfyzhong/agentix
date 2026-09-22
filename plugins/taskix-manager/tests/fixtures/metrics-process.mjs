// A real independent hook process for concurrent metrics and latency tests.
import { runHook } from "../../runtime.mjs";
const [directory, mode, session, clock] = process.argv.slice(2);
// Concurrent persistence checks must not depend on CI worker startup speed.
// Latency samples retain the real production deadline.
if (clock === "controlled") {
    const { mock } = await import("node:test");
    mock.timers.enable({ apis: ["setTimeout"] });
}
const started = performance.now();
const result = await runHook({ hook_event_name: "UserPromptSubmit", session_id: session, cwd: directory, prompt: "Discuss routing" }, async args => {
    if (args[1] !== "snapshot") throw new Error("Unexpected lifecycle call");
    return { result: { project_id: "fixture", inbox_todos: [], routing: { complete: true, candidates: [] } } };
}, {
    env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://unused.test", TASKIX_JEV_API_KEY: "fixture-key", TASKIX_JEV_METRICS_ENABLED: mode, TASKIX_JEV_METRICS_DB: `${directory}/metrics.sqlite` },
    cacheDir: directory,
    fetch: async (_url, init) => ({ ok: true, json: async () => ({ answers: Object.fromEntries(
        Object.entries(JSON.parse(init.body).questions).map(([id, question]) => {
            const choice = id === "intent" ? "question" : "new_job";
            return [id, { type: "choice", choice, confidence: 1,
                probabilities: Object.fromEntries(Object.keys(question.criteria).map(key => [key, key === choice ? 1 : 0])) }];
        }),
    ) }) }),
});
process.stdout.write(JSON.stringify({ elapsed_ms: performance.now() - started, routed: result.hookSpecificOutput.additionalContext.startsWith("Taskix route: discussion.") }));
