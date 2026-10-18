import assert from "node:assert/strict";
import { test } from "node:test";
import {
    mkdtemp,
    mkdir,
    writeFile,
    readFile,
    rm,
    chmod,
    realpath,
    cp,
    stat,
    symlink,
    readlink,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync, spawn } from "node:child_process";
import http from "node:http";
import { once } from "node:events";
import { isolatedOpenCodeEnvironment } from "./fixtures/opencode-headless.mjs";
import { fileURLToPath, pathToFileURL } from "node:url";
const root = fileURLToPath(new URL("../../../", import.meta.url));
async function fixture(t) {
    const dir = await mkdtemp(join(tmpdir(), "opencode-install-"));
    const cleanups = [];
    t.after(async () => {
        for (const cleanup of cleanups.reverse()) await cleanup();
        await rm(dir, {
            recursive: true,
            force: true,
            maxRetries: 10,
            retryDelay: 100,
        });
    });
    const config = join(dir, "config");
    await mkdir(config);
    const runWithEnv = (env, ...args) =>
        spawnSync(process.execPath, ["scripts/opencode-plugin.mjs", ...args], {
            cwd: root,
            env: { ...process.env, ...env, OPENCODE_CONFIG_DIR: config },
            encoding: "utf8",
        });
    return {
        config,
        runWithEnv,
        run: (...args) => runWithEnv({}, ...args),
        after: (cleanup) => cleanups.push(cleanup),
    };
}
test("opencode_installs_native_v2_directory_plugins_without_dependencies", async (t) => {
    const f = await fixture(t);
    const initial =
        '{// keep this comment\n "theme": "dark", "plugins": ["unrelated-cli",],\n "keybinds": {"leader":"ctrl+x",},\n}\n';
    await writeFile(join(f.config, "cli.json"), initial);
    await writeFile(
        join(f.config, "opencode.json"),
        JSON.stringify({
            model: { providerID: "test", model: "model" },
            plugins: ["other-server"],
        }),
    );
    for (let attempt = 0; attempt < 2; attempt++) {
        const result = f.run("install", "local");
        assert.equal(result.status, 0, result.stderr);
        const cli = await readFile(join(f.config, "cli.json"), "utf8");
        assert.match(cli, /\/\/ keep this comment/);
        assert.match(cli, /"keybinds": \{"leader":"ctrl\+x",\}/);
        assert.equal((cli.match(/plugins\/agentix-bridge/g) || []).length, 1);
        assert.doesNotMatch(cli, /opencode\/tui.mjs/);
        const config = JSON.parse(
            await readFile(join(f.config, "opencode.json"), "utf8"),
        );
        assert.deepEqual(config.model, { providerID: "test", model: "model" });
        assert.equal(config.plugins.length, 2);
        assert.match(config.plugins[1], /plugins\/taskix-manager$/);
        assert.equal(config.plugin, undefined);
    }
    const installed = join(f.config, ".agentix-plugin");
    const pkg = JSON.parse(
        await readFile(join(installed, "package.json"), "utf8"),
    );
    assert.ok(pkg.exports["."]);
    assert.ok(pkg.exports["./tui"]);
    const server = await import(
        pathToFileURL(
            join(installed, "plugins/taskix-manager/index.js"),
        )
    );
    assert.equal(typeof server.default.setup, "function");
    const cli = await import(
        pathToFileURL(
            join(installed, "plugins/agentix-bridge/tui.js"),
        )
    );
    assert.equal(typeof cli.default.setup, "function");
    await assert.rejects(
        readFile(
            join(installed, "plugins/taskix-manager/opencode/package.json"),
        ),
        { code: "ENOENT" },
    );
    await assert.rejects(
        readFile(join(installed, "node_modules/zod/package.json")),
        { code: "ENOENT" },
    );
    assert.equal(f.run("remove").status, 0);
    const removed = await readFile(join(f.config, "cli.json"), "utf8");
    assert.match(removed, /unrelated-cli/);
    assert.doesNotMatch(removed, /agentix-plugin/);
    assert.deepEqual(
        JSON.parse(await readFile(join(f.config, "opencode.json"), "utf8"))
            .plugins,
        ["other-server"],
    );
});
test("opencode_update_removes_only_owned_legacy_v1_plugin_entries", async (t) => {
    const f = await fixture(t);
    const old = pathToFileURL(
        join(
            f.config,
            ".agentix-plugin/plugins/agentix-bridge/opencode/tui.mjs",
        ),
    ).href;
    await writeFile(
        join(f.config, "tui.json"),
        JSON.stringify({ theme: "dark", plugin: ["other-old-tui", old] }),
    );
    await writeFile(
        join(f.config, "opencode.json"),
        JSON.stringify({
            plugin: [
                "other-old-server",
                { package: old, options: { keep: false } },
            ],
            plugins: [{ package: "other-v2-server", options: { keep: true } }],
        }),
    );
    const result = f.run("install", "local");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(
        JSON.parse(await readFile(join(f.config, "tui.json"), "utf8")),
        { theme: "dark", plugin: ["other-old-tui"] },
    );
    const config = JSON.parse(
        await readFile(join(f.config, "opencode.json"), "utf8"),
    );
    assert.deepEqual(config.plugin, ["other-old-server"]);
    assert.deepEqual(config.plugins[0], {
        package: "other-v2-server",
        options: { keep: true },
    });
    assert.match(config.plugins[1], /plugins\/taskix-manager$/);
});
test("opencode_rejects_invalid_config_before_replacing_installation", async (t) => {
    const f = await fixture(t);
    await writeFile(join(f.config, "cli.json"), "not JSON");
    const result = f.run("install", "local");
    assert.notEqual(result.status, 0);
    assert.equal(
        await readFile(join(f.config, "cli.json"), "utf8"),
        "not JSON",
    );
});

