import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
async function fixture(t, fail = false, failBuild = false) {
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
    const cargo = join(dir, "cargo");
    await writeFile(cargo, `#!/bin/sh
printf 'cargo|%s|%s\\n' "$CARGO_TARGET_DIR" "$*" >> "$LOCAL_LOG"
[ "$LOCAL_BUILD_FAIL" != 1 ] || exit 1
[ "$LOCAL_BUILD_MODE" != no-output ] || exit 0
profile=debug
for arg in "$@"; do [ "$arg" != --release ] || profile=release; done
target_dir=\${CARGO_TARGET_DIR:-target}
mkdir -p "$target_dir/$profile"
for name in agentix taskix; do
    printf 'built fixture\\n' > "$target_dir/$profile/$name"
    chmod +x "$target_dir/$profile/$name"
done
`);
    await chmod(cargo, 0o755);
    const brew = join(dir, "brew");
    await writeFile(brew, '#!/bin/sh\nif [ "$1" = ruby ]; then shift 3; set -- ruby "$@"; fi\nprintf "%s|%s|%s|%s\\n" "$HOMEBREW_AGENTIX_LOCAL_SOURCE" "$HOMEBREW_AGENTIX_LOCAL_PROFILE" "$HOMEBREW_AGENTIX_LOCAL_TARGET_DIR" "$*" >> "$LOCAL_LOG"\n[ "$LOCAL_FAIL" != 1 ]\n');
    await chmod(brew, 0o755);
    const successfulCalls = async (checkout = root.replace(/\/$/, ""), profile = "release", names = "agentix taskix", targetDirectory = target) => {
        const calls = (await readFile(log, "utf8")).trim().split("\n");
        const formulae = names.split(" ").map(name => `tenfyzhong/tap/${name}`).join(" ");
        assert.equal(calls.length, 3, "Build before installing all artifacts and switching links");
        assert.equal(calls.shift(), `cargo|${targetDirectory === "target" ? "" : targetDirectory}|build --workspace --all-features${profile === "release" ? " --release" : ""}`);
        assert.equal(calls[0], `${checkout}|${profile}|${targetDirectory}|install --build-from-source --skip-link ${formulae}`);
        assert.ok(calls[1].startsWith(`${checkout}|${profile}|${targetDirectory}|ruby `), "Switch must resolve the same artifact inputs as installation");
        assert.ok(calls[1].endsWith(` ${profile} ${formulae}`), "Switch must use the selected profile and formulae");
    };
    return {
        dir, brew, cargo, log, target, successfulCalls,
        run: (...args) => spawnSync("make", ["update", "VERSION=local", `BREW=${brew}`, `CARGO=${cargo}`, `CARGO_TARGET_DIR=${target}`, ...args], {cwd: root, encoding: "utf8", env: {...process.env, LOCAL_LOG: log, LOCAL_FAIL: fail ? "1" : "0", LOCAL_BUILD_FAIL: failBuild ? "1" : "0"}}),
        allCalls: async () => (await readFile(log, "utf8").catch(() => "")).trim().split("\n").filter(Boolean),
        calls: async () => (await readFile(log, "utf8").catch(() => "")).trim().split("\n").filter(line => line && !line.startsWith("cargo|")),
    };
}
for (const profile of ["release", "debug"]) {
    test(`update_local_${profile}_builds_before_install`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        await rm(join(f.target, profile), {recursive: true});
        const result = f.run(`PROFILE=${profile}`, "-j4");
        assert.equal(result.status, 0, result.stdout + result.stderr);
        await f.successfulCalls(undefined, profile);
        assert.equal(await readFile(join(f.target, profile, "agentix"), "utf8"), "built fixture\n");
    });
}
test("update_local_selects_single_formula", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run("FORMULAE=taskix").status, 0);
    await f.successfulCalls(undefined, "release", "taskix");
});
test("update_local_hides_recipes_and_preserves_homebrew_output", {skip: process.platform === "win32"}, async t => {
    const f = await fixture(t);
    await writeFile(f.brew, (await readFile(f.brew, "utf8"))
        .replace('[ "$LOCAL_FAIL" != 1 ]', 'echo "Homebrew output: $1"\necho "Homebrew diagnostic: $1" >&2\n[ "$LOCAL_FAIL" != 1 ]'));
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    await f.successfulCalls();
    assert.doesNotMatch(result.stdout, /HOMEBREW_AGENTIX_LOCAL_|ruby -e|switch VERSION=local/);
    assert.match(result.stdout, /Homebrew output: install/);
    assert.match(result.stdout, /Homebrew output: ruby/);
    assert.match(result.stderr, /Homebrew diagnostic: install/);
    assert.match(result.stderr, /Homebrew diagnostic: ruby/);
});
for (const arg of ["PROFILE=bad", "FORMULAE=unknown", "FORMULAE="]) {
    test(`update_local_rejects_${arg}_before_brew`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        assert.notEqual(f.run(arg).status, 0);
        assert.deepEqual(await f.allCalls(), []);
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
        const result = f.run(`PROFILE=${profile}`, "LOCAL_BUILD_MODE=no-output");
        assert.notEqual(result.status, 0);
        assert.match(result.stderr, profile === "debug" ? /after make build/ : /after make release/);
        assert.deepEqual(await f.calls(), []);
    });
}
test("update_local_does_not_require_unselected_binary", {skip: process.platform === "win32"}, async t => {
    const f = await fixture(t);
    await rm(join(f.target, "release/agentix"));
    assert.equal(f.run("FORMULAE=taskix", "LOCAL_BUILD_MODE=no-output").status, 0);
    await f.successfulCalls(undefined, "release", "taskix");
});

test("update_local_reads_default_target_relative_to_current_checkout", {skip: process.platform === "win32"}, async t => {
    const f = await fixture(t);
    await symlink(f.target, join(f.dir, "target"));
    await writeFile(join(f.dir, "Makefile"), await readFile(join(root, "Makefile")));
    const result = spawnSync("make", ["update", "VERSION=local", `BREW=${f.brew}`, `CARGO=${f.cargo}`, "CARGO_TARGET_DIR="], {cwd: f.dir, encoding: "utf8", env: {...process.env, LOCAL_LOG: f.log, LOCAL_FAIL: "0"}});
    assert.equal(result.status, 0, result.stderr);
    await f.successfulCalls(await realpath(f.dir), "release", "agentix taskix", "target");
});

for (const profile of ["release", "debug"]) {
    test(`update_local_${profile}_stops_before_homebrew_when_build_fails`, {skip: process.platform === "win32"}, async t => {
        const f = await fixture(t, false, true);
        const result = f.run(`PROFILE=${profile}`);
        assert.notEqual(result.status, 0);
        assert.deepEqual(await f.calls(), []);
        assert.deepEqual(await f.allCalls(), [`cargo|${f.target}|build --workspace --all-features${profile === "release" ? " --release" : ""}`]);
    });
}

test("link_debug_target_and_help_are_removed", {skip: process.platform === "win32"}, () => {
    const help = spawnSync("make", ["help"], {cwd: root, encoding: "utf8"});
    assert.equal(help.status, 0, help.stderr);
    assert.doesNotMatch(help.stdout, /link-debug/);
    const removed = spawnSync("make", ["-n", "link-debug"], {cwd: root, encoding: "utf8"});
    assert.notEqual(removed.status, 0);
    assert.match(removed.stderr, /No rule to make target/);
});
