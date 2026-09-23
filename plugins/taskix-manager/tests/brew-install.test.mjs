import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../../../", import.meta.url));
const formulas = ["agentix", "taskix"];
const preflight = formulas.map(name => `list --versions tenfyzhong/tap/${name}`);
const links = version => [
    "unlink tenfyzhong/tap/agentix tenfyzhong/tap/taskix",
    `link ${version === "head" ? "--HEAD " : ""}tenfyzhong/tap/agentix tenfyzhong/tap/taskix`,
];

async function fixture(t, options = {}) {
    const directory = await mkdtemp(join(tmpdir(), "agentix-brew-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const executable = join(directory, "brew");
    const log = join(directory, "calls");
    await writeFile(log, "");
    await writeFile(executable, `#!/bin/sh
echo "$*" >> "$BREW_TEST_LOG"
[ "$*" != "$BREW_TEST_FAIL" ] || exit 9
if [ "$1" = list ]; then
    echo "$3 $BREW_TEST_VERSIONS"
fi
`);
    await chmod(executable, 0o755);
    return {
        run: (...args) => spawnSync("make", [...args, `BREW=${executable}`], {
            cwd: repository,
            env: { ...process.env, BREW_TEST_LOG: log, BREW_TEST_VERSIONS: options.versions ?? "0.4.10 HEAD-abc", BREW_TEST_FAIL: options.fail ?? "" },
            encoding: "utf8",
        }),
        calls: async () => (await readFile(log, "utf8")).trim().split("\n").filter(Boolean),
    };
}

for (const version of ["stable", "head"]) {
    for (const target of ["install", "update", "switch"]) {
        test(`${target}_selects_${version}_for_both_clis`, { skip: process.platform === "win32" }, async t => {
            const f = await fixture(t);
            const result = f.run(target, `VERSION=${version}`);
            assert.equal(result.status, 0, result.stderr);
            const install = `install ${version === "head" ? "--HEAD " : ""}${target === "update" && version === "head" ? "--fetch-HEAD " : ""}--skip-link tenfyzhong/tap/agentix tenfyzhong/tap/taskix`;
            assert.deepEqual(await f.calls(), [
                ...(target === "switch" ? [] : ["update", install]),
                ...preflight, ...links(version),
            ]);
        });
    }
    test(`switch_rejects_missing_${version}_before_unlink`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t, { versions: version === "head" ? "0.4.10" : "HEAD-abc" });
        assert.notEqual(f.run("switch", `VERSION=${version}`).status, 0);
        assert.deepEqual(await f.calls(), preflight.slice(0, 1));
    });
}

test("install_failure_preserves_command_links", { skip: process.platform === "win32" }, async t => {
    const fail = "install --skip-link tenfyzhong/tap/agentix tenfyzhong/tap/taskix";
    const f = await fixture(t, { fail });
    assert.notEqual(f.run("install").status, 0);
    assert.deepEqual(await f.calls(), ["update", fail]);
});

test("invalid_version_fails_before_brew", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.notEqual(f.run("install", "VERSION=typo").status, 0);
    assert.deepEqual(await f.calls(), []);
});

test("switch_supports_a_single_formula", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run("switch", "FORMULAE=agentix").status, 0);
    assert.deepEqual(await f.calls(), [
        preflight[0], "unlink tenfyzhong/tap/agentix", "link tenfyzhong/tap/agentix",
    ]);
});

for (const failure of ["update", "unlink tenfyzhong/tap/agentix tenfyzhong/tap/taskix", "link tenfyzhong/tap/agentix tenfyzhong/tap/taskix"]) {
    test(`brew_failure_is_propagated_at_${failure.split(" ")[0]}`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t, { fail: failure });
        assert.notEqual(f.run("install").status, 0);
        const calls = await f.calls();
        assert.equal(calls.at(-1), failure);
    });
}

test("missing_second_formula_preserves_all_command_links", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t, { fail: preflight[1] });
    assert.notEqual(f.run("switch").status, 0);
    assert.deepEqual(await f.calls(), preflight);
});
