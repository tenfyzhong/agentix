import { test } from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import http from "node:http";
import { execFileSync } from "node:child_process";
import {
    mkdtempSync,
    mkdirSync,
    writeFileSync,
    realpathSync,
    readFileSync,
    readdirSync,
    chmodSync,
    existsSync,
    rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";

const enabled = process.env.AGENTIX_TEST_OPENCODE_NATIVE === "1";
const executable = (name) => {
    const candidate = (process.env.PATH ?? "")
        .split(delimiter)
        .map((directory) => join(directory, name))
        .find(existsSync);
    assert.ok(
        candidate,
        "Required native acceptance executable is unavailable: " + name,
    );
    return candidate;
};
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(fn, description, timeout = 30000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        const value = await fn();
        if (value) return value;
        await pause(50);
    }
    throw new Error("Timed out: " + description);
}

// Bun's native TUI can survive terminal SIGHUP. Snapshot descendants while
// their fixture-owned roots are alive, then terminate exactly that tree.
function terminateProcessTrees(roots) {
    const owned = new Set(
        roots.filter((pid) => Number.isInteger(pid) && pid > 1),
    );
    const pairs = execFileSync("ps", ["-Ao", "pid=,ppid="], {
        encoding: "utf8",
    })
        .trim()
        .split("\n")
        .map((line) => line.trim().split(/\s+/).map(Number));
    for (;;) {
        const before = owned.size;
        for (const [pid, parent] of pairs)
            if (owned.has(parent)) owned.add(pid);
        if (before === owned.size) break;
    }
    for (const pid of owned) {
        try {
            process.kill(pid, "SIGKILL");
        } catch (error) {
            if (error.code !== "ESRCH") throw error;
        }
    }
}

