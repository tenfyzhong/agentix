import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { cp, mkdtemp, readFile, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const script = join(root, "scripts/plugin-version.mjs");
const manifests = ["package.json", "plugins/taskix-manager/package.json", "plugins/agentix-bridge/package.json",
    "plugins/taskix-manager/.codex-plugin/plugin.json", "plugins/taskix-manager/.claude-plugin/plugin.json",
    "plugins/taskix-manager/.antigravity-plugin/plugin.json", "plugins/agentix-bridge/.claude-plugin/plugin.json",
    "plugins/agentix-bridge/.antigravity-plugin/plugin.json", "plugins/taskix-manager/obsidian/taskix-sync/manifest.json"];
async function fixture(t) {
    const dir = await mkdtemp(join(tmpdir(), "plugin-version-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    for (const path of [...manifests, "package-lock.json", "plugins/taskix-manager/package-lock.json", "Cargo.toml", "Cargo.lock"]) {
        await mkdir(join(dir, path, ".."), { recursive: true });
        await cp(join(root, path), join(dir, path));
    }
    await writeFile(join(dir, "VERSION"), "0.0.0-dev\n");
    return dir;
}
function run(dir, ...args) {
    return spawnSync(process.execPath, [script, ...args], { cwd: dir, encoding: "utf8" });
}

test("prepare_aligns_all_manifests_and_workspace_locks_without_changing_dependencies", async t => {
    const dir = await fixture(t);
    const before = JSON.parse(await readFile(join(dir, "package-lock.json")));
    const result = run(dir, "prepare", "1.2.3");
    assert.equal(result.status, 0, result.stderr);
    for (const path of manifests) assert.equal(JSON.parse(await readFile(join(dir, path))).version, "1.2.3", path);
    const after = JSON.parse(await readFile(join(dir, "package-lock.json")));
    assert.equal(after.version, "1.2.3");
    for (const path of ["", "plugins/taskix-manager", "plugins/agentix-bridge"]) assert.equal(after.packages[path].version, "1.2.3");
    for (const [path, value] of Object.entries(before.packages)) {
        if (path.startsWith("node_modules/")) assert.deepEqual(after.packages[path], value);
    }
    assert.match(await readFile(join(dir, "Cargo.toml"), "utf8"), /version = "1.2.3"/);
    assert.equal(run(dir, "check", "1.2.3").status, 0);
    assert.notEqual(run(dir, "check", "1.2.4").status, 0);
    await writeFile(join(dir, manifests[3]), '{"version":"9.9.9"}\n');
    assert.notEqual(run(dir, "check", "1.2.3").status, 0);
});

test("prepare_rejects_invalid_versions_and_malformed_inputs_before_writing", async t => {
    const dir = await fixture(t);
    const before = await readFile(join(dir, "package.json"), "utf8");
    for (const version of ["latest", "01.2.3", "1.2", "1.2.3-", "v1.2.3"]) assert.notEqual(run(dir, "prepare", version).status, 0);
    await writeFile(join(dir, manifests.at(-1)), "broken");
    assert.notEqual(run(dir, "prepare", "1.2.3").status, 0);
    assert.equal(await readFile(join(dir, "package.json"), "utf8"), before);
});

test("repository_versions_are_aligned", () => {
    const result = run(root, "check");
    assert.equal(result.status, 0, result.stderr);
});

test("status_reads_actual_plugin_and_git_identity_and_never_infers_missing_commit", async t => {
    const dir = await fixture(t);
    const statusScript = join(root, "plugins/taskix-manager/version.mjs");
    const status = () => JSON.parse(execFileSync(process.execPath, [statusScript, "--path", join(dir, "plugins/taskix-manager"), "--json"], { encoding: "utf8" }));
    assert.equal(status().commit, null);
    assert.equal(status().update, "unchecked");
    const git = (...args) => execFileSync("git", ["-c", "core.hooksPath=" + join(dir, "empty-hooks"), ...args], { cwd: dir, stdio: "pipe" });
    git("init", "-b", "feature/version");
    git("add", ".");
    git("-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "commit", "-sm", "test: fixture");
    assert.equal(status().branch, "feature/version");
    assert.equal(status().commit, git("rev-parse", "HEAD").toString().trim());
    assert.equal(status().dirty, false);
    await writeFile(join(dir, "plugins/taskix-manager/change.txt"), "dirty");
    assert.equal(status().dirty, true);
    await rm(join(dir, "plugins/taskix-manager/change.txt"));
    assert.equal(run(dir, "prepare", "1.2.3").status, 0);
    git("add", ".");
    git("-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "commit", "-sm", "test: release");
    git("tag", "1.2.3");
    git("checkout", "--detach", "1.2.3");
    assert.equal(status().tag, "1.2.3");
    assert.equal(status().branch, null);
    assert.equal(status().dirty, false);
});

test("update_comparison_handles_stable_development_and_unavailable_sources", async () => {
    const { compareUpdate } = await import("../version.mjs");
    assert.equal(compareUpdate({ version: "1.2.3", tag: "1.2.3", dirty: false }, "1.2.4"), "available");
    assert.equal(compareUpdate({ version: "1.2.3", tag: "1.2.3", dirty: false }, "1.2.3"), "current");
    assert.equal(compareUpdate({ version: "1.3.0", tag: "1.3.0", dirty: false }, "1.2.3"), "ahead");
    assert.equal(compareUpdate({ version: "1.2.3" }, "1.2.3"), "unknown");
    assert.equal(compareUpdate({ branch: "main", commit: "abc", dirty: false }, "1.2.3", "def"), "available");
    assert.equal(compareUpdate({ branch: "main", commit: "abc", dirty: false }, "1.2.3", "abc"), "current");
    assert.equal(compareUpdate({ branch: "main", commit: "abc", dirty: true }, "1.2.3", "abc"), "modified");
});

test("latest_release_resolution_and_network_failures_are_explicit", async () => {
    const { latestStable, checkUpdates } = await import("../version.mjs");
    const reply = value => async () => ({ ok: true, json: async () => value });
    assert.equal(await latestStable(reply({ tag_name: "1.2.3" })), "1.2.3");
    await assert.rejects(latestStable(reply({ tag_name: "1.2.3-rc.1" })), /stable/);
    const status = await checkUpdates({ version: "1.2.3", tag: "1.2.3" }, async () => { throw new Error("offline"); });
    assert.equal(status.update, "unknown");
    assert.equal(status.error, "offline");
});

test("branch_updates_use_encoded_refs_and_never_check_foreign_repositories", async () => {
    const { checkUpdates } = await import("../version.mjs");
    const sha = "a".repeat(40), urls = [];
    const fetcher = async url => {
        urls.push(url);
        return { ok: true, json: async () => url.endsWith("releases/latest") ? { tag_name: "1.2.3" } : { sha } };
    };
    const installed = { version: "0.0.0-dev", branch: "feature/test", commit: sha, source: "git@github.com:tenfyzhong/agentix.git", dirty: false };
    assert.equal((await checkUpdates(installed, fetcher)).update, "current");
    assert.ok(urls[1].endsWith("commits/feature%2Ftest"));
    urls.length = 0;
    assert.equal((await checkUpdates({ ...installed, source: "https://example.org/other.git" }, fetcher)).update, "unknown");
    assert.equal(urls.length, 1);
});

test("release_validation_precedes_build_and_publish_and_ci_checks_versions", async () => {
    const release = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    const resolve = release.split("  prepare-homebrew:")[0];
    assert.match(resolve, /node scripts\/plugin-version\.mjs check "\$release_tag"/);
    assert.match(await readFile(join(root, ".github/workflows/tests.yml"), "utf8"), /node scripts\/plugin-version\.mjs check/);
});

test("missing_worktree_status_cannot_report_current", async () => {
    const { compareUpdate } = await import("../version.mjs");
    assert.equal(compareUpdate({ version: "1.2.3", tag: "1.2.3", dirty: null }, "1.2.3"), "unknown");
});
