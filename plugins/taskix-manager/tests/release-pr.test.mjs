import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { cp, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../../", import.meta.url));
async function fixture(t) {
    const dir = await mkdtemp(join(tmpdir(), "release-pr-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const files = execFileSync("git", ["ls-files"], { cwd: root, encoding: "utf8" }).trim().split("\n");
    for (const path of files.filter(p => /(^VERSION$|^Cargo\.(toml|lock)$|(^|\/)(package(-lock)?|plugin|manifest)\.json$)/.test(p)).concat(["scripts/plugin-version.mjs", "plugins/taskix-manager/version.mjs"])) {
        await mkdir(join(dir, path, ".."), { recursive: true });
        await cp(join(root, path), join(dir, path));
    }
    await writeFile(join(dir, "source.rs"), "original source\n");
    const git = (...args) => execFileSync("git", ["-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args], { cwd: dir, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
    git("init", "-b", "main");
    git("add", "."); git("commit", "-sm", "test: base");
    const base = git("rev-parse", "HEAD");
    execFileSync(process.execPath, [join(dir, "scripts/plugin-version.mjs"), "prepare", "1.2.3"], { cwd: dir });
    git("add", "."); git("commit", "-sm", "chore: release 1.2.3");
    return { dir, git, base, head: git("rev-parse", "HEAD"), branch: "release/1.2.3", event: "pull_request", sameRepository: true };
}

test("only_exact_prepared_release_changes_skip_tests", async t => {
    const { isReleaseOnly } = await import("../../../scripts/release-pr.mjs");
    const f = await fixture(t);
    assert.equal(await isReleaseOnly(f), true);
    for (const change of [{ branch: "feature/release" }, { branch: "release/1.2.4" }, { event: "push" }, { event: "workflow_dispatch" }, { sameRepository: false }, { head: f.base }]) {
        assert.equal(await isReleaseOnly({ ...f, ...change }), false, JSON.stringify(change));
    }
    assert.equal(f.git("status", "--porcelain"), "");
});

test("release_named_pr_with_code_or_dependency_changes_runs_tests", async t => {
    const { isReleaseOnly } = await import("../../../scripts/release-pr.mjs");
    for (const path of ["source.rs", "package.json", "Cargo.lock"]) {
        const f = await fixture(t);
        if (path === "package.json") {
            const value = JSON.parse(await readFile(join(f.dir, path), "utf8"));
            value.scripts = { postinstall: "unexpected code" };
            await writeFile(join(f.dir, path), JSON.stringify(value));
        } else await writeFile(join(f.dir, path), "\n# unrelated change\n", { flag: "a" });
        f.git("add", "."); f.git("commit", "-sm", "test: unrelated change");
        assert.equal(await isReleaseOnly({ ...f, head: f.git("rev-parse", "HEAD") }), false, path);
    }
});

test("every_test_job_depends_on_release_classification", async () => {
    const workflow = await readFile(join(root, ".github/workflows/tests.yml"), "utf8");
    assert.match(workflow, /  classify:/);
    for (const text of [workflow.replace(/\r\n/g, "\n"), workflow.replace(/\r?\n/g, "\r\n")]) {
        for (const id of ["backup", "plugins", "native-opencode", "bottles", "test", "windows-platform"]) {
            const job = text.replace(/\r\n/g, "\n").split(`\n  ${id}:\n`)[1].split(/\n  [\w-]+:\n/)[0];
            assert.match(job, /needs: classify/);
            assert.match(job, /if: needs.classify.outputs.run-tests == 'true'/);
        }
    }
});

test("every_ci_job_depends_on_release_classification", async () => {
    const workflow = (await readFile(join(root, ".github/workflows/ci.yml"), "utf8")).replace(/\r\n/g, "\n");
    const jobs = workflow.split("\njobs:\n")[1].split(/(?=^  [\w-]+:\n)/m).filter(text => text.trim());
    assert.equal(jobs.length, 4);
    const classifier = jobs.find(text => text.startsWith("  classify:\n"));
    assert.ok(classifier, "CI must verify the full release diff before skipping jobs");
    assert.match(classifier, /fetch-depth: 0/);
    assert.match(classifier, /run: node scripts\/release-pr\.mjs/);
    assert.match(classifier, /run-tests: \$\{\{ steps\.changes\.outputs\.run-tests \}\}/);
    for (const job of jobs.filter(text => text !== classifier)) {
        assert.match(job, /needs: classify/);
        assert.match(job, /if: needs\.classify\.outputs\.run-tests == 'true'/);
    }
});
