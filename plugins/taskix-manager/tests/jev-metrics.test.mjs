import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, rm, access, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { routePrompt } from "../jev.mjs";

async function fixture(t, enabled = true) {
    // Persistence assertions use real workers/SQLite but a controlled parent clock.
    // Cold worker startup on CI may exceed the best-effort production deadline.
    // The separate timeout test advances that clock and verifies the 250 ms bound;
    // subprocess latency/contended-write fixtures still use real clocks.
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const dir = await mkdtemp(join(tmpdir(), "jev-metrics-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const path = join(dir, "metrics.sqlite");
    const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://example.test", TASKIX_JEV_API_KEY: "private-key", TASKIX_JEV_METRICS_DB: path, ...(enabled ? { TASKIX_JEV_METRICS_ENABLED: "true" } : {}) };
    const args = { env, prompt: "private prompt", options: { session: "session", cwd: dir }, context: { project_id: "p", inbox_todos: [{ id: "inbox_a", content: "private inbox" }], routing: { complete: true, candidates: [] } }, runner: () => assert.fail("unexpected CLI"), fetch: async (_url, init) => ({ ok: true, json: async () => ({ answers: Object.fromEntries(Object.entries(JSON.parse(init.body).questions).map(([id, q]) => {
        const choice = id === "route" ? "new_job" : "unrelated";
        return [id, { type: "choice", choice, confidence: id === "route" ? .99 : .87, probabilities: Object.fromEntries(Object.keys(q.criteria).map(k => [k, k === choice ? 1 : 0])) }];
    })) }) }) };
    return { path, args };
}
async function rows(path, sql) {
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(path, { readOnly: true });
    try { return db.prepare(sql).all().map(row => ({ ...row })); } finally { db.close(); }
}

test("metrics_default_disabled_creates_no_database", async t => {
    const f = await fixture(t, false);
    await routePrompt(f.args);
    await assert.rejects(access(f.path));
});
test("metrics_records_all_question_scores_without_prompt_or_credentials", async t => {
    const f = await fixture(t);
    const result = await routePrompt(f.args);
    assert.equal(result.decision.action, "agent");
    const [request] = await rows(f.path, "SELECT * FROM requests");
    assert.equal(request.accepted, 0);
    assert.equal(request.called, 1);
    assert.equal(request.reason, "uncertain_or_conflicting");
    assert.equal(request.threshold, .9);
    assert.ok(request.duration_ms >= 0);
    const scores = await rows(f.path, "SELECT * FROM answers ORDER BY question");
    assert.equal(scores.length, 2);
    assert.equal(scores[0].confidence, .87);
    assert.equal(scores[0].issue, "low_confidence");
    assert.equal(scores[1].issue, null);
    assert.doesNotMatch((await readFile(f.path)).toString(), /private prompt|private inbox|private-key/);
});
test("metrics_records_accepted_and_pre_request_fallback", async t => {
    const f = await fixture(t);
    f.args.env.TASKIX_JEV_MIN_CONFIDENCE = ".85";
    await routePrompt(f.args);
    f.args.context.routing.complete = false;
    await routePrompt(f.args);
    const requests = await rows(f.path, "SELECT accepted, called, reason FROM requests ORDER BY rowid");
    assert.deepEqual(requests, [{ accepted: 1, called: 1, reason: null }, { accepted: 0, called: 0, reason: "incomplete_snapshot" }]);
});
test("metrics_storage_failure_does_not_change_routing", async t => {
    const f = await fixture(t);
    f.args.env.TASKIX_JEV_METRICS_DB = join(f.path, "..", "missing", "..", "bad.sqlite", "child");
    // A regular file as a parent makes the destination unwritable on every OS.
    const { writeFile } = await import("node:fs/promises");
    await writeFile(f.path, "not a directory");
    f.args.env.TASKIX_JEV_METRICS_DB = join(f.path, "child");
    assert.equal((await routePrompt(f.args)).decision.reason, "uncertain_or_conflicting");
});

test("metrics_hook_preparation_failure_is_recorded_once_without_calling_jev", async t => {
    const f = await fixture(t);
    const { runHook } = await import("../runtime.mjs");
    await runHook({ hook_event_name: "UserPromptSubmit", session_id: "s", turn_id: "turn_a", cwd: f.args.options.cwd, prompt: "private prompt" }, async () => { throw new Error("private CLI error"); }, { env: f.args.env, cacheDir: f.args.options.cwd });
    const requests = await rows(f.path, "SELECT * FROM requests");
    assert.equal(requests.length, 1);
    assert.equal(requests[0].reason, "preparation_unavailable");
    assert.equal(requests[0].called, 0);
    assert.equal(requests[0].turn_id, "turn_a");
});

test("metrics_disabled_does_not_touch_an_existing_database", async t => {
    const f = await fixture(t);
    await routePrompt(f.args);
    const before = await readFile(f.path);
    f.args.env.TASKIX_JEV_METRICS_ENABLED = "false";
    await routePrompt(f.args);
    assert.deepEqual(await readFile(f.path), before);
});

test("metrics_lock_contention_is_best_effort_and_preserves_route", async t => {
    const f = await fixture(t);
    await routePrompt(f.args);
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(f.path);
    db.exec("BEGIN IMMEDIATE");
    try { assert.equal((await routePrompt(f.args)).decision.reason, "uncertain_or_conflicting"); }
    finally { db.exec("ROLLBACK"); db.close(); }
    assert.equal((await rows(f.path, "SELECT * FROM requests")).length, 1);
});

test("metrics_successful_hook_writes_once", async t => {
    const f = await fixture(t);
    const { runHook } = await import("../runtime.mjs");
    await runHook({ hook_event_name: "UserPromptSubmit", session_id: "s", cwd: f.args.options.cwd, prompt: f.args.prompt }, async args => {
        assert.deepEqual(args, ["routing", "snapshot"]);
        return { result: f.args.context };
    }, { env: f.args.env, cacheDir: f.args.options.cwd, fetch: f.args.fetch });
    assert.equal((await rows(f.path, "SELECT * FROM requests")).length, 1);

});

test("metrics_invalid_answer_does_not_persist_unknown_model_text", async t => {
    const f = await fixture(t);
    f.args.fetch = async () => ({ ok: true, json: async () => ({ answers: { route: { type: "choice", choice: "unexpected private text", confidence: .99, probabilities: {} } } }) });
    await routePrompt(f.args);
    const answers = await rows(f.path, "SELECT * FROM answers");
    assert.ok(answers.every(row => row.issue === "invalid_answer"));
    assert.doesNotMatch((await readFile(f.path)).toString(), /unexpected private text/);
});

for (const patch of [{ TASKIX_JEV_ENABLED: "false" }, { TASKIX_JEV_API_KEY: "" }, { TASKIX_JEV_METRICS_ENABLED: "0" }]) test(`metrics_does_not_write_when_disabled_${JSON.stringify(patch)}`, async t => {
    const f = await fixture(t);
    Object.assign(f.args.env, patch);
    await routePrompt(f.args);
    await assert.rejects(access(f.path));
});

test("native_taskix_report_matches_plugin_database_and_preserves_read_only_data", { skip: !process.env.TASKIX_TEST_METRICS_BIN }, async t => {
    const f = await fixture(t);
    await routePrompt(f.args);
    f.args.env.TASKIX_JEV_MIN_CONFIDENCE = ".85";
    await routePrompt(f.args);
    const before = await readFile(f.path);
    const { execFile } = await import("node:child_process");
    const { promisify } = await import("node:util");
    const run = async (...args) => {
        const { stdout } = await promisify(execFile)(process.env.TASKIX_TEST_METRICS_BIN, ["routing", "metrics", ...args, "--json"], { env: { ...process.env, ...f.args.env, TASKIX_CONFIG: join(f.args.options.cwd, "absent.toml") } });
        return JSON.parse(stdout).result;
    };
    const report = await run("report");
    assert.deepEqual(report.score_gates.map(row => row.score_gate_pass), [2, 0, 0]);
    assert.equal(report.totals.find(row => row.threshold === .85).adoption_rate, 1);
    assert.equal(report.totals.find(row => row.threshold === .9).adoption_rate, 0);
    assert.deepEqual(await readFile(f.path), before);
    const list = await run("list");
    const accepted = list.find(row => row.accepted);
    assert.equal(accepted.answers.find(row => row.question === "inbox_0").subject_id, "inbox_a");
    await run("label", accepted.id, "incorrect");
    assert.equal((await run("report")).totals.find(row => row.threshold === .85).reviewed_accuracy, 0);
    await routePrompt(f.args);
    assert.equal((await rows(f.path, "SELECT review FROM requests WHERE accepted=1 ORDER BY rowid"))[0].review, "incorrect");
});

test("metrics_initializes_versioned_database", async t => {
    const f = await fixture(t);
    await routePrompt(f.args);
    assert.equal((await rows(f.path, "PRAGMA user_version"))[0].user_version, 1);
    assert.equal((await rows(f.path, "PRAGMA application_id"))[0].application_id, 0x544a4556);
});

for (const mutation of ["PRAGMA user_version=99", "PRAGMA application_id=123", "PRAGMA user_version=0; PRAGMA application_id=0"]) test(`metrics_rejects_incompatible_database_${mutation}`, async t => {
    const f = await fixture(t);
    await routePrompt(f.args);
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(f.path); db.exec(mutation); db.close();
    const before = await readFile(f.path);
    assert.equal((await routePrompt(f.args)).decision.reason, "uncertain_or_conflicting");
    assert.deepEqual(await readFile(f.path), before);
});

test("metrics_answer_insert_failure_rolls_back_whole_event", async t => {
    const f = await fixture(t);
    await routePrompt(f.args);
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(f.path);
    db.exec("CREATE TRIGGER reject_answer BEFORE INSERT ON answers BEGIN SELECT RAISE(ABORT, 'test failure'); END");
    db.close();
    await routePrompt(f.args);
    assert.equal((await rows(f.path, "SELECT count(*) AS count FROM requests"))[0].count, 1);
    assert.equal((await rows(f.path, "SELECT count(*) AS count FROM answers"))[0].count, 2);
});

test("metrics_worker_timeout_preserves_event_loop_and_terminates_writer", async t => {
    const { EventEmitter } = await import("node:events");
    const { writeMetricBounded } = await import("../jev-metrics.mjs");
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const worker = new EventEmitter();
    let terminated = false, unref = false;
    worker.unref = () => { unref = true; };
    worker.terminate = async () => { terminated = true; };
    const pending = writeMetricBounded({}, { createWorker: () => worker });
    t.mock.timers.tick(250);
    assert.equal(await pending, false);
    assert.equal(terminated, true);
    assert.equal(unref, true);
});

test("metrics_worker_failure_is_redacted_and_does_not_reject", async () => {
    const { EventEmitter } = await import("node:events");
    const { writeMetricBounded } = await import("../jev-metrics.mjs");
    const worker = new EventEmitter();
    worker.unref = () => {};
    worker.terminate = async () => {};
    const pending = writeMetricBounded({}, { createWorker: () => worker });
    worker.emit("error", new Error("private filesystem details"));
    assert.equal(await pending, false);
});

async function hookProcess(directory, mode, session) {
    const { execFile } = await import("node:child_process");
    const { promisify } = await import("node:util");
    const { fileURLToPath } = await import("node:url");
    const { stdout } = await promisify(execFile)(process.execPath, [fileURLToPath(new URL("fixtures/metrics-process.mjs", import.meta.url)), directory, mode, session]);
    return JSON.parse(stdout);
}

test("concurrent_hook_processes_keep_complete_metrics_transactions", async t => {
    const f = await fixture(t);
    const directory = f.args.options.cwd;
    const outputs = await Promise.all(Array.from({ length: 4 }, (_, i) => hookProcess(directory, "true", `parallel_${i}`)));
    assert.ok(outputs.every(output => output.routed));
    const requests = await rows(f.path, "SELECT * FROM requests");
    // Best effort may drop a contended event, but may never commit partial answers.
    assert.ok(requests.length > 0 && requests.length <= 4);
    assert.equal(new Set(requests.map(row => row.session_id)).size, requests.length);
    assert.equal((await rows(f.path, "SELECT * FROM answers")).length, requests.length);
    assert.deepEqual(await rows(f.path, "PRAGMA integrity_check"), [{ integrity_check: "ok" }]);
    assert.deepEqual(await rows(f.path, "PRAGMA foreign_key_check"), []);
});

test("hook_latency_measurement_includes_optional_writer_and_lock_contention", async t => {
    const f = await fixture(t);
    const off = await hookProcess(f.args.options.cwd, "false", "off");
    await assert.rejects(access(f.path));
    const on = await hookProcess(f.args.options.cwd, "true", "on");
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(f.path);
    db.exec("BEGIN IMMEDIATE");
    let locked;
    try { locked = await hookProcess(f.args.options.cwd, "true", "locked"); }
    finally { db.exec("ROLLBACK"); db.close(); }
    assert.ok(off.routed && on.routed && locked.routed);
    assert.equal((await rows(f.path, "SELECT * FROM requests")).length, 1);
    assert.ok([off, on, locked].every(result => Number.isFinite(result.elapsed_ms) && result.elapsed_ms >= 0));
    t.diagnostic(`Whole hook milliseconds (not a CI performance threshold): ${JSON.stringify({ off: off.elapsed_ms, on: on.elapsed_ms, locked: locked.elapsed_ms })}`);
});
