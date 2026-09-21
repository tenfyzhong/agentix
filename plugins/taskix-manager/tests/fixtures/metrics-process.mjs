// A real independent hook process for concurrent metrics and latency tests.
import { runHook } from "../../runtime.mjs";
const [directory, mode, session] = process.argv.slice(2);
const started = performance.now();
const result = await runHook({ hook_event_name: "UserPromptSubmit", session_id: session, cwd: directory, prompt: "Discuss routing" }, async args => {
    if (args[1] !== "snapshot") throw new Error("Unexpected lifecycle call");
    return { result: { project_id: "fixture", inbox_todos: [], routing: { complete: true, candidates: [] } } };
}, {
    env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://unused.test", TASKIX_JEV_API_KEY: "fixture-key", TASKIX_JEV_METRICS_ENABLED: mode, TASKIX_JEV_METRICS_DB: `${directory}/metrics.sqlite` },
    cacheDir: directory,
    fetch: async (_url, init) => ({ ok: true, json: async () => ({ answers: { route: {
        type: "choice", choice: "discussion", confidence: 1,
        probabilities: Object.fromEntries(Object.keys(JSON.parse(init.body).questions.route.criteria).map(key => [key, key === "discussion" ? 1 : 0])),
    } } }) }),
});
process.stdout.write(JSON.stringify({ elapsed_ms: performance.now() - started, routed: result.hookSpecificOutput.additionalContext.startsWith("Taskix route: discussion.") }));
