import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, lstat, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../../../", import.meta.url));

async function fixture(t) {
    const directory = await mkdtemp(join(tmpdir(), "agentix-dev-install-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const bin = join(directory, "bin");
    await mkdir(bin);
    for (const command of ["cargo", "cp", "rm", "taskix", "codex", "claude", "pi"]) {
        const path = join(bin, command);
        await writeFile(path, `#!/bin/sh
echo "${command} $*" >> "$OMP_FIXTURE_DIR/calls"
exit 0
`);
        await chmod(path, 0o755);
    }
    const omp = join(bin, "omp");
    await writeFile(omp, `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const target = path.join(process.env.OMP_FIXTURE_DIR, "agentix-plugins");
const args = process.argv.slice(2);
fs.appendFileSync(path.join(process.env.OMP_FIXTURE_DIR, "calls"), "omp " + args.join(" ") + "\\n");
if (args.join(" ") === "plugin uninstall agentix-plugins") {
    if (process.env.OMP_FAIL === "uninstall") process.exit(7);
    fs.rmSync(target, { recursive: true, force: true });
} else if (args[0] === "install") {
    if (process.env.OMP_FAIL === "install") process.exit(8);
    try { fs.unlinkSync(target); } catch (error) { if (error.code !== "ENOENT") throw error; }
    if (args[1] === ".") fs.symlinkSync(process.cwd(), target);
    else fs.mkdirSync(target);
} else { throw new Error("Unexpected OMP command: " + args.join(" ")); }
`);
    await chmod(omp, 0o755);
    return {
        target: join(directory, "agentix-plugins"),
        calls: async () => (await readFile(join(directory, "calls"), "utf8")).trim().split("\n"),
        run: (failure = "", target = "dev-test") => spawnSync("make", [target, "CARGO=cargo"], {
            cwd: repository,
            env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, OMP_FIXTURE_DIR: directory, OMP_FAIL: failure },
            encoding: "utf8",
        }),
    };
}

test("dev-test replaces a remote OMP directory and can relink repeatedly", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    await mkdir(f.target);
    await writeFile(join(f.target, "remote-package"), "old version");
    for (let attempt = 0; attempt < 2; attempt++) {
        const result = f.run();
        assert.equal(result.status, 0, result.stderr);
        assert.ok((await lstat(f.target)).isSymbolicLink(), result.stderr);
        assert.equal(await realpath(f.target), await realpath(repository));
    }
    assert.ok(await readFile(join(repository, "package.json")), "relink preserves source checkout");
});

test("dev-test installs OMP from a clean state and propagates installation failures", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run().status, 0);
    assert.ok((await lstat(f.target)).isSymbolicLink());
    assert.notEqual(f.run("install").status, 0, "make must report failed installation");
    await symlink(repository, f.target);
    assert.equal(f.run("uninstall").status, 0, "best-effort cleanup must allow installation to continue");
    assert.ok((await lstat(f.target)).isSymbolicLink());
    assert.equal(await realpath(f.target), await realpath(repository));
});

const removals = [
    "codex plugin remove taskix-manager@agentix",
    "codex plugin marketplace remove agentix",
    "claude plugin uninstall taskix-manager@agentix",
    "claude plugin uninstall agentix-bridge@agentix",
    "claude plugin marketplace remove agentix",
    "pi remove .",
    "pi remove git:github.com/tenfyzhong/agentix",
    "omp plugin uninstall agentix-plugins",
];

test("remove-plugin only removes Agentix integrations", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    const result = f.run("", "remove-plugin");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), removals);
});

for (const target of ["dev-test", "prod-test"]) {
    test(`${target} cleans all integrations before installing without changing binaries`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const result = f.run("", target);
        assert.equal(result.status, 0, result.stderr);
        const calls = await f.calls();
        assert.deepEqual(calls.slice(0, removals.length), removals);
        const source = target === "dev-test" ? "." : "tenfyzhong/agentix";
        assert.deepEqual(calls.slice(removals.length), [
            `codex plugin marketplace add ${source}`,
            "codex plugin add taskix-manager@agentix",
            `claude plugin marketplace add ${target === "dev-test" ? "./" : source}`,
            "claude plugin install taskix-manager@agentix",
            "claude plugin install agentix-bridge@agentix",
            `pi install ${target === "dev-test" ? "." : "git:github.com/tenfyzhong/agentix"}`,
            `omp install ${target === "dev-test" ? "." : "github:tenfyzhong/agentix"}`,
        ]);
    });
}
