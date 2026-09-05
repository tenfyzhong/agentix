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
    return { dir, root, job, run, cleanup };
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
    let tool;
    const pkg = JSON.parse(await readFile("package.json", "utf8"));
    assert.equal(pkg[host].extensions.length, 1);
    const { default: install } = await import(
        pathToFileURL(resolve(pkg[host].extensions[0]))
    );
    install({
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
    };
    await handlers.get("session_start")({}, ctx);
    f.cleanup.push(() => handlers.get("session_shutdown")({}, ctx));
    let calls = 0;
    const invoke = async (args, id = `call-${++calls}`) =>
        (await tool.execute(id, { args }, undefined, undefined, ctx)).details
            .result;
    return { invoke, handlers, ctx };
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
        assert.ok(body.includes(`- [x] ${task.name}`));
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
            for (const path of ["hooks", "runtime.mjs"]) {
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
                await readFile(join(root, "hooks/hooks.json"), "utf8"),
            );
            for (const name of [
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "Stop",
                "SessionEnd",
                "SessionStart",
            ]) {
                const command = manifest.hooks[name][0].hooks[0].command;
                const output = await hookProcess(
                    {
                        hook_event_name: name,
                        session_id: options.session,
                        cwd: f.dir,
                    },
                    command,
                    host,
                    root,
                    shell,
                );
                const current = await f.run(["task", "show", task.id]);
                assert.equal(
                    current.status,
                    name === "SessionEnd" ? "BLOCKED" : "IN_PROGRESS",
                );
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
            await f.run(["task", "done", task.id], resumedOptions);
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
