// Explicit opt-in smoke: a host Agent supplies the native classifier result.
import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdir, mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { watch } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { runTaskix, runHook } from "../runtime.mjs";
import { validateSnapshotDecision } from "../routing-decision.mjs";

test("native classifier result completes isolated followup with guarded writes", { skip: !process.env.TASKIX_NATIVE_ROUTING_DIR }, async t => {
    const exchange = resolve(process.env.TASKIX_NATIVE_ROUTING_DIR);
    await mkdir(exchange, { recursive: true, mode: 0o700 });
    const dir = await mkdtemp(join(tmpdir(), "taskix-native-routing-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const previous = process.env.TASKIX_CONFIG;
    process.env.TASKIX_CONFIG = join(dir, "config.toml");
    t.after(() => { if (previous === undefined) delete process.env.TASKIX_CONFIG; else process.env.TASKIX_CONFIG = previous; });
    const options = { cwd: dir, session: "native-fixture-parent", executor: "agent:codex" };
    const run = async args => (await runTaskix(args, options)).result;
    await mkdir(join(dir, "vault", ".obsidian"), { recursive: true });
    await run(["init", "--root", join(dir, "vault"), "--database", join(dir, "tasks.sqlite")]);
    const project = await run(["project", "register", "--root", dir, "--name", "Native routing fixture"]);
    const job = await run(["job", "create", "--project", project.id, "--title", "Fix login session expiry", "--prompt", "Fix login session expiry and prepare a pull request", "--review-policy", "required"]);
    const task = await run(["task", "add", "--job", job.id, "--title", "Implement login session expiry fix"]);
    const claim = await run(["task", "claim", task.id]);
    const owner = { ...options, token: claim.lease.token };
    for (const args of [["plan", "create", task.id, "--body", "Implement and verify login fix"], ["task", "start", task.id], ["task", "done", task.id]]) await runTaskix(args, owner);
    await run(["job", "create", "--project", project.id, "--title", "Add full text search", "--prompt", "Implement search filters"]);
    const prompt = "Create the pull request for the completed login session expiry fix.";
    const output = await runHook({ hook_event_name: "UserPromptSubmit", session_id: options.session, cwd: dir, prompt }, runTaskix, {
        env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://unused.test", TASKIX_JEV_API_KEY: "fixture" },
        fetch: async () => { throw new Error("Synthetic service outage"); },
    });
    const snapshot = JSON.parse(output.hookSpecificOutput.additionalContext.split("Snapshot: ")[1]);
    t.after(() => rm(snapshot, { force: true }));
    const resultPath = join(exchange, "result.json");
    await rm(resultPath, { force: true });
    const manifest = { snapshot, protocol: resolve("skills/taskix-manager/references/routing-classifier.md"), result_path: resultPath };
    await writeFile(join(exchange, "request.json"), JSON.stringify(manifest), { mode: 0o600 });
    const before = await run(["event", "list", "--job", job.id]);
    const result = await new Promise((resolveResult, reject) => {
        let busy = false;
        const timer = setTimeout(() => { watcher.close(); reject(new Error("Native classifier result not supplied within 180 seconds")); }, 180000);
        const check = async () => {
            if (busy) return;
            busy = true;
            try {
                const data = await readFile(resultPath, "utf8");
                if (data.length > 1000) throw new Error("Oversized classifier result");
                const value = JSON.parse(data); clearTimeout(timer); watcher.close(); resolveResult(value);
            } catch (error) { if (error.code !== "ENOENT") { clearTimeout(timer); watcher.close(); reject(error); } }
            finally { busy = false; }
        };
        const watcher = watch(exchange, () => void check());
        void check();
    });
    assert.deepEqual(await run(["event", "list", "--job", job.id]), before, "classifier must not mutate task state");
    const checked = await validateSnapshotDecision(JSON.parse(await readFile(snapshot, "utf8")), result, options, runTaskix);
    assert.equal(checked.decision.action, "followup");
    assert.equal(checked.decision.job_id, job.id);
    await run([...checked.followup_args, "--prompt", prompt]);
    const next = await run(["task", "add", "--job", job.id, "--title", "Prepare the pull request"]);
    assert.deepEqual(next.dependencies, [task.id]);
    assert.equal((await run(["job", "show", job.id])).review_policy, "required");
});