for (const action of ["install", "remove"]) {
    test(`opencode_${action}_restores_package_and_all_configs_when_publication_fails`, async (t) => {
        const f = await fixture(t);
        const installed = f.run("install", "local");
        assert.equal(installed.status, 0, installed.stderr);
        const marker = join(f.config, ".agentix-plugin", "previous-install");
        await writeFile(marker, "keep installed package");
        // Ensure the first config also changes before the second write fails.
        if (action === "install") {
            await writeFile(join(f.config, "cli.json"), '{"plugins":[],"theme":"fixture"}\n');
        }
        const before = await Promise.all(
            ["cli.json", "opencode.json"].map((name) => readFile(join(f.config, name), "utf8")),
        );
        const result = f.runWithEnv({
            NODE_OPTIONS: `--import=${new URL("./fixtures/opencode-install-fault.mjs", import.meta.url).href}`,
            OPENCODE_TEST_FAIL_RENAME: join(f.config, "opencode.json"),
        }, action, "local");
        assert.notEqual(result.status, 0);
        assert.match(result.stderr, /Fixture config publication failed/);
        assert.equal(await readFile(marker, "utf8"), "keep installed package");
        assert.deepEqual(await Promise.all(
            ["cli.json", "opencode.json"].map((name) => readFile(join(f.config, name), "utf8")),
        ), before);
    });
}

test("opencode_failed_first_install_removes_new_configs_and_package", async (t) => {
    const f = await fixture(t);
    const result = f.runWithEnv({
        NODE_OPTIONS: `--import=${new URL("./fixtures/opencode-install-fault.mjs", import.meta.url).href}`,
        OPENCODE_TEST_FAIL_RENAME: join(f.config, "opencode.json"),
    }, "install", "local");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Fixture config publication failed/);
    for (const path of ["cli.json", "opencode.json", ".agentix-plugin/package.json"])
        await assert.rejects(readFile(join(f.config, path)), { code: "ENOENT" });
});

test("opencode_config_updates_preserve_restrictive_file_permissions", { skip: process.platform === "win32" }, async (t) => {
    const f = await fixture(t);
    const path = join(f.config, "opencode.json");
    await writeFile(path, "{}\n", { mode: 0o600 });
    for (const action of ["install", "remove"]) {
        const result = f.run(action, "local");
        assert.equal(result.status, 0, result.stderr);
        assert.equal((await stat(path)).mode & 0o777, 0o600);
    }
});