// Opt-in native acceptance uses an isolated terminal, fake provider, and a local control hub.
// No user OpenCode configuration, tokens, project, or database is inherited.
test(
    "opencode_native_terminal_streams_and_controls_original_session",
    { skip: !enabled, timeout: 180000 },
    async (t) => {
        const root = mkdtempSync(join(tmpdir(), "agentix-native-"));
        const terminal = "ax-" + process.pid + "-" + Date.now();
        const tmux =
            process.env.AGENTIX_TEST_TMUX_BIN ??
            (process.env.AGENTIX_TEST_TMUX &&
            process.env.AGENTIX_TEST_TMUX !== "1"
                ? process.env.AGENTIX_TEST_TMUX
                : executable("tmux"));
        const opencode =
            process.env.AGENTIX_TEST_OPENCODE ?? executable("opencode");
        const terminatePane = (name) => {
            try {
                const pid = Number(
                    execFileSync(
                        tmux,
                        [
                            "-L",
                            terminal,
                            "display-message",
                            "-p",
                            "-t",
                            name,
                            "#{pane_pid}",
                        ],
                        {
                            encoding: "utf8",
                            stdio: ["ignore", "pipe", "ignore"],
                        },
                    ).trim(),
                );
                terminateProcessTrees([pid]);
            } catch {}
            try {
                execFileSync(
                    tmux,
                    ["-L", terminal, "kill-session", "-t", name],
                    { stdio: "ignore" },
                );
            } catch {}
        };
        assert.match(
            execFileSync(opencode, ["--version"], { encoding: "utf8" }),
            /^opencode v2\./,
            "Native acceptance must use the V2 CLI",
        );
        const agentix =
            process.env.AGENTIX_TEST_AGENTIX ?? resolve("target/debug/agentix");
        assert.ok(
            existsSync(agentix),
            "Build agentix or provide AGENTIX_TEST_AGENTIX for the real workspace launcher",
        );
        const config = join(root, "config", "opencode");
        const permissionMarker = join(root, "permission-executions.log");
        mkdirSync(config, { recursive: true });
        mkdirSync(join(root, "project"));
        mkdirSync(join(root, "tmp"));
        const servicePort = net.createServer();
        await new Promise((resolve) =>
            servicePort.listen(0, "127.0.0.1", resolve),
        );
        writeFileSync(
            join(config, "service.json"),
            JSON.stringify({
                hostname: "127.0.0.1",
                port: servicePort.address().port,
            }),
        );
        await new Promise((resolve) => servicePort.close(resolve));
        const requests = [],
            events = [],
            registrations = [],
            peers = new Set(),
            pending = new Map();
        const completions = [];
        let sequence = 0,
            overflowReturned = false,
            serviceEnvironment;
        const model = http.createServer(async (req, res) => {
            let body = "";
            for await (const chunk of req) body += chunk;
            const input = JSON.parse(body);
            requests.push(input);
            if (
                input.stream &&
                input.tools?.length &&
                JSON.stringify(input.messages).includes("bridge-overflow") &&
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
            if (!input.stream) {
                res.writeHead(200, { "content-type": "application/json" });
                res.end(
                    JSON.stringify({
                        id: "chat_test",
                        object: "chat.completion",
                        created: 1,
                        model: "fixture",
                        choices: [
                            {
                                index: 0,
                                message: {
                                    role: "assistant",
                                    content: "Native fixture",
                                },
                                finish_reason: "stop",
                            },
                        ],
                        usage: {
                            prompt_tokens: 10,
                            completion_tokens: 2,
                            total_tokens: 12,
                        },
                    }),
                );
                return;
            }
            res.writeHead(200, { "content-type": "text/event-stream" });
            const lastUser = input.messages
                ?.filter((m) => m.role === "user")
                .at(-1);
            const text =
                typeof lastUser?.content === "string"
                    ? lastUser.content
                    : JSON.stringify(lastUser?.content);
            const alreadyCalled = input.messages?.at(-1)?.role === "tool";
            const scripted = (input.tools?.length ?? 0) > 0;
            const summarizing =
                text?.startsWith("You MUST summarize the conversation above") ||
                text?.startsWith(
                    "The previous response did not fill in the required summary template",
                );
            const send = (delta, finish_reason = null) =>
                res.write(
                    "data: " +
                        JSON.stringify({
                            id: "chat_test",
                            object: "chat.completion.chunk",
                            created: 1,
                            model: "fixture",
                            choices: [{ index: 0, delta, finish_reason }],
                        }) +
                        "\n\n",
                );
            const call = (name, args, id) => {
                const direct = input.tools.some(
                    (t) => t.function.name === name,
                );
                const available = input.tools.find((t) =>
                    ["execute", "run_code"].includes(t.function.name),
                );
                assert.ok(
                    direct || available,
                    "Native tool names: " +
                        input.tools.map((t) => t.function.name).join(","),
                );
                const invocation = direct
                    ? { name, arguments: JSON.stringify(args) }
                    : {
                          name: available.function.name,
                          arguments: JSON.stringify({
                              code: `console.log(await tools.${name}(${JSON.stringify(args)}));`,
                          }),
                      };
                send(
                    {
                        role: "assistant",
                        tool_calls: [
                            {
                                index: 0,
                                id: id + "_" + requests.length,
                                type: "function",
                                function: invocation,
                            },
                        ],
                    },
                    "tool_calls",
                );
            };
            if (
                scripted &&
                text?.includes("bridge-permission") &&
                !alreadyCalled
            ) {
                call(
                    "shell",
                    {
                        command:
                            "printf 'native-permission\\n' >> " +
                            quote(permissionMarker),
                    },
                    "call_shell",
                );
                res.end("data: [DONE]\n\n");
                return;
            }
            if (
                scripted &&
                text?.includes("bridge-question") &&
                !alreadyCalled
            ) {
                call(
                    "question",
                    {
                        questions: [
                            {
                                question: "Choose native options",
                                header: "Native",
                                options: [
                                    { label: "Alpha", description: "First" },
                                    { label: "Beta", description: "Second" },
                                ],
                                multiple: true,
                            },
                        ],
                    },
                    "call_question",
                );
                res.end("data: [DONE]\n\n");
                return;
            }
            if (scripted && text?.includes("bridge-taskix") && !alreadyCalled) {
                assert.ok(
                    input.tools.some((t) => t.function.name === "taskix"),
                    "Native Taskix tool must bypass CodeMode to retain per-call identity",
                );
                send(
                    {
                        role: "assistant",
                        tool_calls: [
                            {
                                index: 0,
                                id: "call_taskix_first",
                                type: "function",
                                function: {
                                    name: "taskix",
                                    arguments: JSON.stringify({
                                        args: [
                                            "job",
                                            "create",
                                            "--title",
                                            "Native fixture",
                                        ],
                                    }),
                                },
                            },
                            {
                                index: 1,
                                id: "call_taskix_second",
                                type: "function",
                                function: {
                                    name: "taskix",
                                    arguments: JSON.stringify({
                                        args: [
                                            "job",
                                            "create",
                                            "--title",
                                            "Native fixture second",
                                        ],
                                    }),
                                },
                            },
                        ],
                    },
                    "tool_calls",
                );
                res.end("data: [DONE]\n\n");
                return;
            }
            if (scripted && text?.includes("bridge-slow")) {
                send({ role: "assistant", content: "Waiting" });
                res.once("close", () => {});
                return;
            }
            const summary =
                "## Objective\n- Complete native acceptance.\n\n## Requirements\n- Preserve the native session.\n\n## Decisions\n- Use the fixture provider.\n\n## Work State\n### Completed\n- Earlier native turns.\n### Active\n- Continue the current user request.\n### Blocked\n- (none)\n\n## Next Move\n1. Continue the current user request.\n\n## Relevant Files\n- (none)\n\n## Important Context\n- Native fixture acceptance.";
            send({
                role: "assistant",
                content: summarizing ? summary : "Native ",
            });
            await pause(100);
            if (!summarizing) send({ content: "fixture answer" });
            send({}, "stop");
            res.end("data: [DONE]\n\n");
        });
        await new Promise((resolve) => model.listen(0, "127.0.0.1", resolve));
        const hub = net.createServer((socket) => {
            peers.add(socket);
            socket.on("close", () => peers.delete(socket));
            let buffer = "";
            socket.on("data", (data) => {
                buffer += data;
                while (buffer.includes("\n")) {
                    const end = buffer.indexOf("\n"),
                        line = buffer.slice(0, end);
                    buffer = buffer.slice(end + 1);
                    const frame = JSON.parse(line);
                    if (frame.method === "register") {
                        registrations.push({ socket, params: frame.params });
                        socket.write(
                            JSON.stringify({
                                id: "register",
                                ok: true,
                                result: {},
                            }) + "\n",
                        );
                    } else if (frame.event) {
                        events.push(frame.event);
                        if (frame.event.TurnCompleted)
                            completions.push(frame.event.TurnCompleted);
                    } else if (pending.has(frame.id)) {
                        pending.get(frame.id)(frame);
                        pending.delete(frame.id);
                    }
                }
            });
        });
        const endpoint = join(root, "control.sock");
        await new Promise((resolve) => hub.listen(endpoint, resolve));
        t.after(async () => {
            try {
                const roots = execFileSync(
                    tmux,
                    ["-L", terminal, "list-panes", "-a", "-F", "#{pane_pid}"],
                    { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
                )
                    .trim()
                    .split("\n")
                    .map(Number);
                terminateProcessTrees(roots);
            } catch {}
            try {
                execFileSync(tmux, ["-L", terminal, "kill-server"], {
                    stdio: "ignore",
                });
            } catch {}
            if (serviceEnvironment) {
                try {
                    execFileSync(opencode, ["service", "stop"], {
                        env: serviceEnvironment,
                        stdio: "ignore",
                        timeout: 15000,
                    });
                } catch {}
            }
            for (const peer of peers) peer.destroy();
            await new Promise((resolve) => hub.close(resolve));
            model.closeAllConnections();
            await new Promise((resolve) => model.close(resolve));
            rmSync(root, { recursive: true, force: true });
        });

        writeFileSync(
            join(config, "opencode.json"),
            JSON.stringify({
                model: { providerID: "fixture", model: "fixture" },
                agents: {
                    title: {
                        model: { providerID: "fixture", model: "fixture" },
                    },
                },
                permissions: [
                    { action: "shell", resource: "*", effect: "ask" },
                    { action: "question", resource: "*", effect: "allow" },
                ],
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
                        package: "aisdk:@ai-sdk/openai-compatible",
                        name: "Fixture",
                        settings: {
                            baseURL: `http://127.0.0.1:${model.address().port}/v1`,
                            apiKey: "isolated-fixture",
                        },
                        models: {
                            fixture: {
                                name: "Fixture",
                                limit: { context: 32768, output: 4096 },
                            },
                        },
                    },
                },
            }),
        );
        const env = {
            PATH: [
                join(root, "bin"),
                dirname(process.execPath),
                dirname(tmux),
                dirname(opencode),
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
            ].join(delimiter),
            HOME: root,
            TMPDIR: join(root, "tmp"),
            XDG_CONFIG_HOME: join(root, "config"),
            XDG_DATA_HOME: join(root, "data"),
            XDG_STATE_HOME: join(root, "state"),
            XDG_CACHE_HOME: join(root, "cache"),
            TERM: "xterm-256color",
            LANG: "en_US.UTF-8",
            AGENTIX_CONTROL_ENDPOINT: "unix://" + endpoint,
            AGENTIX_OPENCODE_STATE_DIR: join(root, "bridge"),
            OPENCODE_DISABLE_MODELS_FETCH: "true",
            OPENCODE_DISABLE_AUTOUPDATE: "true",
        };
        for (const key of [
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "all_proxy",
            "ALL_PROXY",
            "no_proxy",
            "NO_PROXY",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ])
            if (process.env[key]) env[key] = process.env[key];
        if (process.env.AGENTIX_TEST_NPM_CACHE)
            env.npm_config_cache = process.env.AGENTIX_TEST_NPM_CACHE;
        mkdirSync(join(root, "bin"));
        const taskixLog = join(root, "taskix.jsonl");
        writeFileSync(
            join(root, "bin", "taskix"),
            `#!/usr/bin/env node
const fs=require('node:fs');
const args=process.argv.slice(2);
const file=args.indexOf('--file');
const capture=file<0?undefined:JSON.parse(fs.readFileSync(args[file+1],'utf8'));
fs.appendFileSync(process.env.AGENTIX_TEST_TASKIX_LOG,JSON.stringify({args,capture})+'\\n');
process.stdout.write(JSON.stringify({schema_version:1,ok:true,result:{}})+'\\n');
`,
        );
        chmodSync(join(root, "bin", "taskix"), 0o700);
        env.AGENTIX_TEST_TASKIX_LOG = taskixLog;
        serviceEnvironment = { ...env };
        execFileSync(
            process.execPath,
            ["scripts/opencode-plugin.mjs", "install", "local"],
            {
                env: { ...process.env, ...env, OPENCODE_CONFIG_DIR: config },
                stdio: "pipe",
            },
        );
        const taskixCalls = () =>
            existsSync(taskixLog)
                ? readFileSync(taskixLog, "utf8")
                      .trim()
                      .split("\n")
                      .filter(Boolean)
                      .map((line) => JSON.parse(line))
                : [];
        // Deliberately remove bridge variables: the production launcher must restore them.
        const launchEnv = Object.fromEntries(
            Object.entries(env).filter(
                ([key]) =>
                    ![
                        "AGENTIX_CONTROL_ENDPOINT",
                        "AGENTIX_OPENCODE_STATE_DIR",
                    ].includes(key),
            ),
        );
        const launch = [
            "env",
            "-i",
            ...Object.entries(launchEnv).map(([k, v]) => k + "=" + v),
            agentix,
            "launch-open-code",
            "--command",
            opencode,
            "--state-dir",
            env.AGENTIX_OPENCODE_STATE_DIR,
            "--endpoint",
            env.AGENTIX_CONTROL_ENDPOINT,
        ];
        const resumeLaunch = [
            "env",
            "-i",
            ...Object.entries(env).map(([k, v]) => k + "=" + v),
            opencode,
            "--standalone",
            "--print-logs",
            "--log-level",
            "debug",
        ];
        const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
        execFileSync(
            tmux,
            [
                "-L",
                terminal,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "native",
                "-x",
                "140",
                "-y",
                "40",
                "-c",
                join(root, "project"),
                launch.map(quote).join(" "),
            ],
            { env: { PATH: env.PATH, HOME: root } },
        );
        let registration;
        try {
            registration = await until(
                () => registrations.at(-1),
                "native registration",
            );
        } catch (error) {
            const screen = execFileSync(
                tmux,
                ["-L", terminal, "capture-pane", "-pt", "native", "-S", "-100"],
                { encoding: "utf8" },
            );
            const logDirectory = join(root, "data", "opencode", "log");
            const logs = existsSync(logDirectory)
                ? readdirSync(logDirectory)
                      .map((file) =>
                          readFileSync(join(logDirectory, file), "utf8")
                              .split("\n")
                              .filter((line) =>
                                  /plugin|agentix|error|warn|session.create/i.test(
                                      line,
                                  ),
                              )
                              .join("\n"),
                      )
                      .join("\n")
                      .slice(-25000)
                : "";
            const installed = ["cli.json", "opencode.json"]
                .filter((name) => existsSync(join(config, name)))
                .map(
                    (name) =>
                        name + ": " + readFileSync(join(config, name), "utf8"),
                )
                .join("\n");
            throw new Error(
                error.message + "\n" + screen + "\n" + installed + "\n" + logs,
            );
        }
        const rpc = async (
            method,
            params = {},
            target = registrations.findLast((r) => !r.socket.destroyed),
            expectedOk = true,
        ) => {
            const id = "native-" + ++sequence;
            const response = new Promise((resolve) => pending.set(id, resolve));
            target.socket.write(JSON.stringify({ id, method, params }) + "\n");
            let timer;
            const frame = await Promise.race([
                response,
                new Promise((_, reject) => {
                    timer = setTimeout(
                        () => reject(new Error("RPC timed out: " + method)),
                        15000,
                    );
                }),
            ]).finally(() => clearTimeout(timer));
            assert.equal(
                frame.ok,
                expectedOk,
                JSON.stringify({ frame, events: events.slice(-15) }),
            );
            return frame.ok ? frame.result : frame;
        };
        assert.equal(registration.params.agent, "opencode");
        const panePid = Number(
            execFileSync(
                tmux,
                [
                    "-L",
                    terminal,
                    "display-message",
                    "-p",
                    "-t",
                    "native",
                    "#{pane_pid}",
                ],
                { encoding: "utf8" },
            ).trim(),
        );
        let ancestor = registration.params.pid;
        for (let depth = 0; depth < 8 && ancestor !== panePid; depth++)
            ancestor = Number(
                execFileSync("ps", ["-p", String(ancestor), "-o", "ppid="], {
                    encoding: "utf8",
                }).trim(),
            );
        assert.equal(
            ancestor,
            panePid,
            "Bridge belongs to the original terminal process tree",
        );
        assert.equal(
            registration.params.cwd,
            realpathSync(join(root, "project")),
        );
        assert.ok(
            registration.params.session_file.startsWith(
                env.AGENTIX_OPENCODE_STATE_DIR,
            ),
            "Production launcher supplies durable state directory",
        );
        const initial = registration.params.session_id;
        const reply = await rpc("prompt", {
            request_id: "native-first",
            text: "Answer with native fixture answer.",
        });
        await until(
            () => completions.find((c) => c.turn_id === reply.turn_id),
            "native turn completion",
        );
        assert.ok(requests.length > 0);
        assert.ok(
            events.some((e) => e.AgentMessageDelta?.delta.includes("Native")),
        );
        const history = await rpc("history", { limit: 20 });
        assert.equal(history.turns.at(-1).id, reply.turn_id);
        assert.equal(history.turns.at(-1).agent_text, "Native fixture answer");
        await until(
            () =>
                taskixCalls().some((c) =>
                    c.capture?.messages.some(
                        (m) =>
                            m.id ===
                            `${reply.turn_id}:${reply.turn_id}:assistant`,
                    ),
                ),
            "Taskix native capture",
        ).catch((error) => {
            throw new Error(
                error.message + "\n" + JSON.stringify(taskixCalls()),
            );
        });
        const captured = taskixCalls().flatMap(
            (c) => c.capture?.messages ?? [],
        );
        assert.ok(
            captured.some(
                (m) => m.id === `${reply.turn_id}:${reply.turn_id}:user`,
            ),
        );
        assert.equal(
            captured.find(
                (m) => m.id === `${reply.turn_id}:${reply.turn_id}:assistant`,
            ).text,
            "Native fixture answer",
        );
        assert.ok(
            requests.some((r) =>
                r.messages.some(
                    (m) =>
                        m.role === "system" &&
                        m.content.includes("Task session: " + initial),
                ),
            ),
        );
        await rpc("command", { name: "rename", value: "Native acceptance" });
        assert.equal((await rpc("info")).session.name, "Native acceptance");
        const models = await rpc("command", { name: "model" });
        assert.ok(models.choices.some((m) => m.value === "fixture/fixture"));
        await rpc("command", { name: "model", value: "fixture/fixture" });
        await rpc("command", { name: "compact" });
        assert.equal(
            (await rpc("history")).turns.at(-1).agent_text,
            "Native fixture answer",
        );
        await rpc("command", { name: "diff" });
        await rpc("command", { name: "skills" });
        const terminalTurns = completions.length;
        execFileSync(tmux, [
            "-L",
            terminal,
            "send-keys",
            "-t",
            "native",
            "-l",
            "Native terminal prompt",
        ]);
        execFileSync(tmux, [
            "-L",
            terminal,
            "send-keys",
            "-t",
            "native",
            "Enter",
        ]);
        await until(
            () => completions.length > terminalTurns,
            "native terminal prompt",
        );
        const terminalTurn = completions.at(-1).turn_id;
        assert.notEqual(terminalTurn, reply.turn_id);
        assert.match(
            (await rpc("history")).turns.at(-1).user_text,
            /Native terminal prompt/,
        );
        await until(
            async () => (await rpc("info")).session.status === "idle",
            "terminal idle",
        );
        const slow = await rpc("prompt", {
            request_id: "native-slow",
            text: "bridge-slow",
        });
        await until(
            () =>
                events.find(
                    (e) => e.AgentMessageDelta?.turn_id === slow.turn_id,
                ),
            "slow stream start",
        );
        await rpc("queue", { request_id: "native-queued", text: "After slow" });
        await rpc("stop");
        await until(
            () => completions.find((c) => c.turn_id === slow.turn_id),
            "stopped turn",
        );
        assert.equal(
            completions.find((c) => c.turn_id === slow.turn_id).status,
            "interrupted",
        );
        await until(
            async () => (await rpc("info")).session.status === "idle",
            "abort idle",
        );
        const queue = await rpc("queue_state");
        assert.equal(queue.paused, true);
        assert.equal(queue.items.length, 1);
        const beforeQueued = completions.length;
        await rpc("queue_resume");
        await until(
            () => completions.length > beforeQueued,
            "native queue delivery",
        );
        assert.equal(
            (await rpc("history")).turns.at(-1).user_text,
            "After slow",
        );
        assert.ok(events.some((e) => e.QueueChanged));
        await until(
            async () => (await rpc("info")).session.status === "idle",
            "queue idle",
        );
        await rpc("prompt", {
            request_id: "native-permission",
            text: "bridge-permission",
        });
        const permission = await until(
            () =>
                events.find(
                    (e) =>
                        e.InteractionRequested?.method ===
                        "opencode.permission",
                )?.InteractionRequested,
            "native permission",
        );
        assert.deepEqual(permission.available_decisions, [
            "once",
            "always",
            "reject",
        ]);
        const beforeReconnect = registrations.length;
        const beforeReplay = events.length;
        registrations.at(-1).socket.destroy();
        await until(
            () => registrations.length > beforeReconnect,
            "bridge reconnect",
        );
        await until(
            () =>
                events
                    .slice(beforeReplay)
                    .some(
                        (e) =>
                            e.InteractionRequested?.rpc_id ===
                            permission.rpc_id,
                    ),
            "pending permission replay after Hub reconnect",
        );
        await rpc("interaction", {
            rpc_id: permission.rpc_id,
            response: { decision: "once" },
        });
        await until(
            () => completions.some((c) => c.turn_id === permission.turn_id),
            "permitted turn",
        );
        await until(
            async () => (await rpc("info")).session.status === "idle",
            "permission idle",
        );
        const stale = await rpc(
            "interaction",
            { rpc_id: permission.rpc_id, response: { decision: "once" } },
            undefined,
            false,
        );
        assert.equal(
            stale.code,
            "invalid_request",
            "Already resolved native approval cannot be submitted again",
        );
        await rpc("prompt", {
            request_id: "native-question",
            text: "bridge-question",
        });
        const question = await until(
            () =>
                events.find(
                    (e) => e.InteractionRequested?.method === "opencode.form",
                )?.InteractionRequested,
            "native question",
        );
        assert.equal(question.payload.questions[0].multiple, true);
        assert.equal(question.payload.questions[0].id, "q0");
        // The native question tool enables custom input; explicit false is covered by the SDK fixture.
        await rpc("interaction", {
            rpc_id: question.rpc_id,
            response: { answers: { q0: { answers: ["Alpha", "Beta"] } } },
        });
        await until(
            () => completions.some((c) => c.turn_id === question.turn_id),
            "answered turn",
        );
        await until(
            async () => (await rpc("info")).session.status === "idle",
            "question idle",
        );
        const rejectedQuestion = await rpc("prompt", {
            request_id: "native-question-reject",
            text: "bridge-question reject",
        });
        const rejectedForm = await until(
            () =>
                events.find(
                    (e) =>
                        e.InteractionRequested?.turn_id ===
                            rejectedQuestion.turn_id &&
                        e.InteractionRequested.method === "opencode.form",
                )?.InteractionRequested,
            "native rejectable form",
        );
        await rpc("interaction", {
            rpc_id: rejectedForm.rpc_id,
            response: { decision: "reject" },
        });
        await until(
            () =>
                completions.some((c) => c.turn_id === rejectedQuestion.turn_id),
            "rejected form assistant completion",
        );
        assert.equal(
            completions.find((c) => c.turn_id === rejectedQuestion.turn_id)
                .status,
            "completed",
        );
        const retiredForm = await rpc(
            "interaction",
            { rpc_id: rejectedForm.rpc_id, response: { decision: "reject" } },
            undefined,
            false,
        );
        assert.equal(retiredForm.code, "invalid_request");
        const afterRejectedForm = await rpc("prompt", {
            request_id: "after-form-rejection",
            text: "After form rejection",
        });
        await until(
            () =>
                completions.some(
                    (c) => c.turn_id === afterRejectedForm.turn_id,
                ),
            "prompt after form rejection",
        );
        assert.equal(
            completions.find((c) => c.turn_id === afterRejectedForm.turn_id)
                .status,
            "completed",
        );

        const taskixTurn = await rpc("prompt", {
            request_id: "native-taskix",
            text: "bridge-taskix",
        });
        await until(
            () => completions.some((c) => c.turn_id === taskixTurn.turn_id),
            "Taskix tool completion",
        );
        const write = taskixCalls().find((c) =>
            c.args.includes("Native fixture"),
        );
        assert.ok(write, "Native Taskix tool reached the fake CLI");
        assert.equal(write.args[write.args.indexOf("--session") + 1], initial);
        assert.equal(
            write.args[write.args.indexOf("--executor") + 1],
            "agent:opencode:" + initial,
        );
        assert.match(
            write.args[write.args.indexOf("--idempotency-key") + 1],
            new RegExp("^opencode:" + initial + ":"),
        );
        const secondWrite = taskixCalls().find((c) =>
            c.args.includes("Native fixture second"),
        );
        assert.ok(
            secondWrite,
            "Second Taskix call in the same native turn reached the CLI",
        );
        assert.notEqual(
            write.args[write.args.indexOf("--idempotency-key") + 1],
            secondWrite.args[secondWrite.args.indexOf("--idempotency-key") + 1],
            "Distinct direct Taskix calls require distinct native idempotency keys",
        );
        await until(
            async () => (await rpc("info")).session.status === "idle",
            "Taskix idle",
        );
        const overflow = await rpc("prompt", {
            request_id: "native-overflow",
            text: "bridge-overflow",
        });
        await until(
            () => completions.some((c) => c.turn_id === overflow.turn_id),
            "automatic compaction recovery",
            45000,
        );
        assert.equal(
            overflowReturned,
            true,
            "Native prompt exercised the provider context overflow path",
        );
        assert.equal(
            completions.find((c) => c.turn_id === overflow.turn_id).status,
            "completed",
            JSON.stringify({
                events: events.slice(-30),
                requests: requests.slice(-5).map((r) => ({
                    stream: r.stream,
                    tools: r.tools?.map((t) => t.function.name),
                    lastUser: r.messages
                        ?.filter((m) => m.role === "user")
                        .at(-1)?.content,
                })),
            }),
        );
        assert.equal(
            events.filter((e) => e.TurnStarted?.turn_id === overflow.turn_id)
                .length,
            1,
        );
        assert.equal((await rpc("history")).turns.at(-1).id, overflow.turn_id);
        await until(
            () =>
                taskixCalls().some((c) =>
                    c.capture?.messages.some(
                        (m) =>
                            m.id ===
                            `${overflow.turn_id}:${overflow.turn_id}:assistant`,
                    ),
                ),
            "Taskix recovered capture",
        );
        const recovered = taskixCalls()
            .flatMap((c) => c.capture?.messages ?? [])
            .find(
                (m) =>
                    m.id ===
                    `${overflow.turn_id}:${overflow.turn_id}:assistant`,
            );
        assert.equal(
            recovered.text,
            "Native fixture answer",
            JSON.stringify({
                events: events.slice(-25),
                requests: requests.slice(-5).map((r) => ({
                    system: r.messages
                        ?.find((m) => m.role === "system")
                        ?.content?.slice(0, 100),
                    lastUser: JSON.stringify(
                        r.messages?.filter((m) => m.role === "user").at(-1)
                            ?.content,
                    )?.slice(0, 600),
                })),
            }),
        );
        // A normal existing terminal uses V2's background service, while the first
        // terminal uses the IM launcher's private server. Both retain their own TUI
        // routing; this service belongs exclusively to this isolated HOME/XDG.
        const existingTerminalLaunch = [
            "env",
            "-i",
            ...Object.entries(env).map(([k, v]) => k + "=" + v),
            opencode,
            "--print-logs",
            "--log-level",
            "debug",
        ];
        // Two independent native terminals in the same directory must never share routing.
        const beforeSecond = registrations.length;
        execFileSync(
            tmux,
            [
                "-L",
                terminal,
                "new-session",
                "-d",
                "-s",
                "second",
                "-x",
                "140",
                "-y",
                "40",
                "-c",
                join(root, "project"),
                existingTerminalLaunch.map(quote).join(" "),
            ],
            { env: { PATH: env.PATH, HOME: root } },
        );
        const second = await until(
            () =>
                registrations
                    .slice(beforeSecond)
                    .find((r) => r.params.session_id !== initial),
            "second native terminal",
        );
        const primary = registrations.findLast(
            (r) => r.params.session_id === initial,
        );
        assert.equal(second.params.cwd, primary.params.cwd);
        assert.notEqual(second.params.client_id, primary.params.client_id);
        assert.notEqual(second.params.pid, primary.params.pid);
        await rpc("stop", {}, primary);
        await rpc(
            "queue",
            {
                request_id: "primary-isolated-queue",
                text: "Primary terminal only",
            },
            primary,
        );
        assert.equal((await rpc("queue_state", {}, second)).items.length, 0);
        const otherPermission = await rpc(
            "prompt",
            { request_id: "second-permission", text: "bridge-permission" },
            second,
        );
        const isolatedPermission = await until(
            () =>
                events.find(
                    (e) =>
                        e.InteractionRequested?.turn_id ===
                        otherPermission.turn_id,
                )?.InteractionRequested,
            "second terminal permission",
        );
        // Sending the correct opaque request to the wrong terminal must not resolve it.
        const rejected = await rpc(
            "interaction",
            {
                rpc_id: isolatedPermission.rpc_id,
                response: { decision: "reject" },
            },
            primary,
            false,
        );
        assert.equal(rejected.code, "invalid_request");
        assert.equal(
            (await rpc("queue_state", {}, primary)).items[0].id,
            "primary-isolated-queue",
        );
        await rpc(
            "interaction",
            {
                rpc_id: isolatedPermission.rpc_id,
                response: { decision: "reject" },
            },
            second,
        );
        await until(
            () =>
                completions.some((c) => c.turn_id === otherPermission.turn_id),
            "second terminal rejection",
        );

        assert.equal(
            completions.find((c) => c.turn_id === otherPermission.turn_id)
                .status,
            "completed",
        );
        assert.equal(
            readFileSync(permissionMarker, "utf8"),
            "native-permission\n",
            "Rejected native shell must not execute; only the earlier once-approved command wrote the marker",
        );
        const afterPermissionReject = await rpc(
            "prompt",
            {
                request_id: "after-permission-reject",
                text: "After permission rejection",
            },
            second,
        );
        await until(
            () =>
                completions.some(
                    (c) => c.turn_id === afterPermissionReject.turn_id,
                ),
            "next prompt after permission rejection",
        );
        assert.equal(
            completions.find((c) => c.turn_id === afterPermissionReject.turn_id)
                .status,
            "completed",
        );
        assert.ok(
            !(await rpc("history", {}, primary)).turns.some(
                (t) => t.id === otherPermission.turn_id,
            ),
        );
        assert.ok(
            !(await rpc("history", {}, second)).turns.some(
                (t) => t.id === overflow.turn_id,
            ),
        );
        await rpc("queue_clear", {}, primary);
        terminatePane("second");
        await until(() => second.socket.destroyed, "second terminal shutdown");
        assert.equal(
            (await rpc("command", { name: "new" })).body,
            "Session switch requested",
        );
        await until(
            () =>
                registrations.some(
                    (r) => r.params.previous_session_id === initial,
                ),
            "new session replacement",
        );
        assert.notEqual(registrations.at(-1).params.session_id, initial);
        const client = registrations.at(-1).params.client_id;
        const beforeResume = registrations.length;
        terminatePane("native");
        execFileSync(
            tmux,
            [
                "-L",
                terminal,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "native",
                "-x",
                "140",
                "-y",
                "40",
                "-c",
                join(root, "project"),
                [...resumeLaunch, "--session", initial].map(quote).join(" "),
            ],
            { env: { PATH: env.PATH, HOME: root } },
        );
        await until(
            () => registrations.length > beforeResume,
            "native resume registration",
        );
        assert.equal(registrations.at(-1).params.session_id, initial);
        assert.equal(
            registrations.at(-1).params.previous_session_id,
            undefined,
        );
        assert.notEqual(registrations.at(-1).params.client_id, client);
        assert.ok(
            (await rpc("history")).turns.some((t) => t.id === reply.turn_id),
        );
        const paged = new Set();
        let cursor;
        for (let page = 0; page < 20; page++) {
            const history = await rpc("history", {
                limit: 1,
                ...(cursor ? { cursor } : {}),
            });
            for (const turn of history.turns) {
                assert.ok(
                    !paged.has(turn.id),
                    "Native history pages must advance",
                );
                paged.add(turn.id);
            }
            if (!history.older_cursor) break;
            cursor = history.older_cursor;
        }
        for (const id of [
            reply.turn_id,
            terminalTurn,
            slow.turn_id,
            taskixTurn.turn_id,
            overflow.turn_id,
        ])
            assert.ok(paged.has(id), "Native pagination retains " + id);
        // SIGKILL during a delivery must preserve uncertainty instead of silently replaying it.
        const inflight = await rpc("prompt", {
            request_id: "native-crash-inflight",
            text: "bridge-slow crash recovery",
        });
        await until(
            () =>
                events.some(
                    (e) => e.AgentMessageDelta?.turn_id === inflight.turn_id,
                ),
            "inflight stream before SIGKILL",
        );
        await rpc("queue", {
            request_id: "native-crash-waiting",
            text: "Never automatically replay after crash",
        });
        const killed = registrations.at(-1);
        terminateProcessTrees([killed.params.pid]);
        await until(() => killed.socket.destroyed, "native process killed");
        terminatePane("native");
        const beforeCrashResume = registrations.length;
        const requestsBeforeResume = requests.length;
        execFileSync(
            tmux,
            [
                "-L",
                terminal,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "native",
                "-x",
                "140",
                "-y",
                "40",
                "-c",
                join(root, "project"),
                [...resumeLaunch, "--session", initial].map(quote).join(" "),
            ],
            { env: { PATH: env.PATH, HOME: root } },
        );
        await until(
            () => registrations.length > beforeCrashResume,
            "crash resume registration",
        );
        const restored = await rpc("queue_state");
        assert.equal(restored.paused, true);
        assert.equal(restored.uncertain.id, "native-crash-inflight");
        assert.equal(restored.items[0].id, "native-crash-waiting");
        await pause(750);
        assert.equal(
            requests.length,
            requestsBeforeResume,
            "Crash recovery does not dispatch a new model request",
        );
        const refused = await rpc("queue_resume", {}, undefined, false);
        assert.equal(
            refused.code,
            "delivery_uncertain",
            "Explicit resume requires uncertainty resolution",
        );
        await rpc("queue_clear");
        assert.equal((await rpc("queue_state")).uncertain, null);
        assert.equal((await rpc("queue_state")).items.length, 0);
    },
);
