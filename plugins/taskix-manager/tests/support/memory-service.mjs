// Invoked by the Rust CLI integration fixture with an isolated real service.
import test from "node:test";
import assert from "node:assert/strict";
import { runHook, runTaskix } from "../../runtime.mjs";

test("long identifier-rich prompts retain memory through the real CLI and host", async () => {
    const prompt = `Offline recovery ${Array.from({ length: 140 }, (_, i) => `t${String(i).padStart(3, "0")}`).join(" ")}`;
    const output = await runHook({ hook_event_name: "UserPromptSubmit", session_id: "acceptance-long-prompt",
        cwd: process.cwd(), prompt, turn_id: "long-prompt" }, runTaskix, { env: { TASKIX_JEV_ENABLED: "false", TASKIX_MEMORY_ENABLED: "true" } });
    assert.match(output.hookSpecificOutput.additionalContext, /Offline recovery/);
});

test("real prompt hook consumes extracted memory and deduplicates later turns", async () => {
    const event = { hook_event_name: "UserPromptSubmit", session_id: "acceptance-host", cwd: process.cwd(), prompt: "Offline recovery", turn_id: "first" };
    const routing = { env: { TASKIX_JEV_ENABLED: "false", TASKIX_MEMORY_ENABLED: "true" } };
    const first = await runHook(event, runTaskix, routing);
    assert.match(first.hookSpecificOutput.additionalContext, /Offline recovery/);
    const next = await runHook({ ...event, turn_id: "second" }, runTaskix, routing);
    assert.ok(!next?.hookSpecificOutput?.additionalContext?.includes("Offline recovery"));
});

test("a packet prepared by the real service but not injected remains available to the host", async () => {
    const options = { session: "acceptance-undelivered-host", cwd: process.cwd() };
    const prepared = await runTaskix(["memory", "context", "Offline recovery", "--turn", "discarded"], options);
    assert.match(prepared.result.text, /Offline recovery/);
    // Discard the generated packet without running the host delivery path.
    const event = { hook_event_name: "UserPromptSubmit", session_id: options.session, cwd: options.cwd,
        prompt: "Offline recovery", turn_id: "received" };
    const routing = { env: { TASKIX_JEV_ENABLED: "false", TASKIX_MEMORY_ENABLED: "true" } };
    const received = await runHook(event, runTaskix, routing);
    assert.match(received.hookSpecificOutput.additionalContext, /Offline recovery/);
    const later = await runHook({ ...event, turn_id: "later" }, runTaskix, routing);
    assert.ok(!later?.hookSpecificOutput?.additionalContext?.includes("Offline recovery"));
});
