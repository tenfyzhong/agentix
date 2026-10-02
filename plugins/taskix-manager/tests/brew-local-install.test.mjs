import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
async function fixture(t, fail = false) {
    const dir = await mkdtemp(join(tmpdir(), "make-native-local-"));
    t.after(() => rm(dir, {recursive: true, force: true}));
    const target = join(dir, "custom target");
    for (const profile of ["release", "debug"]) {
        await mkdir(join(target, profile), {recursive: true});
        for (const name of ["agentix", "taskix"]) {
            await writeFile(join(target, profile, name), "prebuilt fixture\n");
            await chmod(join(target, profile, name), 0o755);
        }
    }
    const log = join(dir, "calls");
    const brew = join(dir, "brew");
    await writeFile(brew, '#!/bin/sh\nprintf "%s|%s|%s|%s\\n" "$HOMEBREW_AGENTIX_LOCAL_SOURCE" "$HOMEBREW_AGENTIX_LOCAL_PROFILE" "$HOMEBREW_AGENTIX_LOCAL_TARGET_DIR" "$*" >> "$LOCAL_LOG"\n[ "$LOCAL_FAIL" != 1 ]\n');
    await chmod(brew, 0o755);
    return {
        dir, brew, log, target,
        run: (...args) => spawnSync("make", ["update", "VERSION=local", `BREW=${brew}`, "CARGO=/nonexistent-cargo", `CARGO_TARGET_DIR=${target}`, ...args], {cwd: root, encoding: "utf8", env: {...process.env, LOCAL_LOG: log, LOCAL_FAIL: fail ? "1" : "0"}}),
        calls: async () => (await readFile(log, "utf8").catch(() => "")).trim().split("\n").filter(Boolean),
    };
}
for (const profile of ["release", "debug"]) {
    test(`update_local_${profile}_reuses_prebuilt_binary`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const result = f.run(`PROFILE=${profile}`);
        assert.equal(result.status, 0, result.stdout + result.stderr);
        assert.deepEqual(await f.calls(), [`${root.replace(/\/$/, "")}|${profile}|${f.target}|reinstall --build-from-source tenfyzhong/tap/agentix tenfyzhong/tap/taskix`]);
    });
}
test("update_local_selects_single_formula", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run("FORMULAE=taskix").status, 0);
    assert.deepEqual(await f.calls(), [`${root.replace(/\/$/, "")}|release|${f.target}|reinstall --build-from-source tenfyzhong/tap/taskix`]);
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

for (const [label, name, profile] of [["missing_second_binary", "taskix", "release"], ["missing_debug_binary", "agentix", "debug"], ["nonexecutable_binary", "agentix", "release"]]) {
    test(`update_local_rejects_${label}_before_brew`, {skip: process.platform === "win32"}, async t => {
        const f = await fixture(t);
        const binary = join(f.target, profile, name);
        if (label === "nonexecutable_binary") await chmod(binary, 0o644);
        else await rm(binary);
        const result = f.run(`PROFILE=${profile}`);
        assert.notEqual(result.status, 0);
        assert.match(result.stderr, profile === "debug" ? /run make first/ : /run make release first/);
        assert.deepEqual(await f.calls(), []);
    });
}
test("update_local_does_not_require_unselected_binary", {skip: process.platform === "win32"}, async t => {
    const f = await fixture(t);
    await rm(join(f.target, "release/agentix"));
    assert.equal(f.run("FORMULAE=taskix").status, 0);
    assert.equal((await f.calls()).length, 1);
});

test("update_local_reads_default_target_relative_to_current_checkout", {skip: process.platform === "win32"}, async t => {
    const f = await fixture(t);
    await symlink(f.target, join(f.dir, "target"));
    const result = spawnSync("make", ["-f", join(root, "Makefile"), "update", "VERSION=local", `BREW=${f.brew}`, "CARGO=/nonexistent-cargo", "CARGO_TARGET_DIR="], {cwd: f.dir, encoding: "utf8", env: {...process.env, LOCAL_LOG: f.log, LOCAL_FAIL: "0"}});
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), [`${await realpath(f.dir)}|release|target|reinstall --build-from-source tenfyzhong/tap/agentix tenfyzhong/tap/taskix`]);
});
