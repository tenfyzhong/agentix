import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../../../", import.meta.url));
const hosts = ["codex", "pi", "omp", "claude"];
const removals = {
    codex: ["codex plugin remove taskix-manager@agentix", "codex plugin marketplace remove agentix"],
    pi: ["pi remove .", "pi remove git:github.com/tenfyzhong/agentix", "pi remove git:github.com/tenfyzhong/agentix@main"],
    omp: ["omp plugin uninstall agentix-plugins", "omp plugin uninstall taskix-manager@agentix", "omp plugin uninstall agentix-bridge@agentix", "omp plugin marketplace remove agentix"],
    claude: ["claude plugin uninstall taskix-manager@agentix", "claude plugin uninstall agentix-bridge@agentix", "claude plugin marketplace remove agentix"],
};
function installs(host, remote) {
    if (host === "pi") return [`pi install ${remote ? "git:github.com/tenfyzhong/agentix@main" : "."}`];
    const source = remote ? (host === "codex" ? "tenfyzhong/agentix --ref main" : host === "claude" ? "tenfyzhong/agentix@main" : "tenfyzhong/agentix") : "./";
    return [
        `${host} plugin marketplace add ${source}`,
        `${host} plugin ${host === "codex" ? "add" : "install"} taskix-manager@agentix`,
        ...(host === "codex" ? [] : [`${host} plugin install agentix-bridge@agentix`]),
    ];
}

async function fixture(t) {
    const directory = await mkdtemp(join(tmpdir(), "agentix-dev-install-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const bin = join(directory, "bin");
    await mkdir(bin);
    for (const command of ["cargo", "cp", "rm", "taskix", "codex", "claude", "pi", "omp"]) {
        const path = join(bin, command);
        await writeFile(path, `#!/bin/sh
echo "${command} $*" >> "$INSTALL_FIXTURE_DIR/calls"
if [ "${command}" = "$INSTALL_FAIL_HOST" ]; then
    case "$*" in
        "plugin add "*|"plugin install "*|"install "*) [ "$INSTALL_FAIL" != "install" ] || exit 8 ;;
        "plugin remove "*|"plugin uninstall "*|"remove "*) [ "$INSTALL_FAIL" != "remove" ] || exit 7 ;;
    esac
fi
exit 0
`);
        await chmod(path, 0o755);
    }
    return {
        calls: async () => (await readFile(join(directory, "calls"), "utf8")).trim().split("\n"),
        run: (target, failure = "", host = "codex") => spawnSync("make", [...(Array.isArray(target) ? target : target.split(" ")), "CARGO=cargo"], {
            cwd: repository,
            env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, INSTALL_FIXTURE_DIR: directory, INSTALL_FAIL: failure, INSTALL_FAIL_HOST: host },
            encoding: "utf8",
        }),
    };
}


test("remove_plugin_cleans_all_four_hosts", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run("remove-plugin").status, 0);
    assert.deepEqual(await f.calls(), hosts.flatMap(host => removals[host]));
});

for (const target of ["plugin", "plugin SOURCE=main", "plugin SOURCE=local"]) {
    test(`${target}_installs_all_hosts`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const remote = ["plugin", "plugin SOURCE=main"].includes(target);
        const expected = [...hosts.flatMap(host => removals[host]), ...hosts.flatMap(host => installs(host, remote))];
        for (let attempt = 1; attempt <= 2; attempt++) {
            const result = f.run(target);
            assert.equal(result.status, 0, result.stderr);
            assert.deepEqual(await f.calls(), Array.from({ length: attempt }, () => expected).flat());
        }
    });
}

for (const host of hosts) {
    test(`select_${host}_and_propagate_install_failure`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const args = ["plugin", `HOSTS=${host}`];
        assert.equal(f.run(args, "remove", host).status, 0);
        assert.deepEqual(await f.calls(), [...removals[host], ...installs(host, true)]);
        assert.notEqual(f.run(args, "install", host).status, 0);
    });
}
for (const args of [["plugin", "SOURCE=typo"], ["plugin", "HOSTS=codex typo"], ["remove-plugin", "HOSTS="]]) {
    test(`invalid_options_fail_before_cleanup_${args.join("_")}`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        assert.notEqual(f.run(args).status, 0);
        await assert.rejects(f.calls(), { code: "ENOENT" });
    });
}

test("missing_selected_cli_fails_before_cleanup", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.notEqual(f.run(["plugin", "CLAUDE=/nonexistent/agentix-test-claude"]).status, 0);
    await assert.rejects(f.calls(), { code: "ENOENT" });
});

test("multiple_selected_hosts_leave_other_hosts_untouched", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run(["plugin", "SOURCE=main", "HOSTS=pi claude"]).status, 0);
    assert.deepEqual(await f.calls(), [
        ...removals.pi, ...removals.claude, ...installs("pi", true), ...installs("claude", true),
    ]);
});

test("removed_aliases_fail_without_changing_plugins", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    for (const target of ["dev-test", "prod-test"]) {
        const result = f.run(target);
        assert.notEqual(result.status, 0);
        assert.match(result.stderr, /No rule to make target/);
    }
    await assert.rejects(f.calls(), { code: "ENOENT" });
});
