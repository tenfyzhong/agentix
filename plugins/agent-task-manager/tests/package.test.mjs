import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { posix } from "node:path";

const root = new URL("../", import.meta.url);
const readJson = async (path) =>
    JSON.parse(await readFile(new URL(path, root), "utf8"));

test("repository marketplaces resolve the same complete host plugin", async () => {
    const codex = await readJson("../../.agents/plugins/marketplace.json");
    const claude = await readJson("../../.claude-plugin/marketplace.json");
    assert.equal(codex.name, "agentix");
    assert.equal(claude.name, codex.name);
    assert.equal(codex.interface.displayName, "Agentix");
    assert.equal(claude.owner.name, "tenfyzhong");
    for (const [host, marketplace] of [["codex", codex], ["claude", claude]]) {
        assert.equal(marketplace.plugins.length, 1);
        const entry = marketplace.plugins[0];
        assert.equal(entry.name, "agent-task-manager");
        const source = host === "codex" ? entry.source.path : entry.source;
        assert.equal(source, "./plugins/agent-task-manager");
        const manifest = await readJson(`../../${source}/.${host}-plugin/plugin.json`);
        assert.equal(manifest.name, entry.name);
    }
    assert.equal(codex.plugins[0].source.source, "local");
    assert.deepEqual(codex.plugins[0].policy, {
        installation: "AVAILABLE",
        authentication: "ON_INSTALL",
    });
    assert.equal(codex.plugins[0].category, "Productivity");
});

test("hosts discover shared hooks once and load only their own interruption events", async () => {
    for (const host of ["codex", "claude"]) {
        const manifest = await readJson(`.${host}-plugin/plugin.json`);
        assert.equal(manifest.name, "agent-task-manager");
        assert.deepEqual(
            manifest.hooks,
            host === "codex" ? ["./hooks/hooks.json", "./hooks/codex.json"] : "./hooks/claude.json",
            "load shared hooks once and keep Codex events out of Claude config",
        );
        assert.equal(manifest.skills, "./skills/");
    }
    const claude = await readJson("hooks/claude.json");
    assert.deepEqual(Object.keys(claude.hooks), ["PostToolUseFailure"]);
    assert.equal(claude.hooks.PostToolUseFailure[0].hooks[0].timeout, 3);
    const codex = await readJson("hooks/codex.json");
    assert.deepEqual(Object.keys(codex.hooks), ["Interrupt"]);
    assert.equal(codex.hooks.Interrupt.length, 1);
    assert.equal(codex.hooks.Interrupt[0].hooks.length, 1);
    assert.equal(codex.hooks.Interrupt[0].hooks[0].timeout, 3);
    assert.equal(codex.hooks.Interrupt[0].hooks[0].type, "command");
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
            "agent_end",
            ...(host === "pi" ? ["agent_settled"] : []),
            "agent_start",
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
        "hooks/codex.json",
        "hooks/claude.json",
        "hooks/run.mjs",
        "runtime.mjs",
        "extensions/pi.ts",
        "extensions/omp.ts",
        "skills/agent-task-manager/SKILL.md",
        "skills/agent-task-manager/references/commands.md",
        "obsidian/README.md",
        "obsidian/tasknotes-settings.json",
        "README.md",
    ])
        assert.ok(files.includes(path), `missing packaged file: ${path}`);
    assert.ok(!files.some((path) => path.startsWith("tests/")));
    for (const path of files.filter(path => path.endsWith(".md"))) {
        const prose = (await readFile(new URL(path, root), "utf8"))
            .replace(/```[\s\S]*?```/g, "")
            .replace(/`[^`\n]*`/g, "");
        for (const [, target] of prose.matchAll(/\[[^\]]+\]\(([^\s)]+)\)/g)) {
            if (/^[a-z]+:|^#/i.test(target)) continue;
            const destination = posix.normalize(posix.join(posix.dirname(path), decodeURIComponent(target.split("#")[0])));
            assert.ok(files.includes(destination), `${path}: link ${target} is missing from the installed package`);
        }
    }
});
