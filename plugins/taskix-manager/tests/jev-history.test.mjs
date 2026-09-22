import assert from "node:assert/strict";
import { test } from "node:test";
import { historicalContext, messageTime } from "./jev-history.mjs";

test("transcript_history_stops_before_the_matched_request", async () => {
    const { transcriptHistory } = await import("./jev-history.mjs");
    const row = (role, content, seconds) => ({ type: "response_item", timestamp: new Date(seconds * 1000).toISOString(), payload: { type: "message", role, content } });
    const rows = [row("user", "Investigate routing", 100), row("assistant", "Increase context", 110),
        row("user", "按照建议进行修改", 120), row("assistant", "Future implementation", 130)];
    const result = transcriptHistory(rows, { prompt: "按照建议进行修改", message_time: 120 });
    assert.equal(result.message_time, 120);
    assert.deepEqual(result.history.map(m => m.text), ["Investigate routing", "Increase context"]);
});

test("transcript_history_rejects_ambiguous_or_distant_prompt_matches", async () => {
    const { transcriptHistory } = await import("./jev-history.mjs");
    const row = seconds => ({ type: "response_item", timestamp: new Date(seconds * 1000).toISOString(), payload: { type: "message", role: "user", content: "continue" } });
    assert.equal(transcriptHistory([row(100), row(200)], { prompt: "continue" }), undefined);
    assert.equal(transcriptHistory([row(100)], { prompt: "continue", message_time: 300 }), undefined);
    assert.equal(transcriptHistory([row(100), row(200)], { prompt: "continue", message_time: 200 }).message_time, 200);
});

test("transcript_history_ignores_host_context_reasoning_and_tool_content", async () => {
    const { transcriptHistory } = await import("./jev-history.mjs");
    const row = payload => ({ type: "response_item", timestamp: new Date(100000).toISOString(), payload: { type: "message", ...payload } });
    const rows = [row({ role: "user", content: "<recommended_plugins>injected</recommended_plugins>" }),
        row({ role: "assistant", channel: "analysis", content: "private" }),
        row({ role: "assistant", recipient: "tools.exec", content: "tool call" }),
        row({ role: "assistant", content: "Real advice" }), row({ role: "user", content: "continue" })];
    assert.deepEqual(transcriptHistory(rows, { prompt: "continue" }).history.map(m => m.text), ["Real advice"]);
});

test("message_time_uses_user_uuid_not_delayed_capture_time", () => {
    assert.equal(messageTime({ id: "01a07c6c-df04-7362-a91d-e959a9f522f7:msg_01a07c6c-df57-72c2-ae93-7c36d4232bb3", recorded_at: 1788794044 }), 1788793904.983);
    assert.equal(messageTime({ id: "opaque", recorded_at: 1788794044 }), undefined);
});

test("historical_context_preserves_status_and_excludes_future_snapshots", () => {
    const events = [
        { occurred_at: 10, payload: { id: "job_a", project_id: "p", status: "ACTIVE", title: "Original" } },
        { occurred_at: 11, payload: { id: "task_a", job_id: "job_a", status: "TODO" } },
        { occurred_at: 12, payload: { id: "job_b", project_id: "p", status: "COMPLETED" } },
        { occurred_at: 20, payload: { id: "job_a", project_id: "p", status: "COMPLETED", title: "Future" } },
    ];
    const context = historicalContext(events, 19, "p");
    assert.equal(context.routing.complete, true);
    assert.equal(context.routing.candidates.length, 1);
    assert.equal(context.routing.candidates[0].job.title, "Original");
    assert.equal(context.routing.candidates[0].tasks[0].id, "task_a");
    assert.equal(historicalContext(events, 21, "p").routing.candidates.length, 0);
});

test("unknown_message_time_retains_case_as_incomplete", () => {
    assert.equal(historicalContext([], undefined, "p").routing.complete, false);
});

test("reconstruction_keeps_every_case_and_does_not_use_future_source_metadata", async () => {
    const { reconstructHistory } = await import("./jev-history.mjs");
    const data = { cases: [{ id: "a", source_job_id: "job_a", prompt: "continue", history: [], context: { project_id: "p" } }] };
    const rebuilt = reconstructHistory(data, [], []);
    assert.equal(rebuilt.cases.length, 1);
    assert.equal(rebuilt.cases[0].context.routing.complete, false);
    assert.equal(rebuilt.cases[0].timestamp_available, false);
});

