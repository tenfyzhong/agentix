import { choiceAnswers } from "./support/jev.mjs";
import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import { cp, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { runHook, runTaskix } from "../runtime.mjs";
import { loadPlugin, copy } from "./support/obsidian-plugin.mjs";

// Run through cargo test -p taskix, which prepends the compiled binary directory
// to PATH. Each fixture uses an isolated configuration and database.

async function fixture(t, session) {
    const dir = await mkdtemp(join(tmpdir(), "task-plugin \u{2603} "));
    const previous = process.env.TASKIX_CONFIG;
    const cleanup = [];
    process.env.TASKIX_CONFIG = join(dir, "config.toml");
    t.after(async () => {
        for (const callback of cleanup.reverse()) await callback();
        if (previous === undefined) delete process.env.TASKIX_CONFIG;
        else process.env.TASKIX_CONFIG = previous;
        await rm(dir, { recursive: true, force: true });
    });
    const root = join(dir, "vault");
    await mkdir(join(root, ".obsidian"), { recursive: true });
    const run = async (args, options = {}) =>
        (await runTaskix(args, { cwd: dir, ...options })).result;
    await run([
        "init",
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
    ], { session });
    return { dir, root, project, job, run, cleanup };
}

test("Obsidian bridge uses real CLI revisions, lease guards and manual Job review", async (t) => {
    const f = await fixture(t);
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Bridge"]);
    const { SyncEngine, runCli } = loadPlugin();
    const execute = (args) => runCli({ cliPath: "taskix", configPath: process.env.TASKIX_CONFIG, vaultPath: f.root }, args);
    const lookup = async (id) => (await execute(["obsidian", "show", id])).result;
    const files = new Map();
    const notices = [];
    for (const row of [await lookup(task.id), await lookup(f.job.id)]) {
        files.set(row.path, { ...copy(row.properties), id: row.id, task_id: row.kind === "task" ? row.id : undefined, revision: row.revision, custom: "preserved" });
    }
    const engine = new SyncEngine({
        connection: async () => (await execute(["obsidian", "connection"])).result,
        lookup, execute, notice: (message) => notices.push(message),
        openNotes: () => [...files].map(([path, properties]) => ({ path, properties })),
        read: async (path) => copy(files.get(path)),
        patch: async (path, expected, properties) => {
            const file = files.get(path);
            if (file.id !== expected.id || file.status !== expected.status || file.revision !== expected.revision) return false;
            Object.assign(file, copy(properties));
            return true;
        },
    });
    t.after(() => engine.dispose());
    await engine.initialize();
    const edit = async (id, status) => {
        const row = await lookup(id);
        Object.assign(files.get(row.path), copy(row.properties), { revision: row.revision, status });
        engine.observe(row.path, files.get(row.path));
        await engine.flush();
        return files.get(row.path);
    };
    await edit(task.id, "BLOCKED");
    assert.equal((await f.run(["task", "show", task.id])).status, "BLOCKED");
    const claim = (await execute(["task", "claim", task.id, "--executor", "agent:test", "--session", "bridge-owner"])).result;
    await engine.initialize();
    const rejected = await edit(task.id, "CANCELLED");
    assert.equal(rejected.status, "IN_PROGRESS");
    assert.equal(rejected.custom, "preserved");
    assert.equal(notices.length, 1);
    const owner = ["--session", "bridge-owner", "--lease-token", claim.lease.token];
    await execute(["plan", "create", task.id, "--body", "Verify bridge", ...owner]);
    await execute(["task", "start", task.id, ...owner]);
    await execute(["task", "done", task.id, ...owner]);
    await engine.initialize();
    assert.equal((await f.run(["job", "show", f.job.id])).status, "PENDING_REVIEW");
    await edit(f.job.id, "ACTIVE");
    assert.equal((await f.run(["job", "show", f.job.id])).status, "ACTIVE");
    assert.equal((await f.run(["task", "show", task.id])).status, "DONE");
    await edit(f.job.id, "PENDING_REVIEW");
    const approved = await edit(f.job.id, "COMPLETED");
    assert.equal(approved.status, "COMPLETED");
    assert.ok(approved.completed_at);
    assert.equal(Object.hasOwn(approved, "completedDate"), false);
    const job = await f.run(["job", "show", f.job.id]);
    assert.ok(job.completed_at);
    assert.match(await readFile(join(f.root, "Tasks \u{2603}", job.document_path), "utf8"), /status: "COMPLETED"/);
    assert.equal(notices.length, 1);
});

test("Inbox bridge edits real Markdown, enforces Job review and reopens completed work", async (t) => {
    const f = await fixture(t);
    await f.run(["job", "cancel", f.job.id]);
    const entry = await f.run(["inbox", "add", "--project", f.project.id, "--content", "Deliver\nPreserve these details."]);
    const claimed = await f.run(["inbox", "claim-next", "--project", f.project.id], { session: "inbox-worker", executor: "agent:test" });
    const task = await f.run(["task", "add", "--job", claimed.job.id, "--title", "Implementation"]);
    const { SyncEngine, runCli, parseInbox, patchInbox } = loadPlugin();
    const execute = (args) => runCli({ cliPath: "taskix", configPath: process.env.TASKIX_CONFIG, vaultPath: f.root }, args);
    const lookup = async (id) => (await execute(["obsidian", "show", id])).result;
    const note = await lookup(entry.id);
    const path = join(f.root, note.path);
    const notices = [];
    const engine = new SyncEngine({
        connection: async () => (await execute(["obsidian", "connection"])).result,
        lookup, execute, notice: (message) => notices.push(message),
        openNotes: async function* () { yield { path: note.path, source: await readFile(path, "utf8") }; },
        read: async () => parseInbox(await readFile(path, "utf8"), f.project.id).find((row) => row.id === entry.id),
        patch: async (_path, expected, properties, row) => {
            await writeFile(path, patchInbox(await readFile(path, "utf8"), row, expected, properties));
        },
    });
    t.after(() => engine.dispose());
    await engine.initialize();
    const edit = async (from, to) => {
        const source = (await readFile(path, "utf8")).replace(`- [${from}] Deliver`, `- [${to}] Deliver`);
        await writeFile(path, source); engine.observeInbox(note.path, source); await engine.flush();
    };
    await edit("/", "x");
    assert.equal((await f.run(["job", "show", claimed.job.id])).status, "ACTIVE");
    assert.match(notices[0], /all Tasks must be DONE, FAILED or CANCELLED/);
    assert.match(await readFile(path, "utf8"), /- \[\/\] Deliver/);
    const owner = { session: "inbox-worker", executor: "agent:test" };
    const claim = await f.run(["task", "claim", task.id], owner); owner.token = claim.lease.token;
    await f.run(["plan", "create", task.id, "--body", "Verify"], owner);
    await f.run(["task", "start", task.id], owner); await f.run(["task", "done", task.id], owner);
    await engine.initialize();
    assert.match(await readFile(path, "utf8"), /- \[r\] Deliver/);
    await edit("r", "/");
    assert.equal((await f.run(["job", "show", claimed.job.id])).status, "ACTIVE");
    await edit("/", "r");
    assert.equal((await f.run(["job", "show", claimed.job.id])).status, "PENDING_REVIEW");
    await edit("r", "x");
    assert.equal((await f.run(["job", "show", claimed.job.id])).status, "COMPLETED");
    await edit("x", "/");
    assert.equal((await f.run(["job", "show", claimed.job.id])).status, "ACTIVE");
    await edit("/", " ");
    await edit(" ", "-");
    assert.equal((await f.run(["job", "show", claimed.job.id])).status, "CANCELLED");
    await edit("-", " ");
    assert.equal((await f.run(["inbox", "list", "--project", f.project.id]))[0].status, "TODO");
    assert.equal((await f.run(["task", "show", task.id])).status, "DONE");
    assert.match(await readFile(path, "utf8"), /Preserve these details/);
    assert.equal(notices.length, 1);
});

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

for (const host of ["pi", "omp"]) {
    test(`${host} entrypoint uses real CLI, plans, leases and Obsidian files`, async (t) => {
        taskLanguage(t, "zh-CN");
        const f = await fixture(t);
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
        assert.equal(job.status, "PENDING_REVIEW");
        await f.run(["job", "approve", job.id]);
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
        assert.equal(body.includes("[["), true);
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

for (const host of ["codex", "claude"]) {
    test(`${host} real Stop hooks leave pending entries unclaimed until manual intake`, async (t) => {
        const f = await fixture(t);
        const options = { session: `session:${host}`, executor: `agent:${host}`, cwd: f.dir };
        const previous = await f.run(["task", "add", "--job", f.job.id, "--title", "Previous work"], options);
        const owner = await f.run(["task", "claim", previous.id], options);
        const leased = { ...options, token: owner.lease.token };
        await f.run(["plan", "create", previous.id, "--body", "# Deliver"], leased);
        await f.run(["task", "start", previous.id], leased);
        await f.run(["task", "done", previous.id], leased);
        await f.run(["job", "approve", f.job.id]);
        const entry = await f.run(["inbox", "add", "--project", f.project.id, "--content", "Await user review"]);
        const queue = await f.run(["inbox", "list", "--project", f.project.id]);
        for (let i = 0; i < 2; i++) {
            assert.deepEqual(await runHook({
                hook_event_name: "Stop", session_id: options.session, cwd: f.dir,
                ...(host === "codex" ? { turn_id: `turn-${i}` } : {}),
            }), {});
        }
        assert.deepEqual(await f.run(["inbox", "list", "--project", f.project.id]), queue);
        assert.equal((await f.run(["context"], options)).inbox, null);
        const next = await f.run(["inbox", "claim-next", "--project", f.project.id], options);
        assert.equal(next.entry.id, entry.id);
        await f.run(["inbox", "cancel", entry.id]);
    });
}

for (const host of ["pi", "omp"]) {
    test(`${host} real Inbox Jobs require explicit intake after each completion`, async (t) => {
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
        await f.run(["job", "approve", f.job.id]);
        const first = await f.run(["inbox", "add", "--project", f.project.id, "--content", "First request"]);
        const second = await f.run(["inbox", "add", "--project", f.project.id, "--content", "Second request"]);
        for (const entry of [first, second]) {
            await settle();
            await settle();
            let current = await f.run(["context"], { session: x.ctx.sessionManager.getSessionId() });
            assert.equal(current.inbox, null);
            assert.equal(x.messages.length, 0);
            const queue = await f.run(["inbox", "list", "--project", f.project.id]);
            assert.equal(queue.find(row => row.id === entry.id).status, "TODO");
            assert.equal(queue.find(row => row.id === entry.id).job_id, null);
            const claimed = await x.invoke(["inbox", "claim-next", "--project", f.project.id]);
            assert.equal(claimed.entry.id, entry.id);
            current = await f.run(["context"], { session: x.ctx.sessionManager.getSessionId() });
            assert.equal(current.inbox.id, entry.id);
            if (entry.id === first.id) {
                await finish(current.job_id);
                assert.equal((await f.run(["job", "show", current.job_id])).status, "PENDING_REVIEW");
                await f.run(["job", "approve", current.job_id]);
            }
            else await f.run(["inbox", "cancel", entry.id]);
        }
        const heartbeat = await f.run(["hook", "heartbeat"], { session: x.ctx.sessionManager.getSessionId() });
        assert.ok(heartbeat.inbox_cancellations.some(entry => entry.id === second.id));
        await settle();
        assert.equal(x.messages.length, 0);
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
            if (entity === "project") {
                // Refreshing a non-Git directory registers a new, empty Project.
                const current = await f.run(["context"]);
                assert.notEqual(current.project_id, f.project.id);
                assert.equal(current.job_id, null);
                assert.deepEqual(await f.run(["job", "list", "--project", current.project_id]), []);
            }
            const events = await f.run(["event", "list"]);
            assert.deepEqual(await x.invoke(args, "delete-once"), deleted);
            assert.deepEqual(await f.run(["event", "list"]), events);
            await assert.rejects(x.invoke([entity, "delete", "missing"], "delete-once"), /idempotency|different/);
            await assert.rejects(f.run(["task", "show", task.id]), /not_found/);
            assert.equal((await f.run(["doctor"])).healthy, true);
        });
    }
}

async function hookProcess(event, command, host, root, shell, extraEnv = {}) {
    const args =
        process.platform === "win32"
            ? ["cmd.exe", ["/d", "/s", "/c", `"${command}"`]]
            : [shell, ["-c", command]];
    const env = { ...process.env, ...extraEnv };
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
        ["/bin/sh", process.env.TASKIX_TEST_HOOK_SHELL].filter(Boolean),
    )) {
        test(`${host} bundled hooks run through ${process.platform === "win32" ? "cmd.exe" : shell} and restore fenced leases`, async (t) => {
            taskLanguage(t, "ja");
            const f = await fixture(t);
            const root = join(f.dir, "installed plugin \u{2603}");
            await mkdir(root);
            for (const path of ["hooks", "runtime.mjs", "taskix-cli.mjs", "routing-context.mjs", "conversation.mjs", "discussion.mjs", "lifecycle.mjs", "jev.mjs", "routing-state.mjs", "jev-metrics.mjs", "jev-metrics-worker.mjs", "metrics-schema.sql", `.${host}-plugin`]) {
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

test("Codex plan discussions attach explicitly through the real CLI", async (t) => {
    const f = await fixture(t);
    const options = {executor:"agent:codex",session:"plan-session"};
    const transcript = join(f.dir, "session.jsonl");
    const rows = [
        {type:"event_msg",payload:{type:"task_started",turn_id:"p"}},
        {type:"turn_context",payload:{collaboration_mode:{mode:"plan"}}},
        {type:"response_item",payload:{type:"message",id:"u",role:"user",content:"Plan the feature"}},
        {type:"response_item",payload:{type:"function_call",name:"request_user_input",call_id:"q",arguments:JSON.stringify({questions:[{id:"scope",question:"Which scope?",options:[{label:"Local",description:"This project"}]}]})}},
        {type:"response_item",payload:{type:"function_call_output",call_id:"q",output:JSON.stringify({answers:{scope:{answers:["Local"]}}})}},
        {type:"response_item",payload:{type:"message",id:"a",role:"assistant",content:"<proposed_plan>\n# Feature\nUse local scope\n</proposed_plan>"}},
        {type:"event_msg",payload:{type:"task_started",turn_id:"i"}},
        {type:"turn_context",payload:{collaboration_mode:{mode:"default"}}},
        {type:"response_item",payload:{type:"message",id:"u",role:"user",content:"Implement the plan."}},
        {type:"response_item",payload:{type:"message",id:"a",role:"assistant",content:"Implemented"}},
    ];
    const event = {hook_event_name:"Stop",session_id:"plan-session",cwd:f.dir,transcript_path:transcript};
    await writeFile(transcript, rows.slice(0,6).map(row => JSON.stringify(row)).join("\n"));
    await runHook(event);
    await writeFile(transcript, rows.map(row => JSON.stringify(row)).join("\n"));
    await runHook({...event,hook_event_name:"PreToolUse"});
    const job = await f.run(["job","create","--project",f.project.id,"--title","Feature","--prompt","Implement the plan.","--conversation-turn","p","--conversation-turn","i"],options);
    await runHook(event);
    const first = await f.run(["job", "show", job.id]);
    await runHook(event);
    assert.deepEqual(await f.run(["job", "show", job.id]), first);
    assert.equal(first.prompt, "Plan the feature");
    assert.deepEqual(first.conversation.map(message => message.text), [
        "Plan the feature", "Which scope?\n\n- Local: This project", "Which scope?\nLocal",
        "<proposed_plan>\n# Feature\nUse local scope\n</proposed_plan>", "Implement the plan.", "Implemented",
    ]);
    const doc = await readFile(join(f.root, "Tasks \u{2603}", first.document_path), "utf8");
    assert.ok(doc.includes("    Plan the feature"));
    assert.ok(doc.includes("> # Feature"));
    assert.ok(doc.includes("    Which scope?\n    Local"));
});

test("Jev routing uses real project-scoped CLI candidates and recovers waiting work", async (t) => {
    const { routePrompt } = await import("../jev.mjs");
    const f = await fixture(t);
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Choose deployment region"]);
    await f.run(["task", "wait", task.id, "--reason", "Which region?"]);
    const options = { cwd: f.dir, session: "routing-session" };
    const context = (await runTaskix(["context", "--project", f.project.id], options)).result;
    assert.equal(context.previous_job, null);
    const routed = await routePrompt({
        prompt: "Asia", context, options, runner: runTaskix,
        env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://unused.test", TASKIX_JEV_API_KEY: "mock" },
        fetch: async (_url, init) => {
            const request = JSON.parse(init.body);
            const candidate = request.state.candidates.find(c => c.job.id === f.job.id);
            assert.equal(candidate.tasks[0].reason, "Which region?");
            const choice = `resume:${f.job.id}`;
            return { ok: true, json: async () => (choiceAnswers(request, { intent: "work", route: choice })) };
        },
    });
    assert.equal(routed.decision.action, "resume");
    assert.equal(routed.context.tasks[0].id, task.id);
    assert.equal((await f.run(["task", "show", task.id])).status, "WAITING_USER", "classification must not acquire a lease");
});

for (const host of ["codex", "claude"]) test(`${host} Jev prompt and tool hooks work across processes with real HTTP`, async (t) => {
    const { createServer } = await import("node:http");
    const f = await fixture(t);
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Waiting task"]);
    await f.run(["task", "wait", task.id, "--reason", "Which region?"]);
    const requests = [];
    const server = createServer(async (req, res) => {
        let text = "";
        for await (const chunk of req) text += chunk;
        const body = JSON.parse(text);
        requests.push({ method: req.method, url: req.url, authorization: req.headers.authorization, body });
        const choice = `resume:${f.job.id}`;
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(choiceAnswers(body, { intent: "work", route: choice })));
    });
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
    const config = JSON.parse(await readFile(resolve("hooks/hooks.json"), "utf8"));
    const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: `http://127.0.0.1:${server.address().port}/v1/systemone`, TASKIX_JEV_API_KEY: "integration-key" };
    const event = { session_id: "jev-process", cwd: f.dir, ...(host === "codex" ? { turn_id: "turn_1" } : {}) };
    const hook = (name, extra = {}) => hookProcess({ ...event, hook_event_name: name, ...extra }, config.hooks[name][0].hooks[0].command, host, resolve("."), "/bin/sh", env);
    const prompt = await hook("UserPromptSubmit", { prompt: "Asia" });
    assert.match(prompt.hookSpecificOutput.additionalContext, /Taskix route: resume/);
    assert.ok(!JSON.stringify(prompt).includes("integration-key"));
    assert.equal(requests.length, 1);
    assert.equal(requests[0].method, "POST");
    assert.equal(requests[0].url, "/v1/systemone");
    assert.equal(requests[0].authorization, "Bearer integration-key");
    assert.equal(requests[0].body.state.prompt, "Asia");
    assert.deepEqual(await hook("PreToolUse"), {});
    assert.deepEqual(await hook("PostToolUse"), {});
    assert.equal(requests.length, 1, "tool hooks must not classify again");
    assert.equal((await f.run(["task", "show", task.id])).status, "WAITING_USER");
    await hook("Stop");
});

// Opt-in measurements use real CLI processes and isolated databases. HTTP is
// deterministic so the reported times describe local overhead, not Jev latency.
for (const [count, history] of [[0, 0], [1, 0], [8, 0], [32, 0], [33, 0], [1, 10000]]) {
    test(`routing benchmark ${count} candidates ${history} messages`, { skip: process.env.TASKIX_ROUTING_BENCH !== "1" }, async t => {
        const f = await fixture(t, "benchmark");
        if (count === 0) await f.run(["job", "cancel", f.job.id]);
        for (let i = 1; i < count; i++) await f.run(["job", "create", "--project", f.project.id, "--title", `Candidate ${i}`]);
        if (history) {
            const path = join(f.dir, "history.json");
            await writeFile(path, JSON.stringify(Array.from({ length: history }, (_, i) => ({
                id: `message_${i}`, role: i % 2 ? "assistant" : "user", text: `Message ${i} ${"x".repeat(100)}`,
            }))));
            await f.run(["hook", "record", "--file", path, "--job", f.job.id], { session: "benchmark" });
        }
        const samples = [];
        for (let i = 0; i < 5; i++) {
            let processes = 0, cliBytes = 0, requestBytes = 0, requests = 0;
            const runner = async (args, options) => {
                processes++;
                const result = await runTaskix(args, options);
                cliBytes += Buffer.byteLength(JSON.stringify(result));
                return result;
            };
            const begin = performance.now();
            const result = await runHook({ hook_event_name: "UserPromptSubmit", session_id: "benchmark", cwd: f.dir, turn_id: `turn_${i}`, prompt: "Continue the integration work" }, runner, {
                cacheDir: join(f.dir, "receipts"),
                env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://unused.test", TASKIX_JEV_API_KEY: "mock" },
                fetch: async (_url, init) => {
                    requests++;
                    requestBytes = Buffer.byteLength(init.body);
                    const request = JSON.parse(init.body);
                    const choice = count ? `resume:${f.job.id}` : "new_job";
                    return { ok: true, json: async () => (choiceAnswers(request, { intent: "work", route: choice })) };
                },
            });
            const content = result.hookSpecificOutput.additionalContext;
            assert.equal(processes, count > 0 && count <= 32 ? 2 : 1);
            assert.equal(requests, count > 32 ? 0 : 1);
            assert.match(content, count > 32 ? /Agent/ : count ? /Taskix route: resume/ : /Taskix route: new_job/);
            assert.ok(content.length <= 12000);
            samples.push({ ms: performance.now() - begin, processes, cliBytes, requestBytes, contextChars: content.length });
        }
        samples.sort((a, b) => a.ms - b.ms);
        t.diagnostic(JSON.stringify({ candidates: count, history, median: samples[2], maxMs: samples[4].ms }));
    });
}

for (const host of ["codex", "claude", "pi", "omp"]) for (const changed of [false, true]) test(`${host}_main_agent_fallback_preserves_guarded_followup_and_rejects_stale_writes changed=${changed}`, async t => {
    const f = await fixture(t);
    const ownerOptions = { cwd: f.dir, session: `session:${host}`, executor: `agent:${host}` };
    const old = await f.run(["task", "add", "--job", f.job.id, "--title", "Original implementation"], ownerOptions);
    const claim = await f.run(["task", "claim", old.id], ownerOptions);
    const leased = { ...ownerOptions, token: claim.lease.token };
    await f.run(["plan", "create", old.id, "--body", "Deliver implementation"], leased);
    await f.run(["task", "start", old.id], leased);
    await f.run(["task", "done", old.id], leased);
    const { createServer } = await import("node:http");
    let requests = 0;
    const server = createServer((req, res) => {
        requests++;
        req.resume();
        res.writeHead(503).end("Synthetic outage");
    });
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
    const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: `http://127.0.0.1:${server.address().port}`, TASKIX_JEV_API_KEY: "fixture-key", TASKIX_JEV_METRICS_ENABLED: "false" };
    let content;
    if (["pi", "omp"].includes(host)) {
        const previous = Object.fromEntries(Object.keys(env).map(key => [key, process.env[key]]));
        Object.assign(process.env, env);
        t.after(() => { for (const [key, value] of Object.entries(previous)) {
            if (value === undefined) delete process.env[key]; else process.env[key] = value;
        } });
        const x = await extension(t, f, host);
        content = (await x.handlers.get("before_agent_start")({ prompt: "Create the PR" }, x.ctx)).message.content;
    } else {
        const hooks = JSON.parse(await readFile("hooks/hooks.json", "utf8"));
        const output = await hookProcess({ hook_event_name: "UserPromptSubmit", session_id: ownerOptions.session,
            cwd: f.dir, prompt: "Create the PR", ...(host === "codex" ? { turn_id: "fallback-turn" } : {}),
        }, hooks.hooks.UserPromptSubmit[0].hooks[0].command, host, resolve("."), "/bin/sh", env);
        content = output.hookSpecificOutput.additionalContext;
    }
    assert.equal(requests, 1, "each host tries Jev once before main-Agent fallback");
    assert.match(content, /Jev deferred to the current Agent/);
    assert.match(content, /--expect-revision/);
    assert.doesNotMatch(content, /Delegate once|Snapshot:/);
    const facts = JSON.parse(content.slice(content.lastIndexOf("\n") + 1));
    assert.ok(facts.candidate_ids.includes(f.job.id));
    // Substitute only the main Agent's semantic selection; all reads and writes use real CLI guards.
    const selected = await f.run(["job", "show", f.job.id], ownerOptions);
    assert.equal(selected.status, "PENDING_REVIEW", "routing advice must not reopen the Job");
    const followupArgs = ["job", "followup", selected.id, "--expect-revision", String(selected.revision)];
    if (changed) {
        await f.run(["job", "update", selected.id, "--name", "Human renamed the delivery"]);
        await assert.rejects(f.run([...followupArgs, "--prompt", "Create the PR"], ownerOptions), /revision changed/);
        assert.equal((await f.run(["job", "show", selected.id])).status, "PENDING_REVIEW");
    } else {
        await f.run([...followupArgs, "--prompt", "Create the PR"], ownerOptions);
        const next = await f.run(["task", "add", "--job", selected.id, "--title", "PR delivery"], ownerOptions);
        assert.deepEqual(next.dependencies, [old.id]);
        assert.equal((await f.run(["job", "show", selected.id])).review_policy, "required");
    }
});

test("real CLI prompt latency includes optional metrics writing", async t => {
    const f = await fixture(t);
    const path = join(f.dir, "metrics.sqlite");
    const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://unused.test", TASKIX_JEV_API_KEY: "mock", TASKIX_JEV_METRICS_DB: path };
    const elapsed = {};
    let lock;
    for (const mode of ["off", "on", "locked"]) {
        if (mode === "locked") {
            const { DatabaseSync } = await import("node:sqlite");
            lock = new DatabaseSync(path); lock.exec("BEGIN IMMEDIATE");
        }
        const start = performance.now();
        try {
            const result = await runHook({ hook_event_name: "UserPromptSubmit", session_id: `metrics_${mode}`, cwd: f.dir, prompt: "Explain the current status" }, runTaskix, {
                env: { ...env, TASKIX_JEV_METRICS_ENABLED: mode === "off" ? "false" : "true" }, cacheDir: join(f.dir, "routing"),
                fetch: async (_url, init) => ({ ok: true, json: async () => (choiceAnswers(JSON.parse(init.body), { intent: "question", route: "new_job", review_policy: "not_applicable" })) }),
            });
            elapsed[mode] = performance.now() - start;
            assert.match(result.hookSpecificOutput.additionalContext, /Taskix route: discussion/);
        } finally { if (lock) { lock.exec("ROLLBACK"); lock.close(); lock = null; } }
    }
    t.diagnostic(`Real CLI and SQLite whole-hook ms; HTTP mocked; startup excluded: ${JSON.stringify(elapsed)}`);
});

for (const host of ["codex","claude","pi","omp"]) for (const mode of ["create","followup","resume"]) test(`${host} discussion ${mode} keeps selected original turns and no unrelated Job history`,async t=>{
    const {selectDiscussion}=await import('../discussion.mjs');
    const session=`session:${host}`;
    const f=await fixture(t,session);
    const options={cwd:f.dir,session,executor:`agent:${host}`};
    if(mode!=="create")await f.run(['job','update',f.job.id,'--prompt','Original capacity requirement']);
    if(mode==="followup") {
        const task=await f.run(['task','add','--job',f.job.id,'--title','Original delivery'],options);
        const owned=await f.run(['task','claim',task.id],options);
        const owner={...options,token:owned.lease.token};
        await f.run(['plan','create',task.id,'--body','Original implementation'],owner);
        await f.run(['task','start',task.id],owner);
        await f.run(['task','done',task.id],owner);
        assert.equal((await f.run(['job','show',f.job.id])).status,'PENDING_REVIEW');
    }
    const x=['pi','omp'].includes(host)?await extension(t,f,host):undefined;
    const transcript=join(f.dir,'discussion.jsonl');
    const rows=[];
    const prompts=['Why are messages truncated?','Can multiple cards work?','Unrelated topic','Keep short messages in one card','Implement the latest capacity plan'];
    let current;
    for(let i=0;i<prompts.length;i++) {
        const prompt=prompts[i];
        if(x) {
            await x.handlers.get('before_agent_start')({prompt},x.ctx);
            if(i<prompts.length-1)await x.handlers.get('agent_end')({messages:[{role:'user',content:prompt},{role:'assistant',content:`Answer ${i}`}]},x.ctx);
        } else {
            if(host==='codex')rows.push({type:'event_msg',payload:{type:'task_started',turn_id:`t${i}`}},
                {type:'response_item',payload:{type:'message',id:'u',role:'user',content:prompt}},
                {type:'response_item',payload:{type:'message',id:'a',role:'assistant',content:`Answer ${i}`}});
            else rows.push({type:'user',uuid:`u${i}`,message:{role:'user',content:prompt}},
                {type:'assistant',uuid:`a${i}`,message:{role:'assistant',content:`Answer ${i}`}});
            await writeFile(transcript,rows.map(JSON.stringify).join('\n'));
            await runHook({hook_event_name:i===prompts.length-1?'PreToolUse':'Stop',session_id:session,cwd:f.dir,transcript_path:transcript});
        }
        const pending=(await runTaskix(['conversation','list','--limit','100'],options)).result;
        current=pending.turns.at(-1).turn_id;
        assert.deepEqual((await f.run(['job','show',f.job.id])).conversation,[],'discussion does not leak into a previous Job');
    }
    const target={title:'Capacity cards',goal:'Only split long messages',prompt:prompts.at(-1),...(mode==='create'?{}:{job_id:f.job.id})};
    let selected=await selectDiscussion({target,current_turn:current},options,runTaskix,{
        env:mode==='resume'?{}:{TASKIX_JEV_ENABLED:'true',TASKIX_JEV_URL:'https://unused.test',TASKIX_JEV_API_KEY:'test'},
        fetch:async(_url,init)=>{
            const request=JSON.parse(init.body);
            return {ok:true,json:async()=>({answers:Object.fromEntries(Object.entries(request.questions).map(([id,q],i)=>{
                const choice=i===2?'unrelated':'related';return [id,{type:'choice',choice,confidence:1,probabilities:Object.fromEntries(Object.keys(q.criteria).map(key=>[key,key===choice?1:0]))}];
            }))})};
        },
    });
    if(mode==='resume') {
        assert.equal(selected.status,'agent');
        const full=(await runTaskix(['conversation','list','--full'],options)).result;
        const job=await f.run(['job','show',f.job.id]);
        const related=full.turns.filter(turn=>!turn.messages.some(m=>m.text==='Unrelated topic'));
        selected={status:'selected',args:['--conversation-revision',String(full.revision),'--expect-revision',String(job.revision),...related.flatMap(turn=>['--turn',turn.turn_id])]};
    }
    assert.equal(selected.status,'selected');
    const args=mode==='create'?['job','create','--project',f.project.id,'--title',target.title,'--goal',target.goal,'--prompt',target.prompt]
        :mode==='followup'?['job','followup',f.job.id,'--prompt',target.prompt]:['conversation','attach','--job',f.job.id];
    const job=await f.run([...args,...selected.args],options);
    assert.equal(job.prompt,mode==='create'?prompts[0]:'Original capacity requirement');
    assert.equal(job.status,'ACTIVE');
    if(mode!=='create')assert.equal(job.id,f.job.id);
    assert.deepEqual(job.conversation.filter(m=>m.role==='user').map(m=>m.text),prompts.filter((_,i)=>i!==2));
    const draft=(await runTaskix(['conversation','list','--full'],options)).result;
    assert.equal(draft.turns.length,1);
    assert.equal(draft.turns[0].messages[0].text,'Unrelated topic');
    const note=await readFile(join(f.root,'Tasks ☃',job.document_path),'utf8');
    assert.ok(note.includes('Why are messages truncated?'));
    assert.ok(note.includes('Keep short messages in one card'));
    assert.ok(!note.includes('Unrelated topic'));
    if(x)await x.handlers.get('agent_end')({messages:[{role:'assistant',content:'Implementation complete'}]},x.ctx);
    else await runHook({hook_event_name:'Stop',session_id:session,cwd:f.dir,transcript_path:transcript});
    const after=await f.run(['job','show',job.id]);
    assert.equal(after.conversation.filter(m=>m.role==='user').length,4);
});

// Execute the packaged standalone helper, not only its imported implementation.
test("packaged_discussion_cli_returns_guarded_arguments_and_attaches_the_original_turn", async t => {
    const session = "discussion-cli", f = await fixture(t, session);
    const options = { cwd: f.dir, session, executor: "agent:codex" };
    const transcript = join(f.dir, "turn.json");
    await writeFile(transcript, JSON.stringify({ turn_id: "current", source: "test", messages: [
        { id: "original", role: "user", text: "Improve the current fallback" },
    ] }));
    await f.run(["hook", "record", "--file", transcript], options);
    const packaged = join(f.dir, "packaged plugin");
    await mkdir(packaged);
    const pkg = JSON.parse(await readFile("package.json", "utf8"));
    for (const file of ["package.json", ...pkg.files.filter(file => file.endsWith(".mjs"))]) {
        await cp(resolve(file), join(packaged, file));
    }
    const child = spawn(process.execPath, [join(packaged, "discussion.mjs"), session], {
        cwd: f.dir, env: { ...process.env, TASKIX_JEV_ENABLED: "false" }, stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "", stderr = "";
    child.stdout.on("data", chunk => { stdout += chunk; });
    child.stderr.on("data", chunk => { stderr += chunk; });
    const exited = new Promise((resolve, reject) => { child.on("error", reject); child.on("close", resolve); });
    child.stdin.end(JSON.stringify({ current_turn: "current", target: {
        title: "Improve fallback", prompt: "Improve the current fallback", job_id: f.job.id,
    } }));
    assert.equal(await exited, 0, stderr);
    const selected = JSON.parse(stdout);
    assert.equal(selected.status, "selected");
    assert.ok(selected.args.includes("--expect-revision"));
    await f.run(["conversation", "attach", "--job", f.job.id, ...selected.args], options);
    const job = await f.run(["job", "show", f.job.id]);
    assert.deepEqual(job.conversation.map(message => message.text), ["Improve the current fallback"]);
});

for (const host of ["codex", "claude", "pi", "omp"]) test(`${host} lifecycle assessment uses real HTTP and CLI guards without mutating work`, async t => {
    const { createServer } = await import("node:http");
    const { registerExtension } = await import("../runtime.mjs");
    const session = `lifecycle-${host}`, f = await fixture(t, session);
    const owner = { cwd: f.dir, session, executor: `agent:${host}` };
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Choose region and verify deployment"]);
    owner.token = (await f.run(["task", "claim", task.id], owner)).lease.token;
    await f.run(["plan", "create", task.id, "--body", "Verify deployment in the selected region"], owner);
    await f.run(["task", "start", task.id], owner);
    let choice = "wait", requests = 0;
    const server = createServer(async (req, res) => {
        let text = "";
        for await (const chunk of req) text += chunk;
        const request = JSON.parse(text); requests++;
        assert.ok(Buffer.byteLength(text) <= 30000);
        assert.ok(!text.includes(owner.token));
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(choiceAnswers(request, { outcome: choice, recovery: choice })));
    });
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
    const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: `http://127.0.0.1:${server.address().port}`, TASKIX_JEV_API_KEY: "test" };
    let tool;
    if (["pi", "omp"].includes(host)) registerExtension({ on() {}, registerTool: registered => { tool = registered; } }, host, runTaskix, globalThis, undefined, { env });
    const classify = async kind => {
        const input = { kind, job_id: f.job.id, task_id: task.id, prompt: "Deploy to the chosen region", history: [{ role: "assistant", text: "Need the user to select a region before deployment." }] };
        if (tool) return (await tool.execute("assessment", { args: ["lifecycle", "classify", JSON.stringify(input)] }, undefined, undefined,
            { cwd: f.dir, sessionManager: { getSessionId: () => session } })).details;
        const child = spawn(process.execPath, [resolve("lifecycle.mjs"), session], { cwd: f.dir, env: { ...process.env, ...env }, stdio: ["pipe", "pipe", "pipe"] });
        let stdout = "", stderr = "";
        child.stdout.on("data", c => { stdout += c; }); child.stderr.on("data", c => { stderr += c; });
        child.stdin.end(JSON.stringify(input));
        assert.equal(await new Promise(resolve => child.on("close", resolve)), 0, stderr);
        return JSON.parse(stdout);
    };
    const verdict = await classify("outcome");
    assert.equal(verdict.status, "selected");
    assert.equal(verdict.decision.action, "wait");
    assert.equal((await f.run(["task", "show", task.id])).status, "IN_PROGRESS");
    await f.run([...verdict.args, "--reason", "Need region"], owner);
    assert.equal((await f.run(["task", "show", task.id])).status, "WAITING_USER");
    choice = "resume";
    const recovery = await classify("recovery");
    assert.equal(recovery.decision.action, "resume");
    const recovered = await f.run(recovery.args, { ...owner, token: undefined });
    owner.token = recovered.lease.token;
    await assert.rejects(f.run(recovery.args, { ...owner, token: undefined }), /revision|lease|claimed/);
    await f.run(["task", "start", task.id], owner);
    choice = "ready";
    const ready = await classify("outcome");
    assert.equal(ready.decision.requires_verification, true);
    assert.deepEqual(ready.args, []);
    assert.equal((await f.run(["task", "show", task.id])).status, "IN_PROGRESS");
    assert.equal((await f.run(["job", "show", f.job.id])).status, "ACTIVE");
    assert.equal(requests, 3);
});

for (const [kind, action, prepare, expected] of [
    ["outcome", "block", null, "BLOCKED"], ["outcome", "fail", null, "FAILED"],
    ["recovery", "retry", "fail", "TODO"], ["recovery", "reopen", "done", "TODO"],
    ["recovery", "cancel", null, "CANCELLED"], ["recovery", "release", null, "BLOCKED"],
]) test(`Jev ${kind} ${action} obeys real Task transition guards`, async t => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = await fixture(t, "assessment"), owner = { session: "assessment", executor: "agent:test" };
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Implement and verify"]);
    owner.token = (await f.run(["task", "claim", task.id], owner)).lease.token;
    await f.run(["plan", "create", task.id, "--body", "Verify implementation"], owner);
    await f.run(["task", "start", task.id], owner);
    if (prepare) {
        await f.run(["task", prepare, task.id, ...(prepare === "fail" ? ["--reason", "Validation failed"] : [])], owner);
        delete owner.token;
    }
    const input = { kind, job_id: f.job.id, task_id: task.id, prompt: "Explicit user request", history: [] };
    const verdict = await assessLifecycle(input, { cwd: f.dir, session: owner.session }, runTaskix, {
        env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://mock.test", TASKIX_JEV_API_KEY: "test" },
        fetch: async (_url, init) => ({ ok: true, json: async () => choiceAnswers(JSON.parse(init.body), { [kind]: action }) }),
    });
    assert.equal(verdict.status, "selected", JSON.stringify(verdict));
    const before = await f.run(["task", "show", task.id]);
    assert.notEqual(before.status, expected);
    await f.run([...verdict.args, ...(verdict.required_arguments.length ? ["--reason", "Verified reason"] : [])], owner);
    assert.equal((await f.run(["task", "show", task.id])).status, expected);
});

for (const action of ["approve", "reject", "cancel"]) test(`Jev explicit Job ${action} produces a revision-guarded real transition`, async t => {
    const { routePrompt } = await import("../jev.mjs");
    const f = await fixture(t, "job-review"), options = { cwd: f.dir, session: "job-review", executor: "agent:test" };
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Deliver"]);
    const token = (await f.run(["task", "claim", task.id], options)).lease.token;
    await f.run(["plan", "create", task.id, "--body", "Verify"], { ...options, token });
    await f.run(["task", "start", task.id], { ...options, token });
    await f.run(["task", "done", task.id], { ...options, token });
    const context = (await runTaskix(["routing", "snapshot"], options)).result;
    const result = await routePrompt({ prompt: "Explicit user review decision", context, options, runner: runTaskix,
        env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://mock.test", TASKIX_JEV_API_KEY: "test" },
        fetch: async (_url, init) => ({ ok: true, json: async () => choiceAnswers(JSON.parse(init.body), { intent: action, route: `followup:${f.job.id}`, review_policy: "not_applicable" }) }),
    });
    assert.equal(result.decision.action, action);
    assert.equal((await f.run(["job", "show", f.job.id])).status, "PENDING_REVIEW");
    const args = ["job", action, f.job.id, "--expect-revision", String(result.decision.job_revision), ...(action === "reject" ? ["--reason", "User rejected verification"] : [])];
    await f.run(args);
    assert.equal((await f.run(["job", "show", f.job.id])).status, { approve: "COMPLETED", reject: "ACTIVE", cancel: "CANCELLED" }[action]);
    await assert.rejects(f.run(args), /revision|conflict/);
});

for (const [initial, choice, effective] of [["none", "required", "required"], ["required", "none", "required"], ["none", "none", "none"]]) test(`Jev review policy ${initial} plus ${choice} persists ${effective} and controls completion`, async t => {
    const { assessLifecycle } = await import("../lifecycle.mjs");
    const f = await fixture(t, "policy-assessment"), owner = { cwd: f.dir, session: "policy-assessment", executor: "agent:test" };
    await f.run(["job", "update", f.job.id, "--review-policy", initial]);
    const verdict = await assessLifecycle({ kind: "review_policy", job_id: f.job.id, prompt: "Classify the requested scope" }, owner, runTaskix, {
        env: { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://mock.test", TASKIX_JEV_API_KEY: "test" },
        fetch: async (_url, init) => ({ ok: true, json: async () => choiceAnswers(JSON.parse(init.body), { review_policy: choice }) }),
    });
    assert.equal(verdict.status, "selected");
    assert.equal(verdict.decision.action, effective);
    assert.equal((await f.run(["job", "show", f.job.id])).review_policy, initial);
    await f.run(verdict.args, owner);
    assert.equal((await f.run(["job", "show", f.job.id])).review_policy, effective);
    const task = await f.run(["task", "add", "--job", f.job.id, "--title", "Verify scope"]);
    owner.token = (await f.run(["task", "claim", task.id], owner)).lease.token;
    await f.run(["plan", "create", task.id, "--body", "Verify acceptance criteria"], owner);
    await f.run(["task", "start", task.id], owner);
    await f.run(["task", "done", task.id], owner);
    assert.equal((await f.run(["job", "show", f.job.id])).status, effective === "required" ? "PENDING_REVIEW" : "COMPLETED");
    await assert.rejects(f.run(verdict.args, owner), /revision|conflict/);
});
