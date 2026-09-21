import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../../../", import.meta.url));
const removals = ["codex plugin remove taskix-manager@agentix", "codex plugin marketplace remove agentix"];

async function fixture(t) {
    const directory = await mkdtemp(join(tmpdir(), "agentix-dev-install-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const bin = join(directory, "bin");
    await mkdir(bin);
    for (const command of ["cargo", "cp", "rm", "taskix", "codex", "claude", "pi", "omp"]) {
        const path = join(bin, command);
        await writeFile(path, `#!/bin/sh
echo "${command} $*" >> "$INSTALL_FIXTURE_DIR/calls"
if [ "${command}" = "codex" ]; then
    case "$*" in
        "plugin add "*) [ "$INSTALL_FAIL" != "install" ] || exit 8 ;;
        "plugin remove "*) [ "$INSTALL_FAIL" != "remove" ] || exit 7 ;;
    esac
fi
exit 0
`);
        await chmod(path, 0o755);
    }
    return {
        calls: async () => (await readFile(join(directory, "calls"), "utf8")).trim().split("\n"),
        run: (target, failure = "") => spawnSync("make", [target, "CARGO=cargo"], {
            cwd: repository,
            env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, INSTALL_FIXTURE_DIR: directory, INSTALL_FAIL: failure },
            encoding: "utf8",
        }),
    };
}

test("remove-plugin only removes the Agentix Codex integration", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    const result = f.run("remove-plugin");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), removals);
});

for (const target of ["dev-test", "prod-test"]) {
    test(`${target} reinstalls only Codex without changing binaries or other hosts`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const source = target === "dev-test" ? "." : "tenfyzhong/agentix";
        const expected = [...removals, `codex plugin marketplace add ${source}`, "codex plugin add taskix-manager@agentix"];
        for (let attempt = 1; attempt <= 2; attempt++) {
            const result = f.run(target);
            assert.equal(result.status, 0, result.stderr);
            assert.deepEqual(await f.calls(), Array.from({ length: attempt }, () => expected).flat());
        }
    });
    test(`${target} tolerates missing plugins but propagates installation failures`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        assert.equal(f.run(target, "remove").status, 0);
        assert.notEqual(f.run(target, "install").status, 0);
    });
}