for (const relative of [false, true]) {
    test(`opencode_failed_update_preserves_${relative ? "relative" : "absolute"}_config_symlinks`, { skip: process.platform === "win32" }, async (t) => {
        const f = await fixture(t);
        const original = '{"plugins":[],"theme":"original"}\n';
        const target = join(f.config, "dotfile.json");
        const link = relative ? "dotfile.json" : target;
        await writeFile(target, original);
        await symlink(link, join(f.config, "cli.json"));
        const result = f.runWithEnv({
            NODE_OPTIONS: `--import=${new URL("./fixtures/opencode-install-fault.mjs", import.meta.url).href}`,
            OPENCODE_TEST_FAIL_RENAME: join(f.config, "opencode.json"),
        }, "install", "local");
        assert.notEqual(result.status, 0);
        assert.match(result.stderr, /Fixture config publication failed/);
        assert.equal(await readFile(target, "utf8"), original);
        assert.equal(await readFile(join(f.config, "cli.json"), "utf8"), original);
        assert.equal(await readlink(join(f.config, "cli.json")), link);
    });
}

test(
    "native_opencode_loads_installed_server_over_tcp_and_sends_taskix_schema",
    {
        skip: process.env.AGENTIX_TEST_OPENCODE_NATIVE !== "1",
        timeout: 180000,
    },
    async (t) => {
        const f = await fixture(t);
        const home = join(f.config, "isolated-home");
        const project = join(home, "project");
        const vault = join(home, "vault");
        await mkdir(project, { recursive: true });
        await mkdir(join(home,"tmp"), {recursive:true});
        await mkdir(join(vault, ".obsidian"), { recursive: true });
        const binary = process.env.AGENTIX_TEST_OPENCODE || "opencode";
        const taskix = process.env.AGENTIX_TEST_TASKIX;
        assert.ok(
            taskix,
            "Provide AGENTIX_TEST_TASKIX with the current built taskix binary",
        );
        await readFile(taskix);
        const env = isolatedOpenCodeEnvironment(home, f.config, taskix);
        env.TASKIX_CONFIG = join(home, "taskix.toml");
        await writeFile(
            env.TASKIX_CONFIG,
            `schema_version = 1\n[storage]\npath = ${JSON.stringify(join(home, "tasks.sqlite3"))}\n[documents]\nroot = ${JSON.stringify(vault)}\ndirectory = "Tasks"\n`,
        );
        const version = spawnSync(binary, ["--version"], {
            env,
            encoding: "utf8",
            timeout: 15000,
        });
        assert.equal(version.status, 0, version.stderr);
        assert.equal(
            version.stdout.trim(),
            "opencode v2.0.32",
            "Native acceptance is pinned to OpenCode 2.0.32",
        );
        const requests = [];
        let overflowReturned = false;
        const model = http.createServer(async (req, res) => {
            let body = "";
            for await (const chunk of req) body += chunk;
            const input = JSON.parse(body);
            requests.push(input);
            const hasImage = input.messages?.some(
                (message) =>
                    Array.isArray(message.content) &&
                    message.content.some(
                        (part) =>
                            part.type === "image_url" || part.type === "image",
                    ),
            );
            if (
                input.stream &&
                input.tools?.length &&
                hasImage &&
                !overflowReturned
            ) {
                overflowReturned = true;
                res.writeHead(400, { "content-type": "application/json" });
                res.end(
                    JSON.stringify({
                        error: {
                            message: "maximum context length exceeded",
                            type: "invalid_request_error",
                            code: "context_length_exceeded",
                        },
                    }),
                );
                return;
            }
            const message = {
                role: "assistant",
                content: "Native headless fixture answer",
            };
            if (!input.stream) {
                res.writeHead(200, { "content-type": "application/json" });
                res.end(
                    JSON.stringify({
                        id: "fixture",
                        object: "chat.completion",
                        created: 1,
                        model: "fixture",
                        choices: [{ index: 0, message, finish_reason: "stop" }],
                        usage: {
                            prompt_tokens: 10,
                            completion_tokens: 5,
                            total_tokens: 15,
                        },
                    }),
                );
                return;
            }
            res.writeHead(200, { "content-type": "text/event-stream" });
            res.write(
                "data: " +
                    JSON.stringify({
                        id: "fixture",
                        object: "chat.completion.chunk",
                        created: 1,
                        model: "fixture",
                        choices: [
                            { index: 0, delta: message, finish_reason: null },
                        ],
                    }) +
                    "\n\n",
            );
            res.write(
                "data: " +
                    JSON.stringify({
                        id: "fixture",
                        object: "chat.completion.chunk",
                        created: 1,
                        model: "fixture",
                        choices: [
                            { index: 0, delta: {}, finish_reason: "stop" },
                        ],
                    }) +
                    "\n\n",
            );
            res.end("data: [DONE]\n\n");
        });
        await new Promise((resolve) => model.listen(0, "127.0.0.1", resolve));
        f.after(async () => {
            model.closeAllConnections();
            await new Promise((resolve) => model.close(resolve));
        });
        await writeFile(
            join(f.config, "opencode.json"),
            JSON.stringify({
                model: { providerID: "fixture", model: "fixture" },
                agents: {
                    title: {
                        model: { providerID: "fixture", model: "fixture" },
                    },
                },
                policies: [
                    { action: "provider.use", resource: "*", effect: "deny" },
                    {
                        action: "provider.use",
                        resource: "fixture",
                        effect: "allow",
                    },
                ],
                providers: {
                    fixture: {
                        name: "Fixture",
                        package: "aisdk:@ai-sdk/openai-compatible",
                        settings: {
                            baseURL: `http://127.0.0.1:${model.address().port}/v1`,
                            apiKey: "isolated-fixture",
                        },
                        models: {
                            fixture: {
                                name: "Fixture",
                                capabilities: {
                                    tools: true,
                                    input: ["text", "image"],
                                    output: ["text"],
                                },
                                limit: { context: 32768, output: 4096 },
                            },
                        },
                    },
                },
            }),
        );
        const installed = spawnSync(
            process.execPath,
            ["scripts/opencode-plugin.mjs", "install", process.env.AGENTIX_TEST_OPENCODE_SOURCE || "local"],
            { cwd: root, env: { ...env, OPENCODE: binary }, encoding: "utf8" },
        );
        assert.equal(installed.status, 0, installed.stderr);
        const native = spawn(
            binary,
            ["serve", "--hostname", "127.0.0.1", "--port", "0"],
            { cwd: project, env, stdio: ["ignore", "pipe", "pipe"] },
        );
        let logs = "";
        native.stdout.on("data", (data) => {
            logs += data;
        });
        native.stderr.on("data", (data) => {
            logs += data;
        });
        f.after(async () => {
            if (native.exitCode !== null) return;
            const exit = once(native, "exit");
            native.kill();
            const kill = setTimeout(() => native.kill("SIGKILL"), 5000);
            await exit;
            clearTimeout(kill);
        });
        let base, password;
        const safeLogs = () =>
            logs.replace(
                /server password [^\s]+/g,
                "server password [redacted]",
            );
        const deadline = Date.now() + 60000;
        while (Date.now() < deadline) {
            base = logs.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0];
            password = logs.match(/server password ([^\s]+)/)?.[1];
            if (base && password) break;
            assert.equal(native.exitCode, null, safeLogs());
            await new Promise((resolve) => setTimeout(resolve, 50));
        }
        assert.ok(
            base && password,
            "Native TCP listener did not start: " + safeLogs(),
        );
        async function request(path, method = "GET", body) {
            const response = await fetch(base + path, {
                method,
                headers: {
                    "content-type": "application/json",
                    authorization:
                        "Basic " +
                        Buffer.from("opencode:" + password).toString("base64"),
                },
                body: body === undefined ? undefined : JSON.stringify(body),
                signal: AbortSignal.timeout(90000),
            });
            const text = await response.text();
            assert.equal(
                response.ok,
                true,
                `${method} ${path}: ${text}\n${safeLogs()}`,
            );
            return text ? JSON.parse(text) : undefined;
        }
        const info = await request("/api/info");
        assert.equal(info.version, "2.0.32");
        const locationQuery =
            "?" + new URLSearchParams({ "location[directory]": project });
        let skills;
        const skillDeadline = Date.now() + 15000;
        while (Date.now() < skillDeadline) {
            skills = await request("/api/skill" + locationQuery);
            if (skills.data.some((skill) => skill.name === "taskix-manager"))
                break;
            await new Promise((resolve) => setTimeout(resolve, 100));
        }
        assert.ok(
            skills.data.some((skill) => skill.name === "taskix-manager"),
            JSON.stringify({skills:skills.data.map(skill=>skill.name),plugins:await request("/api/plugin"+locationQuery)}),
        );
        const session = (
            await request("/api/session", "POST", {
                location: { directory: project },
                model: { providerID: "fixture", id: "fixture" },
            })
        ).data;
        const prompt = async (body) => {
            const admission = (
                await request(`/api/session/${session.id}/prompt`, "POST", body)
            ).data;
            assert.equal(admission.type, "user");
            await request(
                `/api/experimental/session/${session.id}/wait`,
                "POST",
            );
            return admission;
        };
        const first = await prompt({
            text: "Read-only native plugin smoke test.",
        });
        const schema = requests
            .flatMap((request) => request.tools || [])
            .find((tool) => tool.function?.name === "taskix");
        assert.ok(schema, "Taskix is a direct native tool with its own call ID");
        assert.equal(schema.function.parameters.properties.args.type, "array");
        assert.equal(schema.function.parameters.properties.args.items.type, "string");
        assert.deepEqual(schema.function.parameters.required, ["args"]);
        assert.ok(
            requests.some((request) =>
                request.messages.some(
                    (message) =>
                        message.role === "system" &&
                        message.content.includes("Task session: " + session.id),
                ),
            ),
        );
        const history = () =>
            request(`/api/session/${session.id}/message?order=asc&limit=100`);
        const initial = await history();
        assert.ok(
            initial.data.some(
                (message) => message.type === "user" && message.id === first.id,
            ),
        );
        assert.ok(
            initial.data.some(
                (message) =>
                    message.type === "assistant" &&
                    message.content.some(
                        (part) =>
                            part.type === "text" &&
                            part.text === "Native headless fixture answer",
                    ),
            ),
            JSON.stringify(initial),
        );
        const attachment = {
            uri: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4//8/AAX+Av4N70a4AAAAAElFTkSuQmCC",
            name: "attachment-only.png",
        };
        const attachmentPrompt = () =>
            prompt({
                text: "",
                files: [attachment],
                metadata: { fixture: "attachment" },
            });
        const admitted = await attachmentPrompt();
        let projection = await history();
        assert.equal(
            overflowReturned,
            true,
            "Actual attachment exercised native provider overflow compaction",
        );
        const user = projection.data.find(
            (message) => message.type === "user" && message.id === admitted.id,
        );
        assert.ok(user, JSON.stringify(projection));
        assert.equal(user.text, "");
        assert.equal(user.files[0].name, attachment.name);
        assert.equal(user.metadata.fixture, "attachment");
        const before = projection.data
            .filter((message) => message.type === "user")
            .map((message) => message.id);
        const compact = (
            await request(`/api/session/${session.id}/compact`, "POST", {})
        ).data;
        assert.equal(compact.type, "compaction");
        await request(`/api/experimental/session/${session.id}/wait`, "POST");
        projection = await history();
        assert.ok(
            projection.data.some((message) => message.type === "compaction"),
            JSON.stringify(projection),
        );
        assert.deepEqual(
            projection.data
                .filter((message) => message.type === "user")
                .map((message) => message.id),
            before,
            "V2 compaction preserves native user IDs without synthetic user replays",
        );
        const repeated = await attachmentPrompt();
        assert.notEqual(
            repeated.id,
            admitted.id,
            "Identical real attachment is admitted as a new native user turn",
        );
        const final = await history();
        assert.ok(
            final.data.some(
                (message) =>
                    message.type === "user" &&
                    message.id === repeated.id &&
                    message.files[0].name === attachment.name,
            ),
        );
    },
);

