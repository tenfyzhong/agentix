import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import { cp, mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { runTaskcli } from "../runtime.mjs";

// Cargo supplies its freshly compiled executable: these tests must never fall
// back to a developer's installed taskcli or normal task database.
assert.ok(process.env.TASKCLI_BIN, "Run through cargo test -p taskcli");

async function fixture(t, format = "markdown") {
    const dir = await mkdtemp(join(tmpdir(), "task-plugin \u{2603} "));
    const previous = process.env.TASKCLI_CONFIG;
    const cleanup = [];
    process.env.TASKCLI_CONFIG = join(dir, "config.toml");
    t.after(async () => {
        for (const callback of cleanup.reverse()) await callback();
        if (previous === undefined) delete process.env.TASKCLI_CONFIG;
        else process.env.TASKCLI_CONFIG = previous;
        await rm(dir, { recursive: true, force: true });
    });
    const root = join(dir, "vault");
    await mkdir(join(root, ".obsidian"), { recursive: true });
    const run = async (args, options = {}) =>
        (await runTaskcli(args, { cwd: dir, ...options })).result;
    await run([
        "init",
        "--format",
        format,
        "--root",
        root,
        "--directory",
        "Tasks \u{2603}",
        "--database",
        join(dir, "tasks.sqlite3"),
    ]);
    const project = await run([
        "project",
        "register",
        "--root",
        dir,
        "--name",
        "Plugin tests",
    ]);
    const job = await run([
        "job",
        "create",
        "--project",
        project.id,
        "--title",
        "Integration",
    ]);
    return { dir, root, project, job, run, cleanup };
}

function taskLanguage(t, language) {
    const previous = process.env.AGENT_TASK_LANG;
    process.env.AGENT_TASK_LANG = language;
    t.after(() => {
        if (previous === undefined) delete process.env.AGENT_TASK_LANG;
        else process.env.AGENT_TASK_LANG = previous;
    });
}

async function extension(t, f, host) {
    const handlers = new Map();
    const messages = [];
    let tool;
    const pkg = JSON.parse(await readFile("package.json", "utf8"));
    assert.equal(pkg[host].extensions.length, 1);
    const { default: install } = await import(
        pathToFileURL(resolve(pkg[host].extensions[0]))
    );
    install({
        sendMessage: (...args) => messages.push(args),
        on: (name, handler) => handlers.set(name, handler),
        registerTool: (value) => {
            tool = value;
        },
    });
    assert.equal(tool.parameters.properties.args.type, "array");
    assert.ok(tool.parameters.required.includes("args"));
    const ctx = {
        cwd: f.dir,
        sessionManager: { getSessionId: () => `session:${host}` },
        isIdle: () => true,
    };
    await handlers.get("session_start")({}, ctx);
    f.cleanup.push(() => handlers.get("session_shutdown")({}, ctx));
    let calls = 0;
    const invoke = async (args, id = `call-${++calls}`) =>
        (await tool.execute(id, { args }, undefined, undefined, ctx)).details
            .result;
    return { invoke, handlers, ctx, messages };
}

for (const [host, format] of [
    ["pi", "markdown"],
    ["omp", "obsidian"],
]) {
    test(`${host} entrypoint uses real CLI, plans, leases and ${format} files`, async (t) => {
        taskLanguage(t, "zh-CN");
        const f = await fixture(t, format);
        const x = await extension(t, f, host);
        const task = await x.invoke([
            "task",
            "add",
            "--job",
            f.job.id,
            "--title",
            "Build $(not-a-shell) Unicode \u{2603}",
        ]);
        const claim = await x.invoke([
            "task",
            "claim",
            task.id,
            "--delegated-by",
            "team:test",
        ]);
        assert.equal(claim.lease.session_ref, `session:${host}`);
        assert.equal(claim.lease.delegated_by, "team:test");
        assert.equal(claim.phase, "PLANNING");
        await x.invoke([
            "plan",
            "create",
            task.id,
            "--body",
            "# Plan\nAcceptance checks.",
        ]);
        const context = await x.handlers.get("before_agent_start")({}, x.ctx);
        assert.ok(context.message.content.includes(task.id));
        const injected = JSON.parse(context.message.content.split("\n").at(-1));
        assert.equal(injected.task_language, "zh-CN");
        assert.equal(injected.documents.language, undefined);
        const revision = await x.invoke([
            "plan",
            "revise",
            task.id,
            "--body",
            "# Revised plan",
        ]);
        assert.equal(revision.version, 2);
        assert.ok((await readFile(revision.absolute_path, "utf8")).endsWith("# Revised plan"));
        await x.invoke(["task", "wait", task.id, "--reason", "Need review"]);
        assert.equal(
            (await f.run(["task", "show", task.id])).status,
            "WAITING_USER",
        );
        await x.invoke(["task", "claim", task.id]);
        await assert.rejects(
            x.invoke(["task", "done", task.id]),
            /EXECUTING/,
        );
        await x.invoke(["task", "start", task.id]);
        await x.invoke(["task", "done", task.id]);
        const job = await f.run(["job", "show", f.job.id]);
        assert.equal(job.status, "COMPLETED");
        const body = await readFile(
            join(f.root, "Tasks \u{2603}", job.document_path),
            "utf8",
        );
        assert.ok(body.includes(task.name));
        assert.ok(body.includes("Tasks/"));
        const note = await f.run(["plan", "show", task.id]);
        assert.equal(note.properties.status, "DONE");
        assert.equal(note.properties.id, task.id);
        assert.ok(note.path.includes("/Tasks/"));
        assert.equal(body.includes("[["), format === "obsidian");
        assert.equal((await f.run(["doctor"])).healthy, true);
    });
}

test("tool retries preserve identity after claim, Plan revision and lease-releasing writes", async (t) => {
    const f = await fixture(t);
    const x = await extension(t, f, "pi");
    const task = await x.invoke([
        "task",
        "add",
        "--job",
        f.job.id,
        "--title",
        "Idempotent",
    ]);
    const claimArgs = ["task", "claim", task.id];
    const claim = await x.invoke(claimArgs, "claim-once");
    assert.deepEqual(await x.invoke(claimArgs, "claim-once"), claim);
    await x.invoke(["plan", "create", task.id, "--body", "# Plan"]);
    const planArgs = ["plan", "revise", task.id, "--body", "# Retry safe"];
    const plan = await x.invoke(planArgs, "revise-once");
    assert.deepEqual(await x.invoke(planArgs, "revise-once"), plan);
    const startArgs = ["task", "start", task.id];
    const start = await x.invoke(startArgs, "start-once");
    assert.deepEqual(await x.invoke(startArgs, "start-once"), start);
    assert.equal(start.lease.token, claim.lease.token);
    const doneArgs = ["task", "done", task.id];
    const done = await x.invoke(doneArgs, "done-once");
    const before = await f.run(["event", "list", "--job", f.job.id]);
    assert.deepEqual(await x.invoke(doneArgs, "done-once"), done);
    assert.deepEqual(await f.run(["event", "list", "--job", f.job.id]), before);
    await assert.rejects(
        x.invoke(["task", "cancel", task.id], "done-once"),
        /idempotency|different/,
    );
});

for (const host of ["pi", "omp"]) {
    test(`${host} real Inbox Jobs continue in order and retain cancellation facts`, async (t) => {
        const f = await fixture(t);
        const x = await extension(t, f, host);
        async function finish(job) {
            const task = await x.invoke(["task", "add", "--job", job, "--title", "Deliver"]);
            await x.invoke(["task", "claim", task.id]);
            await x.invoke(["plan", "create", task.id, "--body", "# Deliver and verify"]);
            await x.invoke(["task", "start", task.id]);
            await x.invoke(["task", "done", task.id]);
        }
        async function settle() {
            await x.handlers.get("agent_end")({ messages: [{ role: "assistant", stopReason: "stop" }] }, x.ctx);
            if (host === "pi") await x.handlers.get("agent_settled")({}, x.ctx);
        }
        await finish(f.job.id);
        const first = await f.run(["inbox", "add", "--project", f.project.id, "--content", "First request"]);
        const second = await f.run(["inbox", "add", "--project", f.project.id, "--content", "Second request"]);
        await settle();
        assert.equal(x.messages.length, 1);
        let current = await f.run(["context"], { session: x.ctx.sessionManager.getSessionId() });
        assert.equal(current.inbox.id, first.id);
        await finish(current.job_id);
        await settle();
        assert.equal(x.messages.length, 2);
        current = await f.run(["context"], { session: x.ctx.sessionManager.getSessionId() });
        assert.equal(current.inbox.id, second.id);
        await f.run(["inbox", "cancel", second.id]);
        const heartbeat = await f.run(["hook", "heartbeat"], { session: x.ctx.sessionManager.getSessionId() });
        assert.ok(heartbeat.inbox_cancellations.some(entry => entry.id === second.id));
        await settle();
        assert.equal(x.messages.length, 2);
    });
    for (const entity of ["job", "project"]) {
        test(`${host} ${entity} deletion replays its committed result without duplicate events`, async (t) => {
            const f = await fixture(t);
            const x = await extension(t, f, host);
            const task = await x.invoke(["task", "add", "--job", f.job.id, "--title", "Delete safely"]);
            await x.invoke(["task", "claim", task.id]);
            const args = [entity, "delete", f[entity].id];
            await assert.rejects(x.invoke(args, "delete-once"), /release active Task leases/);
            await x.handlers.get("session_shutdown")({}, x.ctx);
            const deleted = await x.invoke(args, "delete-once");
            assert.equal(deleted.deleted, true);
            const events = await f.run(["event", "list"]);
            assert.deepEqual(await x.invoke(args, "delete-once"), deleted);
            assert.deepEqual(await f.run(["event", "list"]), events);
            await assert.rejects(x.invoke([entity, "delete", "missing"], "delete-once"), /idempotency|different/);
            await assert.rejects(f.run(["task", "show", task.id]), /not_found/);
            assert.equal((await f.run(["doctor"])).healthy, true);
        });
    }
}

async function hookProcess(event, command, host, root, shell) {
    const args =
        process.platform === "win32"
            ? ["cmd.exe", ["/d", "/s", "/c", `"${command}"`]]
            : [shell, ["-c", command]];
    const env = { ...process.env };
    delete env.CLAUDE_PLUGIN_ROOT;
    delete env.PLUGIN_ROOT;
    env[host === "claude" ? "CLAUDE_PLUGIN_ROOT" : "PLUGIN_ROOT"] = root;
    const child = spawn(args[0], args[1], {
        env,
        cwd: event.cwd,
        windowsVerbatimArguments: process.platform === "win32",
        stdio: ["pipe", "pipe", "pipe"],
    });
    const out = [],
        err = [];
    child.stdout.on("data", (chunk) => out.push(chunk));
    child.stderr.on("data", (chunk) => err.push(chunk));
    const exited = new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("close", (code) => resolve(code));
    });
    child.stdin.end(JSON.stringify(event));
    assert.equal(await exited, 0, Buffer.concat(err).toString());
    return JSON.parse(Buffer.concat(out).toString());
}

