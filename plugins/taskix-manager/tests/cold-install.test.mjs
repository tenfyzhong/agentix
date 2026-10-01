import assert from "node:assert/strict";
import test from "node:test";
import { cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { execFileSync, spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const source = fileURLToPath(new URL("../", import.meta.url));

for (const distribution of ["directory", "npm_tarball"]) {
    test(`${distribution}_loads_hooks_and_adapters_without_node_modules`, async t => {
        const directory = await mkdtemp(join(tmpdir(), "taskix-cold-install-"));
        t.after(() => rm(directory, { recursive: true, force: true }));
        const root = join(directory, "installed plugin \u{2603}");
        await cp(source, root, {
            recursive: true,
            filter: path => !/[\\/](node_modules|tests)([\\/]|$)/.test(path),
        });
        if (distribution === "npm_tarball") {
            const command = process.platform === "win32" ? "cmd.exe" : "npm";
            const args = process.platform === "win32"
                ? ["/d", "/s", "/c", "npm pack --ignore-scripts --offline --json"]
                : ["pack", "--ignore-scripts", "--offline", "--json"];
            const [archive] = JSON.parse(execFileSync(command, args, {
                cwd: root, encoding: "utf8", timeout: 30000,
                env: { ...process.env, npm_config_cache: join(directory, "empty npm cache") },
            }));
            const unpacked = join(directory, "unpacked");
            await mkdir(unpacked);
            execFileSync("tar", ["-xzf", join(root, archive.filename), "-C", unpacked, "--strip-components=1"]);
            await rm(root, { recursive: true });
            await cp(unpacked, root, { recursive: true });
        }
        await assert.rejects(stat(join(root, "node_modules")), { code: "ENOENT" });
        const env = {
            ...process.env, NODE_PATH: "", TASKIX_JEV_ENABLED: "false",
            XDG_STATE_HOME: join(directory, "state"),
        };
        const runHook = input => spawnSync(process.execPath, [join(root, "hooks", "run.mjs")], {
            input: JSON.stringify(input), encoding: "utf8", cwd: directory, env,
        });
        const success = runHook({ hook_event_name: "PostToolUseFailure", session_id: "cold-install", is_interrupt: false });
        assert.equal(success.status, 0, success.stderr);
        assert.equal(success.stderr, "");
        assert.deepEqual(JSON.parse(success.stdout), {});

        // Confirm that the hook's diagnostic handler loads too, before any CLI call.
        const failure = runHook({ hook_event_name: "Stop", cwd: directory });
        assert.equal(failure.status, 1);
        assert.match(failure.stderr, /Task board hook: Hook requires session_id/);
        const error = JSON.parse(await readFile(join(directory, "state", "taskix", "hooks.jsonl"), "utf8"));
        assert.equal(error.message, "Hook requires session_id");

        const config = join(directory, "config.toml");
        await writeFile(config, '[memory]\nenabled = true # TOML comment\n');
        const checks = `
            import assert from "node:assert/strict";
            const root = ${JSON.stringify(pathToFileURL(`${root}/`).href)};
            const memory = await import(new URL("memory.mjs", root));
            assert.equal(await memory.memoryConfigured(${JSON.stringify(config)}), true);
            await import(new URL("discussion.mjs", root));
            await import(new URL("lifecycle.mjs", root));
            for (const host of ["pi", "omp"]) {
                const { default: install } = await import(new URL("extensions/" + host + ".mjs", root));
                const events = [], tools = [];
                install({ on: event => events.push(event), registerTool: tool => tools.push(tool) });
                assert.equal(events.includes("agent_settled"), host === "pi");
                assert.equal(tools[0].name, "taskix");
                assert.equal(tools[0].parameters.properties.args.type, "array");
            }
        `;
        for (const host of ["pi", "omp"]) {
            await cp(join(root, "extensions", `${host}.ts`), join(root, "extensions", `${host}.mjs`));
        }
        const loaded = spawnSync(process.execPath, ["--input-type=module", "-e", checks], {
            encoding: "utf8", cwd: directory, env,
        });
        assert.equal(loaded.status, 0, loaded.stderr);
    });
}
