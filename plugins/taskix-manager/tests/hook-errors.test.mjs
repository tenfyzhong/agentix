import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, delimiter } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const hook = fileURLToPath(new URL("../hooks/run.mjs", import.meta.url));

async function fixture(t) {
    const dir = await mkdtemp(join(tmpdir(), "taskix-hook-errors-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const bin = join(dir, "bin");
    await mkdir(bin);
    return { dir, bin, log: join(dir, "state", "taskix", "hooks.jsonl") };
}

function run(f, input, extra = {}) {
    return spawnSync(process.execPath, [hook], {
        input: typeof input === "string" ? input : JSON.stringify(input),
        encoding: "utf8",
        env: { ...process.env, PATH: f.bin + delimiter + process.env.PATH,
            XDG_STATE_HOME: join(f.dir, "state"), TASKIX_JEV_ENABLED: "false", ...extra },
    });
}

test("hook_failure_persists_reason_and_context_without_event_payload", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    await writeFile(join(f.bin, "taskix"), '#!/bin/sh\nprintf \'%s\\n\' \'{"ok":false,"error":{"code":"invalid_or_failed","message":"Operation not permitted (os error 1)"}}\'\nexit 1\n', { mode: 0o700 });
    const result = run(f, { hook_event_name: "PostToolUse", session_id: "test-session", cwd: f.dir, prompt: "private-prompt", tool_input: { secret: "private-input" } });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /Operation not permitted/);
    assert.match(result.stderr, /taskix hook heartbeat/);
    assert.ok(result.stderr.includes(f.log));
    const raw = await readFile(f.log, "utf8");
    const entry = JSON.parse(raw);
    assert.equal(entry.event, "PostToolUse");
    assert.equal(entry.session_id, "test-session");
    assert.equal(entry.cwd, f.dir);
    assert.equal(entry.command, "taskix hook heartbeat");
    assert.equal(entry.exit_code, 1);
    assert.match(entry.message, /Operation not permitted/);
    assert.ok(Number.isFinite(Date.parse(entry.timestamp)));
    assert.doesNotMatch(raw, /private-prompt|private-input/);
    if (process.platform !== "win32") {
        const { stat } = await import("node:fs/promises");
        assert.equal((await stat(f.log)).mode & 0o777, 0o600);
    }
});

test("hook_invalid_input_is_logged_and_log_failure_preserves_original_error", async t => {
    const f = await fixture(t);
    const result = run(f, "{");
    assert.equal(result.status, 1);
    assert.equal(JSON.parse(await readFile(f.log, "utf8")).event, "unknown");
    const blocked = join(f.dir, "blocked");
    await writeFile(blocked, "file");
    const failed = run(f, { hook_event_name: "Stop" }, { XDG_STATE_HOME: blocked });
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /Hook requires session_id/);
    assert.match(failed.stderr, /Could not write hook error log/);
    assert.equal(failed.stdout, "");
});

test("successful_hook_does_not_create_error_log", async t => {
    const f = await fixture(t);
    const result = run(f, { hook_event_name: "PostToolUseFailure", session_id: "test-session", is_interrupt: false });
    assert.equal(result.status, 0);
    assert.equal(result.stderr, "");
    assert.deepEqual(JSON.parse(result.stdout), {});
    await assert.rejects(readFile(f.log), { code: "ENOENT" });
});

test("missing_taskix_reports_spawn_reason_without_full_arguments", async t => {
    const f = await fixture(t);
    const result = run(f, { hook_event_name: "PostToolUse", session_id: "test-session", cwd: f.dir }, { PATH: f.bin });
    assert.equal(result.status, 1);
    const entry = JSON.parse(await readFile(f.log, "utf8"));
    assert.equal(entry.code, "ENOENT");
    assert.match(entry.message, /taskix hook heartbeat.*ENOENT/);
    assert.doesNotMatch(entry.message, /--session|--json/);
});

test("non_json_cli_failure_preserves_stderr_and_exit_status", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    await writeFile(join(f.bin, "taskix"), '#!/bin/sh\nprintf \'permission denied opening config\\n\' >&2\nexit 7\n', { mode: 0o700 });
    const result = run(f, { hook_event_name: "PostToolUse", session_id: "test-session", cwd: f.dir });
    assert.equal(result.status, 1);
    const entry = JSON.parse(await readFile(f.log, "utf8"));
    assert.equal(entry.exit_code, 7);
    assert.match(entry.message, /permission denied opening config/);
    assert.match(result.stderr, /permission denied opening config/);
});

test("silent_cli_failure_reports_exit_code_without_argument_dump", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    await writeFile(join(f.bin, "taskix"), "#!/bin/sh\nexit 9\n", { mode: 0o700 });
    const result = run(f, { hook_event_name: "PostToolUse", session_id: "test-session", cwd: f.dir });
    const entry = JSON.parse(await readFile(f.log, "utf8"));
    assert.equal(entry.exit_code, 9);
    assert.match(entry.message, /exit code 9/);
    assert.doesNotMatch(entry.message, /--session|--json/);
});
