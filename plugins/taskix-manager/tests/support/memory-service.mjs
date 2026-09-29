// Invoked by the Rust CLI integration fixture with an isolated real service.
import test from "node:test";
import assert from "node:assert/strict";
import { runHook, runTaskix } from "../../runtime.mjs";

test("real prompt hook consumes extracted memory and deduplicates later turns", async () => {
    const event = { hook_event_name: "UserPromptSubmit", session_id: "acceptance-host", cwd: process.cwd(), prompt: "Offline recovery", turn_id: "first" };
    const routing = { env: { TASKIX_JEV_ENABLED: "false" } };
    const first = await runHook(event, runTaskix, routing);
    assert.match(first.hookSpecificOutput.additionalContext, /Offline recovery/);
    const next = await runHook({ ...event, turn_id: "second" }, runTaskix, routing);
    assert.ok(!next?.hookSpecificOutput?.additionalContext?.includes("Offline recovery"));
});
