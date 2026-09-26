import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile, writeFile } from "node:fs/promises";
import { replayDecision, replaySummary } from "./jev-decision-replay.mjs";

// Explicit local fixtures only. No real task database is opened or changed.
test("live_lifecycle_replay_of_explicitly_selected_history", {
    skip: !process.env.TASKIX_JEV_LIFECYCLE_REPLAY_INPUT, timeout: 3600000,
}, async () => {
    const data = JSON.parse(await readFile(process.env.TASKIX_JEV_LIFECYCLE_REPLAY_INPUT, "utf8"));
    const output = process.env.TASKIX_JEV_LIFECYCLE_REPLAY_OUTPUT;
    assert.ok(output && data.schema_version === 1 && data.cases.length > 0 && data.cases.length <= 2000);
    const results = [];
    for (const sample of data.cases) {
        results.push(await replayDecision(sample));
        await writeFile(output, JSON.stringify({ schema_version: 1, method: data.method, requested: data.cases.length,
            summary: replaySummary(results), results }, null, 2), { mode: 0o600 });
        console.log(`${results.length}/${data.cases.length} ${sample.id}: ${results.at(-1).decision.action}`);
    }
});
