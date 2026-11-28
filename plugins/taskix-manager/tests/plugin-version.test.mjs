import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { cp, mkdtemp, readFile, readdir, rm, writeFile, mkdir } from "node:fs/promises";
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
    for (const crate of await readdir(join(root, "crates"), { withFileTypes: true })) {
        if (!crate.isDirectory()) continue;
        const path = join("crates", crate.name, "Cargo.toml");
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

test("prepare_preserves_the_aquamarine_compatibility_version", async t => {
    const dir = await fixture(t);
    const result = run(dir, "prepare", "1.2.3");
    assert.equal(result.status, 0, result.stderr);
    const lock = await readFile(join(dir, "Cargo.lock"), "utf8");
    assert.match(lock, /name = "aquamarine"\r?\nversion = "0\.6\.0"/);
    assert.match(lock, /name = "aquamarine-compat-macro"\r?\nversion = "1\.2\.3"/);
    assert.equal(run(dir, "check", "1.2.3").status, 0);
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

function workflowStep(workflow, name) {
    const step = workflow.split(`      - name: ${name}\n`)[1];
    assert.ok(step, `missing step ${name}`);
    const body = step.split(/\n(?:      - |  [a-z])/)[0];
    const inline = body.match(/        run: ([^|\n].*)/);
    return inline ? inline[1] : body.split("        run: |\n")[1].replace(/^          /gm, "");
}

async function releaseFixture(t) {
    const dir = await fixture(t);
    for (const path of ["scripts/plugin-version.mjs", "plugins/taskix-manager/version.mjs", ".github/scripts/release-version.sh", ".github/scripts/set-release-version.sh"]) {
        await mkdir(join(dir, path, ".."), { recursive: true });
        await cp(join(root, path), join(dir, path));
    }
    assert.equal(run(dir, "prepare", "1.2.3").status, 0);
    const git = (...args) => execFileSync("git", ["-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args], { cwd: dir, encoding: "utf8" });
    git("init", "-b", "main");
    git("add", ".");
    git("commit", "-sm", "test: development source");
    git("tag", "1.2.3");
    return { dir, git };
}

const shellStep = (dir, script, env = {}) => spawnSync("bash", ["--noprofile", "--norc", "-euo", "pipefail", "-c", script], {
    cwd: dir, encoding: "utf8", env: { ...process.env, BASH_ENV: "/dev/null", ENV: "/dev/null", ...env },
});

test("release_checks_committed_versions_for_push_and_dispatch_without_changing_git", { skip: process.platform === "win32" }, async t => {
    const workflow = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    for (const event of ["push", "workflow_dispatch"]) {
        const { dir, git } = await releaseFixture(t);
        const script = workflowStep(workflow, "Resolve release version").replaceAll("${{ github.event_name }}", event);
        const result = shellStep(dir, script, { INPUT_TAG: "1.2.3", PUSHED_TAG: event === "push" ? "1.2.3" : "main", GITHUB_OUTPUT: join(dir, "output") });
        assert.equal(result.status, 0, result.stderr);
        assert.match(await readFile(join(dir, "output"), "utf8"), /tag=1.2.3\nversion=1.2.3/);
        assert.equal(git("show", "1.2.3:VERSION").trim(), "1.2.3");
    }
});

test("release_resolution_rejects_inconsistent_source_metadata", { skip: process.platform === "win32" }, async t => {
    const { dir } = await releaseFixture(t);
    await writeFile(join(dir, manifests[3]), '{"version":"9.9.9"}\n');
    const workflow = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    const result = shellStep(dir, workflowStep(workflow, "Resolve release version").replaceAll("${{ github.event_name }}", "push"), { PUSHED_TAG: "1.2.3", GITHUB_OUTPUT: join(dir, "output") });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /9.9.9/);
    assert.equal((await readFile(join(dir, "VERSION"), "utf8")).trim(), "1.2.3");
});

test("release_resolution_rejects_invalid_tags_and_wrong_commits", { skip: process.platform === "win32" }, async t => {
    const { dir, git } = await releaseFixture(t);
    const workflow = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    const script = workflowStep(workflow, "Resolve release version").replaceAll("${{ github.event_name }}", "push");
    for (const tag of ["invalid", "v1.2.3", "1.2.3-rc.1"]) {
        git("tag", tag);
        const result = shellStep(dir, script, { PUSHED_TAG: tag, GITHUB_OUTPUT: join(dir, "output") });
        assert.notEqual(result.status, 0, tag);
        assert.equal((await readFile(join(dir, "VERSION"), "utf8")).trim(), "1.2.3");
    }
    git("commit", "--allow-empty", "-sm", "test: another commit");
    const result = shellStep(dir, script, { PUSHED_TAG: "1.2.3", GITHUB_OUTPUT: join(dir, "output") });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /does not point at the checked-out commit/);
});

test("release_build_checks_committed_metadata_and_packages_plugin_versions", { skip: process.platform === "win32" }, async t => {
    const { dir, git } = await releaseFixture(t);
    const workflow = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    const result = shellStep(dir, workflowStep(workflow, "Set release version"), { RELEASE_TAG: "1.2.3" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(run(dir, "check", "1.2.3").status, 0, "all metadata must match before packaging");
    assert.equal(git("show", "HEAD:VERSION").trim(), "1.2.3");
    await assertStampedArchive(dir, workflow);

    // Older tags without unified metadata still use the Cargo-only setter.
    await rm(join(dir, "VERSION"));
    const legacy = shellStep(dir, workflowStep(workflow, "Set release version"), { RELEASE_TAG: "1.2.4" });
    assert.equal(legacy.status, 0, legacy.stderr);
    assert.match(await readFile(join(dir, "Cargo.toml"), "utf8"), /version = "1.2.4"/);
    const lock = await readFile(join(dir, "Cargo.lock"), "utf8");
    assert.match(lock, /name = "aquamarine"\r?\nversion = "0\.6\.0"/);
    assert.match(lock, /name = "aquamarine-compat-macro"\r?\nversion = "1\.2\.4"/);
    assert.equal(JSON.parse(await readFile(join(dir, "package.json"))).version, "1.2.3");
});

async function assertStampedArchive(dir, workflow) {
    const target = "aarch64-apple-darwin";
    for (const file of ["README.md", "LICENSE", "docs/task-board.md", "docs/task-workflow-mechanisms.md", ...["agentix", "taskix"].flatMap(binary => [
        `config/${binary}.example.toml`, `completions/${binary}.bash`, `completions/_${binary}`, `completions/${binary}.fish`,
    ])]) {
        await mkdir(join(dir, file, ".."), { recursive: true });
        await cp(join(root, file), join(dir, file));
    }
    await mkdir(join(dir, `target/${target}/release`), { recursive: true });
    for (const binary of ["agentix", "taskix"]) await writeFile(join(dir, `target/${target}/release/${binary}`), "binary fixture");
    await writeFile(join(dir, "plugins/taskix-manager/untracked-secret"), "must not ship");
    const packed = shellStep(dir, workflowStep(workflow, "Package release binary"), { RELEASE_TAG: "1.2.3", TARGET: target, RUNNER_OS: "macOS" });
    assert.equal(packed.status, 0, packed.stderr);
    const archive = join(dir, `dist/taskix-1.2.3-${target}.tar.gz`);
    const files = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" });
    assert.doesNotMatch(files, /untracked-secret/);
    for (const file of manifests.filter(path => path.startsWith("plugins/taskix-manager/"))) {
        const manifest = execFileSync("tar", ["-xOf", archive, `./${file}`], { encoding: "utf8" });
        assert.equal(JSON.parse(manifest).version, "1.2.3", file);
    }
}

test("release_archive_uses_stamped_worktree_not_committed_plugin_manifests", { skip: process.platform === "win32" }, async t => {
    const { dir } = await releaseFixture(t);
    assert.equal(run(dir, "prepare", "1.2.3").status, 0);
    await assertStampedArchive(dir, await readFile(join(root, ".github/workflows/release.yml"), "utf8"));
});

test("missing_worktree_status_cannot_report_current", async () => {
    const { compareUpdate } = await import("../version.mjs");
    assert.equal(compareUpdate({ version: "1.2.3", tag: "1.2.3", dirty: null }, "1.2.3"), "unknown");
});

test("prepare_supports_build_metadata_development_versions", async t => {
    const dir = await fixture(t);
    assert.equal(run(dir, "prepare", "0.8.0+dev").status, 0);
    assert.equal(run(dir, "check").status, 0);
    for (const path of manifests) assert.equal(JSON.parse(await readFile(join(dir, path))).version, "0.8.0+dev");
    assert.notEqual(run(dir, "check", "0.8.0+dev").status, 0, "explicit release tags must be stable");
    assert.equal(run(dir, "prepare", "0.9.0").status, 0);
    assert.equal(run(dir, "check", "0.9.0").status, 0);
});

test("release_rejects_development_metadata_instead_of_stamping_it", { skip: process.platform === "win32" }, async t => {
    const { dir } = await releaseFixture(t);
    assert.equal(run(dir, "prepare", "0.0.0-dev").status, 0);
    const workflow = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    for (const name of ["Resolve release version", "Set release version"]) {
        const result = shellStep(dir, workflowStep(workflow, name).replaceAll("${{ github.event_name }}", "push"), { PUSHED_TAG: "1.2.3", RELEASE_TAG: "1.2.3", GITHUB_OUTPUT: join(dir, "output") });
        assert.notEqual(result.status, 0, name);
        assert.equal((await readFile(join(dir, "VERSION"), "utf8")).trim(), "0.0.0-dev");
    }
});

test("development_pr_pushes_only_a_version_branch_and_preserves_release_tag", { skip: process.platform === "win32" }, async t => {
    const { createDevelopmentPr } = await import("../../../scripts/development-pr.mjs");
    const { dir, git } = await releaseFixture(t);
    const remote = await mkdtemp(join(tmpdir(), "development-remote-"));
    t.after(() => rm(remote, { recursive: true, force: true }));
    execFileSync("git", ["init", "--bare", remote]);
    git("remote", "add", "origin", remote);
    git("push", "origin", "main", "--tags");
    const original = git("rev-parse", "HEAD").trim();
    let prs = [], creates = 0;
    const execute = (command, args) => {
        if (command === "gh") {
            if (args[1] === "list") return JSON.stringify(prs);
            creates++;
            prs = [{ url: "https://example.com/pr/1" }];
            return prs[0].url;
        }
        if (command === "git") return git(...args);
        return execFileSync(process.execPath, args, { cwd: dir, encoding: "utf8" });
    };
    createDevelopmentPr("1.2.3", { run: execute });
    assert.equal(git("show", "origin/development/1.2.3:VERSION").trim(), "1.2.3+dev");
    assert.equal(git("rev-parse", "origin/main").trim(), original);
    assert.equal(git("show", "1.2.3:VERSION").trim(), "1.2.3");
    assert.match(git("log", "-1", "--format=%B"), /Signed-off-by:/);
    createDevelopmentPr("1.2.3", { run: execute });
    assert.equal(creates, 1);
});