for (const host of ["codex", "claude"]) {
    for (const shell of new Set(
        ["/bin/sh", process.env.TASKCLI_TEST_HOOK_SHELL].filter(Boolean),
    )) {
        test(`${host} bundled hooks run through ${process.platform === "win32" ? "cmd.exe" : shell} and restore fenced leases`, async (t) => {
            taskLanguage(t, "ja");
            const f = await fixture(t);
            const root = join(f.dir, "installed plugin \u{2603}");
            await mkdir(root);
            for (const path of ["hooks", "runtime.mjs", `.${host}-plugin`]) {
                await cp(resolve(path), join(root, path), { recursive: true });
            }
            const task = await f.run([
                "task",
                "add",
                "--job",
                f.job.id,
                "--title",
                "Hooks",
            ]);
            const options = {
                executor: "agent:hooks",
                session: "host-session",
            };
            const claim = await f.run(["task", "claim", task.id], options);
            // Hooks must renew and recover even an unfinished planning phase.
            assert.equal(claim.phase, "PLANNING");
            const manifest = JSON.parse(
                await readFile(join(root, `.${host}-plugin/plugin.json`), "utf8"),
            );
            const hooks = {};
            // Claude merges its extra manifest hooks with default discovery;
            // Codex's manifest replaces default discovery.
            const hookPaths = host === "claude"
                ? ["./hooks/hooks.json", manifest.hooks]
                : manifest.hooks;
            for (const path of hookPaths) {
                const config = JSON.parse(await readFile(join(root, path), "utf8"));
                for (const [name, groups] of Object.entries(config.hooks)) {
                    assert.equal(hooks[name], undefined, `Duplicate ${name}`);
                    hooks[name] = groups;
                }
            }
            const interruptEvent = host === "codex" ? "Interrupt" : "PostToolUseFailure";
            if (host === "claude") {
                const events = await f.run(["event", "list"]);
                for (const is_interrupt of [undefined, false, "true"]) {
                    await hookProcess(
                        { hook_event_name: interruptEvent, is_interrupt, session_id: options.session, cwd: f.dir },
                        hooks[interruptEvent][0].hooks[0].command,
                        host, root, shell,
                    );
                    assert.deepEqual(await f.run(["task", "show", task.id]), claim);
                    assert.deepEqual(await f.run(["event", "list"]), events);
                }
            }
            let expectedStatus = "IN_PROGRESS";
            for (const name of [
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "Stop",
                interruptEvent,
                interruptEvent,
                "PostToolUse",
                "SessionStart",
                "SessionEnd",
                "SessionStart",
            ]) {
                const command = hooks[name][0].hooks[0].command;
                const output = await hookProcess(
                    {
                        hook_event_name: name,
                        is_interrupt: name === "PostToolUseFailure",
                        session_id: options.session,
                        cwd: f.dir,
                    },
                    command,
                    host,
                    root,
                    shell,
                );
                const current = await f.run(["task", "show", task.id]);
                if (name === "SessionEnd" || name === interruptEvent) expectedStatus = "BLOCKED";
                if (name === "SessionStart") expectedStatus = "IN_PROGRESS";
                assert.equal(current.status, expectedStatus);
                if (expectedStatus === "BLOCKED") assert.equal(current.lease, null);
                if (name === interruptEvent) assert.equal(current.reason, "session interrupted");
                if (name === "SessionStart")
                    assert.equal(
                        JSON.parse(output.hookSpecificOutput.additionalContext.split("\n").at(-1)).task_language,
                        "ja",
                    );
                if (name === "SessionStart")
                    assert.ok(
                        output.hookSpecificOutput.additionalContext.includes(
                            task.id,
                        ),
                    );
            }
            const resumed = await f.run(["task", "show", task.id]);
            assert.equal(resumed.phase, "PLANNING");
            assert.notEqual(resumed.lease.token, claim.lease.token);
            await assert.rejects(
                f.run(["plan", "create", task.id, "--body", "# Stale Plan"], {
                    ...options,
                    token: claim.lease.token,
                }),
                /conflict/,
            );
            const resumedOptions = {
                ...options,
                token: resumed.lease.token,
            };
            await f.run(
                ["plan", "create", task.id, "--body", "# Owned Plan"],
                resumedOptions,
            );
            await f.run(["task", "start", task.id], resumedOptions);
            await hookProcess(
                { hook_event_name: interruptEvent, is_interrupt: true, session_id: options.session, turn_id: "executing-turn", cwd: f.dir },
                hooks[interruptEvent][0].hooks[0].command,
                host,
                root,
                shell,
            );
            const interrupted = await f.run(["task", "show", task.id]);
            assert.equal(interrupted.status, "BLOCKED");
            assert.equal(interrupted.lease, null);
            await assert.rejects(f.run(["task", "done", task.id], resumedOptions), /conflict/);
            await f.run(["job", "delete", f.job.id]);
        });
    }
}

