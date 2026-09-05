import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = new URL("../", import.meta.url);
const readJson = async (path) =>
    JSON.parse(await readFile(new URL(path, root), "utf8"));

test("both command-hook hosts discover exactly one bundled lifecycle configuration", async () => {
    for (const host of ["codex", "claude"]) {
        const manifest = await readJson(`.${host}-plugin/plugin.json`);
        assert.equal(manifest.name, "agent-task-manager");
        assert.equal(
            manifest.hooks,
            undefined,
            "use the default hook file, without duplicate declarations",
        );
        assert.equal(manifest.skills, "./skills/");
    }
    const config = await readJson("hooks/hooks.json");
    assert.deepEqual(Object.keys(config.hooks).sort(), [
        "PostToolUse",
        "PreToolUse",
        "SessionEnd",
        "SessionStart",
        "Stop",
    ]);
    for (const [event, groups] of Object.entries(config.hooks)) {
        assert.equal(groups.length, 1, event);
        assert.equal(groups[0].hooks.length, 1, event);
        assert.ok([undefined, "", "*"].includes(groups[0].matcher));
        const hook = groups[0].hooks[0];
        assert.equal(hook.type, "command");
        assert.equal(hook.timeout, event === "SessionEnd" ? 3 : 30);
    }
});

test("package manifests select one host-specific extension and the shared skill", async () => {
    const pkg = await readJson("package.json");
    for (const host of ["pi", "omp"]) {
        assert.deepEqual(pkg[host].extensions, [`./extensions/${host}.ts`]);
        assert.deepEqual(pkg[host].skills, ["./skills"]);
        const { default: install } = await import(
            new URL(pkg[host].extensions[0], root)
        );
        const handlers = new Map(),
            tools = [];
        install({
            on: (event, handler) => {
                assert.ok(!handlers.has(event), event);
                handlers.set(event, handler);
            },
            registerTool: (tool) => tools.push(tool),
        });
        assert.deepEqual([...handlers.keys()].sort(), [
            "before_agent_start",
            "session_shutdown",
            "session_start",
        ]);
        assert.deepEqual(
            tools.map((tool) => tool.name),
            ["taskcli"],
        );
    }
});

test("npm package contains all host manifests, hooks and resources but no tests", async () => {
    const pkg = await readJson("package.json");
    assert.ok(
        Array.isArray(pkg.files),
        "declare the plugin's installable files explicitly",
    );
    const command = "npm pack --dry-run --json --ignore-scripts --offline";
    const output = execFileSync(
        process.platform === "win32" ? "cmd.exe" : "/bin/sh",
        process.platform === "win32"
            ? ["/d", "/s", "/c", `"${command}"`]
            : ["-c", command],
        {
            cwd: fileURLToPath(root),
            encoding: "utf8",
            timeout: 30000,
            windowsVerbatimArguments: process.platform === "win32",
        },
    );
    const files = JSON.parse(output)[0].files.map((file) => file.path);
    for (const path of [
        ".codex-plugin/plugin.json",
        ".claude-plugin/plugin.json",
        "package.json",
        "hooks/hooks.json",
        "hooks/run.mjs",
        "runtime.mjs",
        "extensions/pi.ts",
        "extensions/omp.ts",
        "skills/agent-task-manager/SKILL.md",
        "skills/agent-task-manager/references/commands.md",
        "README.md",
    ])
        assert.ok(files.includes(path), `missing packaged file: ${path}`);
    assert.ok(!files.some((path) => path.startsWith("tests/")));
});
