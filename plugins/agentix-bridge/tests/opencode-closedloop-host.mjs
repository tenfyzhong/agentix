// Native host and deterministic model fixture for the Rust production IM stack.
import http from "node:http";
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync, appendFileSync, chmodSync } from "node:fs";
import { join, resolve } from "node:path";
import { isolatedOpenCodeEnvironment } from "../../taskix-manager/tests/fixtures/opencode-headless.mjs";
const [root, endpoint] = process.argv.slice(2);
const taskix =
    process.env.AGENTIX_TEST_TASKIX ??
    resolve(
        execFileSync("git", ["rev-parse", "--git-common-dir"], {
            encoding: "utf8",
        }).trim(),
        "../target/debug/taskix",
    );
const agentix =
    process.env.AGENTIX_TEST_AGENTIX ??
    resolve(
        execFileSync("git", ["rev-parse", "--git-common-dir"], {
            encoding: "utf8",
        }).trim(),
        "../target/debug/agentix",
    );
const config = join(root, "config/opencode");
mkdirSync(config, { recursive: true });
mkdirSync(join(root, "project"), { recursive: true });
const server = http.createServer(async (req, res) => {
    let body = "";
    for await (const chunk of req) body += chunk;
    const input = JSON.parse(body);
    writeFileSync(join(root, "provider.json"), JSON.stringify(input));
    appendFileSync(join(root, "provider.jsonl"), JSON.stringify(input) + "\n");
    if (!input.stream) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
            JSON.stringify({
                id: "fixture",
                object: "chat.completion",
                choices: [
                    {
                        index: 0,
                        message: { role: "assistant", content: "Fixture" },
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
    const send = (delta, finish_reason = null) =>
        res.write(
            "data: " +
                JSON.stringify({
                    id: "fixture",
                    object: "chat.completion.chunk",
                    created: 1,
                    model: "fixture",
                    choices: [{ index: 0, delta, finish_reason }],
                }) +
                "\n\n",
        );
    const call = (name, args, id) => {
        const direct = input.tools.some((tool) => tool.function.name === name);
        const invocation = direct
            ? { name, arguments: JSON.stringify(args) }
            : {
                  name: input.tools.find((tool) =>
                      ["execute", "run_code"].includes(tool.function.name),
                  ).function.name,
                  arguments: JSON.stringify({
                      code: `console.log(await tools.${name}(${JSON.stringify(args)}));`,
                  }),
              };
        send(
            {
                role: "assistant",
                tool_calls: [
                    { index: 0, id, type: "function", function: invocation },
                ],
            },
            "tool_calls",
        );
    };
    const user = input.messages.filter((m) => m.role === "user").at(-1);
    const text = JSON.stringify(user?.content);
    const done = input.messages.at(-1)?.role === "tool";
    if (!done && input.tools?.length && text.includes("permission-loop"))
        call(
            "shell",
            {
                command: "printf closedloop-permission",
            },
            "call_permission",
        );
    else if (!done && input.tools?.length && text.includes("taskix-loop"))
        call("taskix", { args: ["project", "list"] }, "call_taskix");
    else if (!done && input.tools?.length && text.includes("question-loop"))
        call(
            "question",
            {
                questions: [
                    {
                        question: "Choose closed loop features",
                        header: "Features",
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
    else {
        send({ role: "assistant", content: "Closed loop " });
        await new Promise((r) => setTimeout(r, 150));
        send({ content: done ? "tool answer" : "plain answer" });
        send({}, "stop");
    }
    res.end("data: [DONE]\n\n");
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
writeFileSync(
    join(config, "opencode.json"),
    JSON.stringify({
        model: { providerID: "fixture", model: "fixture" },
        agents: {
            title: { model: { providerID: "fixture", model: "fixture" } },
        },
        permissions: [
            { action: "shell", resource: "*", effect: "ask" },
            { action: "question", resource: "*", effect: "allow" },
        ],
        policies: [
            { action: "provider.use", resource: "*", effect: "deny" },
            { action: "provider.use", resource: "fixture", effect: "allow" },
        ],
        providers: {
            fixture: {
                name: "Fixture",
                package: "aisdk:@ai-sdk/openai-compatible",
                settings: {
                    apiKey: "fixture",
                    baseURL: `http://127.0.0.1:${server.address().port}/v1`,
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
    ...isolatedOpenCodeEnvironment(root, config, taskix),
    TASKIX_CONFIG: join(root, "taskix.toml"),
    TMPDIR: join(root, "tmp"),
    TASKIX_MEMORY_ENABLED: "false",
    TERM: "xterm-256color",
    LANG: "en_US.UTF-8",
    AGENTIX_CONTROL_ENDPOINT: endpoint,
    AGENTIX_OPENCODE_STATE_DIR: join(root, "bridge"),
};
delete env.OPENCODE_DISABLE_DEFAULT_PLUGINS;
if (process.env.RMUX_TMPDIR) env.RMUX_TMPDIR = process.env.RMUX_TMPDIR;
mkdirSync(join(root, "tmp"), { recursive: true });
mkdirSync(join(root, "vault/.obsidian"), { recursive: true });
execFileSync(
    taskix,
    [
        "init",
        "--root",
        join(root, "vault"),
        "--directory",
        "Tasks",
        "--database",
        join(root, "tasks.sqlite3"),
    ],
    { env, stdio: "pipe" },
);
execFileSync(
    process.execPath,
    ["scripts/opencode-plugin.mjs", "install", "local"],
    {
        env,
        stdio: "pipe",
        timeout: 90000,
    },
);
const quote = (s) => "'" + s.replaceAll("'", "'\\''") + "'";
writeFileSync(
    join(root, "launch"),
    '#!/bin/sh\nprintf \'%s\\t%s\\n\' "$$" "$(LC_ALL=C ps -p "$$" -o lstart=)" >> ' +
        quote(join(root, "native-pids")) +
        "\nexec " +
        [
            "env",
            "-i",
            ...Object.entries(env)
                .filter(
                    ([key]) =>
                        ![
                            "AGENTIX_CONTROL_ENDPOINT",
                            "AGENTIX_OPENCODE_STATE_DIR",
                        ].includes(key),
                )
                .map(([k, v]) => k + "=" + v),
            agentix,
            "launch-open-code",
            "--command",
            process.env.AGENTIX_TEST_OPENCODE ?? "opencode",
            "--state-dir",
            env.AGENTIX_OPENCODE_STATE_DIR,
            "--endpoint",
            env.AGENTIX_CONTROL_ENDPOINT,
        ]
            .map(quote)
            .join(" ") +
        "\n",
);
chmodSync(join(root, "launch"), 0o700);
console.log("ready");
process.stdin.resume();
process.stdin.on("end", () => {
    server.closeAllConnections();
    server.close();
});
