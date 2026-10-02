import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
async function fixture(t, fail = false) {
    const dir = await mkdtemp(join(tmpdir(), "make-native-local-"));
    t.after(() => rm(dir, {recursive: true, force: true}));
    const log = join(dir, "calls");
    const brew = join(dir, "brew");
    await writeFile(brew, '#!/bin/sh\nprintf "%s|%s|%s\\n" "$HOMEBREW_AGENTIX_LOCAL_SOURCE" "$HOMEBREW_AGENTIX_LOCAL_PROFILE" "$*" >> "$LOCAL_LOG"\n[ "$LOCAL_FAIL" != 1 ]\n');
    await chmod(brew, 0o755);
    return {
        run: (...args) => spawnSync("make", ["update", "VERSION=local", `BREW=${brew}`, "CARGO=/nonexistent-cargo", ...args], {cwd: root, encoding: "utf8", env: {...process.env, LOCAL_LOG: log, LOCAL_FAIL: fail ? "1" : "0"}}),
        calls: async () => (await readFile(log, "utf8").catch(() => "")).trim().split("\n").filter(Boolean),
    };
}
for (const profile of ["release", "debug"]) {
    test(`update_local_${profile}_uses_formula_source_build`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const result = f.run(`PROFILE=${profile}`);
        assert.equal(result.status, 0, result.stdout + result.stderr);
        assert.deepEqual(await f.calls(), [`${root.replace(/\/$/, "")}|${profile}|reinstall --build-from-source tenfyzhong/tap/agentix tenfyzhong/tap/taskix`]);
    });
}
test("update_local_selects_single_formula", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run("FORMULAE=taskix").status, 0);
    assert.deepEqual(await f.calls(), [`${root.replace(/\/$/, "")}|release|reinstall --build-from-source tenfyzhong/tap/taskix`]);
});
for (const arg of ["PROFILE=bad", "FORMULAE=unknown", "FORMULAE="]) {
    test(`update_local_rejects_${arg}_before_brew`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        assert.notEqual(f.run(arg).status, 0);
        assert.deepEqual(await f.calls(), []);
    });
}
test("update_local_propagates_homebrew_failure", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t, true);
    assert.notEqual(f.run().status, 0);
    assert.equal((await f.calls()).length, 1);
});
