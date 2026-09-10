import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { listen, waitFor } from "../../agentix-bridge/tests/support.mjs";

test("native OMP discovers taskix-manager from the extension package", {
    skip: process.platform === "win32" || !process.env.AGENTIX_TEST_NATIVE_HOSTS,
    timeout: 30000,
}, async t => {
    const directory = await mkdtemp(join(tmpdir(), "ax-omp-skill-"));
    const server = await listen(directory, t);
    const vault = join(directory, "vault");
    await mkdir(join(vault, ".obsidian"), { recursive: true });
    const config = join(directory, "taskix.toml");
    const readResults = join(directory, "read-results.json");
    await writeFile(config, `schema_version = 1\n[storage]\npath = ${JSON.stringify(join(directory, "tasks.sqlite3"))}\n[documents]\nroot = ${JSON.stringify(vault)}\ndirectory = "Tasks"\n`);
    const host = spawn("omp", [
        "--mode", "rpc", "--no-extensions", "--no-lsp", "--no-title", "--no-rules",
        "--session-dir", join(directory, "sessions"),
        "-e", process.env.TASKIX_TEST_OMP_PACKAGE_DIR ?? fileURLToPath(new URL("../../../", import.meta.url)),
        "-e", fileURLToPath(new URL("./omp-read-skill.ts", import.meta.url)),
    ], {
        cwd: directory,
        env: {
            ...process.env,
            PI_CODING_AGENT_DIR: join(directory, "config"),
            TASKIX_CONFIG: config,
            TASKIX_TEST_READ_RESULTS: readResults,
            AGENTIX_CONTROL_ENDPOINT: `unix://${join(directory, "control.sock")}`,
            PI_OFFLINE: "1",
            OPENAI_API_KEY: "test-only-no-model-requests",
        },
        stdio: ["pipe", "pipe", "pipe"],
    });
    let output = "";
    host.stdout.on("data", data => { output += data; });
    host.stderr.on("data", data => { output += data; });
    t.after(async () => {
        if (host.exitCode === null) {
            host.kill("SIGTERM");
            await once(host, "exit");
        }
        await rm(directory, { recursive: true, force: true });
    });
    const client = await server.next();
    t.after(() => client.socket.destroy());
    const result = await client.request("command", { name: "skills" });
    assert.equal(result.ok, true, output);
    assert.match(result.result.body, /taskix-manager/, output);
    // session_start handlers finish before the host accepts RPC state requests.
    host.stdin.write(JSON.stringify({ id: "read-probe-ready", type: "get_state" }) + "\n");
    await waitFor(() => output.includes('"id":"read-probe-ready"'));
    const reads = JSON.parse(await readFile(readResults, "utf8"));
    assert.equal(reads.length, 2);
    for (const [index, read] of reads.entries()) {
        assert.equal(read.error, undefined, read.error);
        assert.ok(!read.result.isError, JSON.stringify(read.result));
        const text = read.result.content.filter(part => part.type === "text").map(part => part.text).join("\n");
        assert.match(text, index === 0 ? /Begin or resume/ : /Taskix workflow/);
    }
});