const remotePackages = {
    "opencode.json": "github:tenfyzhong/agentix#main::path:plugins/taskix-manager",
    "cli.json": "github:tenfyzhong/agentix#main::path:plugins/agentix-bridge",
};

async function remoteCLI(f) {
    const bin = join(f.config, "bin");
    await mkdir(bin);
    const executable = join(bin, "opencode");
    await cp(new URL("./fixtures/opencode-install-cli.mjs", import.meta.url), executable);
    await chmod(executable, 0o755);
    // The old clone implementation must fail offline, never access the network.
    await writeFile(join(bin, "git"), "#!/bin/sh\nexit 7\n");
    await chmod(join(bin, "git"), 0o755);
    return {
        OPENCODE: executable,
        OPENCODE_CONFIG: join(f.config, "user-config.json"),
        OPENCODE_CONFIG_CONTENT: '{"plugins":["unrelated-user-plugin"]}',
        PATH: bin + ":" + process.env.PATH,
        OPENCODE_TEST_CALLS: join(f.config, "calls.jsonl"),
    };
}

for (const existing of [false, true]) {
    test(`opencode_remote_${existing ? "migration" : "install"}_uses_native_github_packages`, { skip: process.platform === "win32" }, async (t) => {
        const f = await fixture(t);
        for (const name of Object.keys(remotePackages))
            await writeFile(join(f.config, name), '{// preserved\n"plugins":["unrelated"]}\n');
        if (existing) assert.equal(f.run("install", "local").status, 0);
        const env = await remoteCLI(f);
        for (let attempt = 0; attempt < 2; attempt++) {
            const result = f.runWithEnv(env, "install", "main");
            assert.equal(result.status, 0, result.stderr);
            for (const [name, ref] of Object.entries(remotePackages)) {
                const text = await readFile(join(f.config, name), "utf8");
                assert.match(text, /preserved/);
                assert.deepEqual(JSON.parse(text.replace("// preserved", "")).plugins, ["unrelated", ref]);
            }
            await assert.rejects(stat(join(f.config, ".agentix-plugin")), { code: "ENOENT" });
        }
        const calls = (await readFile(env.OPENCODE_TEST_CALLS, "utf8")).trim().split("\n").map(JSON.parse);
        assert.deepEqual(calls.map(c => c.args), Array(2).fill(Object.values(remotePackages)).flat().map(ref => ["plugin", "add", ref]));
        assert.ok(calls.every(c => c.config !== f.config && c.cwd !== root && c.customConfig === undefined && c.configContent === undefined));
        assert.equal(f.run("install", "local").status, 0);
        for (const name of Object.keys(remotePackages)) {
            const text = await readFile(join(f.config, name), "utf8");
            assert.doesNotMatch(text, /github:tenfyzhong/);
            assert.match(text, /file:\/\//);
        }
        assert.equal(f.runWithEnv(env, "install", "main").status, 0);
        assert.equal(f.run("remove").status, 0);
        for (const name of Object.keys(remotePackages))
            assert.deepEqual(JSON.parse((await readFile(join(f.config, name), "utf8")).replace("// preserved", "")).plugins, ["unrelated"]);
    });
}

for (const failure of ["first", "second", "publication"]) {
    for (const existing of [false, true]) {
        test(`opencode_remote_${failure}_failure_preserves_${existing ? "existing" : "empty"}_installation`, { skip: process.platform === "win32" }, async (t) => {
            const f = await fixture(t);
            if (existing) assert.equal(f.run("install", "local").status, 0);
            const before = await Promise.all(Object.keys(remotePackages).map(name => readFile(join(f.config, name), "utf8").catch(() => null)));
            const env = await remoteCLI(f);
            if (failure === "publication") {
                env.NODE_OPTIONS = `--import=${new URL("./fixtures/opencode-install-fault.mjs", import.meta.url).href}`;
                env.OPENCODE_TEST_FAIL_RENAME = join(f.config, "opencode.json");
            } else env.OPENCODE_TEST_FAIL_PACKAGE = Object.values(remotePackages)[failure === "first" ? 0 : 1];
            const result = f.runWithEnv(env, "install", "main");
            assert.notEqual(result.status, 0);
            assert.match(result.stderr, failure === "publication" ? /Fixture config publication failed/ : /Fixture native install failed/);
            assert.deepEqual(await Promise.all(Object.keys(remotePackages).map(name => readFile(join(f.config, name), "utf8").catch(() => null))), before);
            if (existing) await import(pathToFileURL(join(f.config, ".agentix-plugin/plugins/taskix-manager/index.js")));
            else await assert.rejects(stat(join(f.config, ".agentix-plugin")), { code: "ENOENT" });
        });
    }
}

for (const missing of [
    "plugins/taskix-manager/opencode/server.mjs",
    "plugins/agentix-bridge/opencode/tui.mjs",
    "plugins/agentix-bridge/opencode/turns.mjs",
]) {
    for (const existing of [false, true]) {
        test(`opencode_${existing ? "update" : "first_install"}_rejects_missing_${missing}`, async (t) => {
            const f = await fixture(t);
            if (existing) assert.equal(f.run("install", "local").status, 0);
            const paths = ["cli.json", "opencode.json"];
            const before = await Promise.all(paths.map(path => readFile(join(f.config, path), "utf8").catch(() => null)));
            const source = join(f.config, "source");
            for (const path of ["scripts", "plugins/taskix-manager", "plugins/agentix-bridge", "package.json"]) {
                await cp(join(root, path), join(source, path), { recursive: true, filter: path => !/[\\/](node_modules|tests)([\\/]|$)/.test(path) });
            }
            await rm(join(source, missing));
            const result = spawnSync(process.execPath, [join(source, "scripts/opencode-plugin.mjs"), "install", "local"], {
                env: { ...process.env, OPENCODE_CONFIG_DIR: f.config }, encoding: "utf8",
            });
            assert.notEqual(result.status, 0);
            assert.match(result.stderr, /Cannot load OpenCode plugins from SOURCE=local/);
            assert.deepEqual(await Promise.all(paths.map(path => readFile(join(f.config, path), "utf8").catch(() => null))), before);
            if (existing) await import(pathToFileURL(join(f.config, ".agentix-plugin/plugins/taskix-manager/index.js")));
            else await assert.rejects(stat(join(f.config, ".agentix-plugin")), { code: "ENOENT" });
        });
    }
}

test("native_opencode_environment_keeps_network_plumbing_and_discards_credentials", () => {
    const env = isolatedOpenCodeEnvironment(
        "/fixture/home",
        "/fixture/config",
        "/fixture/bin/taskix",
        {
            PATH: "/system/bin",
            HTTPS_PROXY: "http://proxy.invalid:8080",
            NODE_EXTRA_CA_CERTS: "/fixture/ca.pem",
            OPENAI_API_KEY: "must-not-leak",
            ANTHROPIC_API_KEY: "must-not-leak",
            OPENCODE_CONFIG: "/user/real-config",
            TASKIX_CONFIG: "/user/real-taskix",
            GITHUB_TOKEN: "must-not-leak",
        },
    );
    assert.equal(env.HOME, "/fixture/home");
    assert.equal(env.HTTPS_PROXY, "http://proxy.invalid:8080");
    assert.equal(env.NODE_EXTRA_CA_CERTS, "/fixture/ca.pem");
    for (const key of [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GITHUB_TOKEN",
        "OPENCODE_CONFIG",
        "TASKIX_CONFIG",
    ])
        assert.equal(env[key], undefined);
});

for (const both of [false, true]) {
    test(`opencode_ignores_cli_jsonc_${both ? 'alongside_json' : 'alone'}`, async (t) => {
        const f = await fixture(t);
        const inactive = '{"plugins":["inactive-plugin"]}';
        await writeFile(join(f.config, 'cli.jsonc'), inactive);
        if (both) await writeFile(join(f.config, 'cli.json'), '{"plugins":["active-plugin"]}');
        const result = f.run('install', 'local');
        assert.equal(result.status, 0, result.stderr);
        const active = JSON.parse(await readFile(join(f.config, 'cli.json'), 'utf8'));
        assert.ok(active.plugins.some(p => p.includes('plugins/agentix-bridge')));
        if (both) assert.ok(active.plugins.includes('active-plugin'));
        assert.equal(await readFile(join(f.config, 'cli.jsonc'), 'utf8'), inactive);
    });
}

test("opencode_remote_migration_preserves_jsonc_and_unowned_package_references", { skip: process.platform === "win32" }, async (t) => {
    const f = await fixture(t);
    const old = pathToFileURL(join(f.config, ".agentix-plugin/plugins/taskix-manager")).href;
    const foreign = "github:another/agentix#main::path:plugins/taskix-manager";
    await writeFile(join(f.config, "opencode.json"), JSON.stringify({ plugins: [old, foreign] }));
    await writeFile(join(f.config, "opencode.jsonc"), '{// keep jsonc\n"plugins":[' + JSON.stringify({package: remotePackages["opencode.json"]}) + ',],"theme":"dark",}\n', {mode: 0o600});
    const env = await remoteCLI(f);
    const result = f.runWithEnv(env, "install", "main");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(await readFile(join(f.config, "opencode.json"), "utf8")).plugins, [foreign]);
    const text = await readFile(join(f.config, "opencode.jsonc"), "utf8");
    assert.match(text, /keep jsonc/);
    assert.match(text, /"theme":"dark"/);
    assert.equal(text.split(remotePackages["opencode.json"]).length - 1, 1);
    assert.doesNotMatch(text, /file:/);
    assert.equal((await stat(join(f.config, "opencode.jsonc"))).mode & 0o777, 0o600);
});