test("real CLI errors, aborts and identity overrides are not reported as success", async (t) => {
    const f = await fixture(t);
    await assert.rejects(f.run(["task", "show", "task_missing"]), /not_found/);
    await assert.rejects(
        f.run(["task", "claim", "task_missing", "--session=someone-else"]),
        /managed by the host/,
    );
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(
        f.run(["context"], { signal: controller.signal }),
        /abort/i,
    );
});

for (const host of ["pi", "omp"]) {
    test(`${host} normal continuations and stale session callbacks preserve the current lease`, async (t) => {
        const f = await fixture(t);
        const x = await extension(t, f, host);
        const task = await x.invoke(["task", "add", "--job", f.job.id, "--title", "Continue work"]);
        const claim = await x.invoke(["task", "claim", task.id]);
        await x.invoke(["plan", "create", task.id, "--body", "# Continue"]);
        await x.invoke(["task", "start", task.id]);
        await x.handlers.get("session_start")({}, x.ctx);
        for (const event of [
            { messages: [{ role: "assistant", stopReason: "stop" }] },
            { messages: [{ role: "assistant", stopReason: "aborted" }], willContinue: true },
            { messages: [{ role: "assistant", stopReason: "aborted" }, { role: "assistant", stopReason: "stop" }] },
        ]) {
            await x.handlers.get("agent_end")(event, x.ctx);
            if (host === "pi") await x.handlers.get("agent_settled")({}, x.ctx);
            const current = await f.run(["task", "show", task.id]);
            assert.equal(current.phase, "EXECUTING");
            assert.equal(current.lease.token, claim.lease.token);
        }
        const nextCtx = { ...x.ctx, sessionManager: { getSessionId: () => `next:${host}` } };
        await x.handlers.get("session_start")({}, nextCtx);
        f.cleanup.push(() => x.handlers.get("session_shutdown")({}, nextCtx));
        assert.equal((await f.run(["task", "show", task.id])).reason, "session ended");
        const other = await f.run(["task", "add", "--job", f.job.id, "--title", "New session"]);
        const next = await f.run(["task", "claim", other.id], { executor: `agent:${host}:next:${host}`, session: `next:${host}` });
        await x.handlers.get("agent_end")({ messages: [{ role: "assistant", stopReason: "aborted" }] }, x.ctx);
        if (host === "pi") await x.handlers.get("agent_settled")({}, x.ctx);
        await x.handlers.get("session_shutdown")({}, x.ctx);
        assert.deepEqual(await f.run(["task", "show", other.id]), next);
        await x.handlers.get("session_shutdown")({}, nextCtx);
        assert.equal((await f.run(["task", "show", other.id])).lease, null);
    });

    for (const executing of [false, true]) {
        test(`${host} releases ${executing ? "executing" : "planning"} work on interruption and shutdown`, async (t) => {
            const f = await fixture(t);
            const x = await extension(t, f, host);
            const task = await x.invoke(["task", "add", "--job", f.job.id, "--title", "Interrupted"]);
            const claim = await x.invoke(["task", "claim", task.id]);
            await x.invoke(["plan", "create", task.id, "--body", "# Keep this plan"]);
            if (executing) await x.invoke(["task", "start", task.id]);
            await x.handlers.get("agent_end")({ messages: [{ role: "assistant", stopReason: "aborted" }] }, x.ctx);
            if (host === "pi") await x.handlers.get("agent_settled")({}, x.ctx);
            const blocked = await f.run(["task", "show", task.id]);
            assert.equal(blocked.reason, "session interrupted");
            assert.equal(blocked.lease, null);
            assert.equal(blocked.current_plan, (await f.run(["plan", "show", task.id])).id);
            await assert.rejects(f.run(["task", "heartbeat", task.id], {
                session: claim.lease.session_ref, token: claim.lease.token,
            }), /conflict/);
            await x.handlers.get("before_agent_start")({}, x.ctx);
            assert.equal((await f.run(["task", "show", task.id])).lease, null);
            const next = await x.invoke(["task", "claim", task.id]);
            assert.notEqual(next.lease.token, claim.lease.token);
            if (executing) await x.invoke(["task", "start", task.id]);
            await x.handlers.get("session_shutdown")({}, x.ctx);
            const ended = await f.run(["task", "show", task.id]);
            assert.equal(ended.reason, "session ended");
            assert.equal(ended.lease, null);
            await f.run(["job", "delete", f.job.id]);
        });
    }
}

for (const host of ["pi", "omp"]) {
    test(`${host} resolves real task prefixes for lease-authorized operations`, async (t) => {
        const f = await fixture(t);
        const x = await extension(t, f, host);
        const task = await x.invoke(["task", "add", "--job", f.job.id, "--title", "Prefix ownership"]);
        await x.invoke(["task", "claim", task.id]);
        const prefix = task.id.slice(0, -1);
        await x.invoke(["plan", "create", prefix, "--body", "# Prefix plan"]);
        await x.invoke(["plan", "revise", prefix, "--body", "# Revised prefix plan"]);
        await x.invoke(["task", "start", prefix]);
        const done = await x.invoke(["task", "done", prefix], "prefix-done");
        assert.equal(done.status, "DONE");
        assert.deepEqual(await x.invoke(["task", "done", prefix], "prefix-done"), done);
        assert.equal((await f.run(["plan", "show", task.id])).body, "# Revised prefix plan");
    });
}