test("message_time_accepts_legacy_turn_colon_message_format", () => {
    assert.equal(messageTime({ id: "01a07c6c-df04-7362-a91d-e959a9f522f7:01a07c6c-df57-72c2-ae93-7c36d4232bb3", recorded_at: 1788794044 }), 1788793904.983);
});

test("reconstruction_excludes_current_turn_replies_and_other_sessions", async () => {
    const { reconstructHistory } = await import("./jev-history.mjs");
    const turn = "01a07c6c-df04-7362-a91d-e959a9f522f7";
    const current = { id: `${turn}:01a07c6c-df57-72c2-ae93-7c36d4232bb3`, role: "user", text: "continue", recorded_at: 1788794044, session_id: "s" };
    const history = [
        { id: "old", role: "assistant", text: "advice", session_id: "s" },
        { id: "foreign", role: "assistant", text: "other advice", session_id: "other" },
        { id: `${turn}:msg_hash`, role: "assistant", text: "already answering", session_id: "s" },
    ];
    const data = { cases: [{ source_job_id: "job_a", prompt: "continue", history, context: { project_id: "p" } }] };
    const result = reconstructHistory(data, [{ id: "job_a", conversation: [...history, current] }], []);
    assert.deepEqual(result.cases[0].history, [history[0]]);
});
test("goal_creation_recovers_request_time_without_using_continuation_wrappers", async () => {
    const { initialGoalHistory } = await import("./jev-history.mjs");
    const time = Date.parse("2026-09-07T18:14:05.540Z") / 1000;
    const message = text => ({ timestamp: new Date((time - 1) * 1000).toISOString(), type: "response_item",
        payload: { type: "message", role: "assistant", content: text } });
    const goal = { threadId: "session", objective: "Implement the advice", status: "active",
        tokensUsed: 0, timeUsedSeconds: 0, createdAt: Math.floor(time) };
    const event = { timestamp: new Date(time * 1000).toISOString(), type: "event_msg",
        payload: { type: "thread_goal_updated", threadId: "session", goal } };
    const sample = { session_ref: "session", prompt: "Implement the advice" };
    const rows = [message("Earlier advice"), event, message("Future answer")];
    const actual = initialGoalHistory(rows, sample);
    assert.equal(actual.message_time, time);
    assert.deepEqual(actual.history.map(m => m.text), ["Earlier advice"]);
    assert.equal(initialGoalHistory(rows, { ...sample, session_ref: "other" }), undefined);
    assert.equal(initialGoalHistory([...rows, event], sample), undefined);
    assert.equal(initialGoalHistory([{ ...event, payload: { ...event.payload,
        goal: { ...goal, tokensUsed: 100 } } }], sample), undefined);
    assert.equal(initialGoalHistory([{ ...event, timestamp: new Date((time + 30) * 1000).toISOString() }], sample), undefined);
});
test("image_caption_history_requires_real_attachment_and_unique_exact_caption", async () => {
    const { imageCaptionHistory } = await import("./jev-history.mjs");
    const row = (role, content, seconds) => ({ type: "response_item", timestamp: new Date(seconds * 1000).toISOString(),
        payload: { type: "message", role, content } });
    const attached = row("user", [{ type: "input_text", text: '<image name=[Image #1] path="/tmp/a.png">' },
        { type: "input_image", image_url: "data:image/png;base64,AA==" },
        { type: "input_text", text: "</image>\nRemove the blank line [Image #1]" }], 120);
    const rows = [row("assistant", "Earlier advice", 100), attached, row("assistant", "Future answer", 130)];
    const sample = { prompt: "Remove the blank line [Image #1]" };
    assert.equal(imageCaptionHistory(rows, sample).message_time, 120);
    assert.deepEqual(imageCaptionHistory(rows, sample).history.map(m => m.text), ["Earlier advice"]);
    assert.equal(imageCaptionHistory([...rows, attached], sample), undefined);
    assert.equal(imageCaptionHistory(rows, { prompt: "Remove the blank line" }), undefined);
    assert.equal(imageCaptionHistory([row("user", [{ type: "input_text", text:
        '<image name=[Image #1] path="/tmp/a.png">\n</image>\nRemove the blank line [Image #1]' }], 120)], sample), undefined);
});
