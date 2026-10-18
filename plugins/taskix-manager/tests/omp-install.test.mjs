import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, chmod, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

for (const failure of [false, true]) {
    test(`omp_remote_branch_clone_${failure ? "failure_preserves_cache" : "install_and_remove"}`, { skip: process.platform === "win32" }, async t => {
        const dir = await mkdtemp(join(tmpdir(), "omp-install-"));
        t.after(() => rm(dir, { recursive: true, force: true }));
        const bin = join(dir, "bin");
        await mkdir(bin);
        const cache = join(dir, "agentix", "omp-marketplace");
        await mkdir(cache, { recursive: true });
        await writeFile(join(cache, "old"), "previous");
        for (const command of ["git", "omp"]) {
            const file = join(bin, command);
            await writeFile(file, `#!/bin/sh
printf '%s\\n' '${command}' "$@" >> "$TEST_CALLS"
if [ '${command}' = git ]; then
    ${failure ? "exit 7" : 'for last; do :; done; mkdir -p "$last"; touch "$last/new"'}
fi
`);
            await chmod(file, 0o755);
        }
        const run = (...args) => spawnSync(process.execPath, ["scripts/omp-plugin.mjs", ...args], {
            cwd: new URL("../../../", import.meta.url),
            env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, OMP: join(bin, "omp"), XDG_CACHE_HOME: dir, TEST_CALLS: join(dir, "calls") },
            encoding: "utf8",
        });
        const result = run("install", "feature/plugin-test");
        if (failure) {
            assert.notEqual(result.status, 0);
            assert.equal(await readFile(join(cache, "old"), "utf8"), "previous");
            assert.doesNotMatch(await readFile(join(dir, "calls"), "utf8"), /\nomp\n/);
        } else {
            assert.equal(result.status, 0, result.stderr);
            assert.equal(await readFile(join(cache, "new"), "utf8"), "");
            const calls = await readFile(join(dir, "calls"), "utf8");
            assert.match(calls, /git\nclone\n--depth\n1\n--single-branch\n--branch\nfeature\/plugin-test\n--\nhttps:\/\/github.com\/tenfyzhong\/agentix.git\n/);
            assert.ok(calls.includes(`omp\nplugin\nmarketplace\nadd\n${cache}\n`));
            assert.match(calls, /omp\nplugin\ninstall\ntaskix-manager@agentix\nomp\nplugin\ninstall\nagentix-bridge@agentix\n/);
            assert.equal(run("remove").status, 0);
            await assert.rejects(readFile(join(cache, "new")), { code: "ENOENT" });
        }
    });
}
