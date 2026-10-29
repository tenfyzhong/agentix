import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../../../", import.meta.url));
const hosts = ["codex", "pi", "omp", "claude", "opencode"];
const branch = "feature/plugin-test";
const cleanupHosts = hosts.filter(host => host !== "opencode");
const removals = {
    codex: ["codex plugin remove taskix-manager@agentix", "codex plugin marketplace remove agentix"],
    pi: ["pi remove .", "pi remove git:github.com/tenfyzhong/agentix", "pi remove git:github.com/tenfyzhong/agentix@main", `pi remove git:github.com/tenfyzhong/agentix@${branch}`],
    omp: ["omp plugin uninstall agentix-plugins", "omp plugin uninstall taskix-manager@agentix", "omp plugin uninstall agentix-bridge@agentix", "omp plugin marketplace remove agentix", "node scripts/omp-plugin.mjs remove"],
    claude: ["claude plugin uninstall taskix-manager@agentix", "claude plugin uninstall agentix-bridge@agentix", "claude plugin marketplace remove agentix"],
    opencode: ["node scripts/opencode-plugin.mjs remove"],
};
function installs(host) {
    if (host === "opencode") return [`node scripts/opencode-plugin.mjs install --remote ${branch}`];
    if (host === "omp") return [`node scripts/omp-plugin.mjs install ${branch}`];
    if (host === "pi") return [`pi install git:github.com/tenfyzhong/agentix@${branch}`];
    const source = host === "codex" ? `tenfyzhong/agentix --ref ${branch}` : `tenfyzhong/agentix@${branch}`;
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
    for (const command of ["cargo", "cp", "rm", "taskix", "codex", "claude", "pi", "omp", "opencode", "node", "git"]) {
        const path = join(bin, command);
        await writeFile(path, `#!/bin/sh
if [ "${command}" = "git" ]; then
    case "$*" in
        "symbolic-ref --quiet --short HEAD") [ "$INSTALL_DETACHED" != "1" ] || exit 1; echo "\${INSTALL_BRANCH:-${branch}}"; exit 0 ;;
        "ls-remote "*) [ "$INSTALL_MISSING_BRANCH" != "1" ] || exit 2; exit 0 ;;
    esac
fi
echo "${command} $*" >> "$INSTALL_FIXTURE_DIR/calls"
if [ "${command}" = "$INSTALL_FAIL_HOST" ] || { [ "${command}" = "node" ] && [ "$INSTALL_FAIL_HOST" = "opencode" -o "$INSTALL_FAIL_HOST" = "omp" ]; }; then
    case "$*" in
        "plugin add "*|"plugin install "*|"install "*|"scripts/opencode-plugin.mjs install "*|"scripts/omp-plugin.mjs install "*) [ "$INSTALL_FAIL" != "install" ] || exit 8 ;;
        "plugin remove "*|"plugin uninstall "*|"remove "*) [ "$INSTALL_FAIL" != "remove" ] || exit 7 ;;
    esac
fi
exit 0
`);
        await chmod(path, 0o755);
    }
    return {
        calls: async () => (await readFile(join(directory, "calls"), "utf8")).trim().split("\n"),
        run: (target, failure = "", host = "codex", extraEnv = {}) => spawnSync("make", ["CARGO=cargo", "NODE=node", ...(Array.isArray(target) ? target : target.split(" "))], {
            cwd: repository,
            env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, XDG_CACHE_HOME: join(directory, "cache"), OPENCODE_CONFIG_DIR: join(directory, "opencode"), INSTALL_FIXTURE_DIR: directory, INSTALL_FAIL: failure, INSTALL_FAIL_HOST: host, ...extraEnv },
            encoding: "utf8",
        }),
    };
}


test("remove_plugin_cleans_all_five_hosts", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run("remove-plugin").status, 0);
    assert.deepEqual(await f.calls(), hosts.flatMap(host => removals[host]));
});

for (const target of ["plugin"]) {
    test(`${target}_installs_all_hosts`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const expected = [...cleanupHosts.flatMap(host => removals[host]), ...hosts.flatMap(host => installs(host))];
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
        assert.deepEqual(await f.calls(), [
            ...(host === "opencode" ? [] : removals[host]), ...installs(host),
        ]);
        assert.notEqual(f.run(args, "install", host).status, 0);
    });
}
for (const args of [["plugin", "SOURCE=typo"], ["plugin", "SOURCE=local"], ["plugin", "SOURCE=main"], ["plugin", "HOSTS=codex typo"], ["remove-plugin", "HOSTS="]]) {
    test(`invalid_options_fail_before_cleanup_${args.join("_")}`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        assert.notEqual(f.run(args).status, 0);
        await assert.rejects(f.calls(), { code: "ENOENT" });
    });
}

test("missing_selected_cli_fails_before_cleanup", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.notEqual(f.run(["plugin", "CLAUDE=/nonexistent/agentix-test-claude"]).status, 0);
    assert.notEqual(f.run(["plugin", "HOSTS=opencode", "OPENCODE=/nonexistent/agentix-test-opencode"]).status, 0);
    await assert.rejects(f.calls(), { code: "ENOENT" });
});

for (const command of ["OPENCODE", "NODE"]) {
    test(`missing_default_${command.toLowerCase()}_fails_before_cleanup`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        assert.notEqual(f.run(["plugin", `${command}=/nonexistent/agentix-test-${command.toLowerCase()}`]).status, 0);
        await assert.rejects(f.calls(), { code: "ENOENT" });
    });
}

test("multiple_selected_hosts_leave_other_hosts_untouched", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.equal(f.run(["plugin", "HOSTS=pi claude"]).status, 0);
    assert.deepEqual(await f.calls(), [
        ...removals.pi, ...removals.claude, ...installs("pi"), ...installs("claude"),
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

for (const flag of ["INSTALL_DETACHED", "INSTALL_MISSING_BRANCH"]) {
    test(`${flag}_fails_before_cleanup`, { skip: process.platform === "win32" }, async t => {
        const f = await fixture(t);
        const result = f.run("plugin", "", "codex", { [flag]: "1" });
        assert.notEqual(result.status, 0);
        await assert.rejects(f.calls(), { code: "ENOENT" });
    });
}

test("main_branch_installs_remote_main_without_duplicate_removal", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    const result = f.run("plugin", "", "codex", { INSTALL_BRANCH: "main" });
    assert.equal(result.status, 0, result.stderr);
    const expected = [...cleanupHosts.flatMap(host => removals[host]).filter(call => !call.endsWith(`@${branch}`)), ...hosts.flatMap(installs).map(call => call.replaceAll(branch, "main"))];
    assert.deepEqual(await f.calls(), expected);
});

test("detached_head_still_allows_removal", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    const result = f.run("remove-plugin", "", "codex", { INSTALL_DETACHED: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), hosts.flatMap(host => removals[host]).filter(call => !call.endsWith(`@${branch}`)));
});

test("omp_requires_node_before_cleanup", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    assert.notEqual(f.run(["plugin", "HOSTS=omp", "NODE=/nonexistent/node"]).status, 0);
    await assert.rejects(f.calls(), { code: "ENOENT" });
});

test("npm_node_environment_cannot_bypass_installation_fixture", { skip: process.platform === "win32" }, async t => {
    const f = await fixture(t);
    const result = f.run("remove-plugin", "", "codex", { NODE: process.execPath });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), hosts.flatMap(host => removals[host]));
});
