import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";

async function scenario({ version = "1.2.3", existing = [], branch = false, changed = false } = {}) {
    const { createDevelopmentPr } = await import("../../../scripts/development-pr.mjs");
    const calls = [];
    const run = (command, args) => {
        calls.push([command, ...args]);
        if (args[0] === "show") return args[1].includes("development/") ? "1.2.3+dev\n" : version;
        if (args[0] === "rev-parse") return changed && calls.filter(c => c[1] === "rev-parse").length > 1 ? "new" : "base";
        if (args[0] === "ls-remote") return branch ? "sha refs/heads/development/1.2.3" : "";
        if (args[0] === "pr" && args[1] === "list") return JSON.stringify(existing);
        return "";
    };
    const result = createDevelopmentPr("1.2.3", { run });
    return { calls, result };
}

test("development_pr_creates_signed_version_commit_and_one_pr", async () => {
    const { calls } = await scenario();
    assert.ok(calls.some(c => c[0] === "node" && c.includes("1.2.3+dev")));
    assert.ok(calls.some(c => c.includes("commit") && c.includes("-s")));
    assert.ok(calls.some(c => c[1] === "push" && c.includes("HEAD:refs/heads/development/1.2.3")));
    assert.equal(calls.filter(c => c[1] === "pr" && c[2] === "create").length, 1);
});

test("development_pr_skips_newer_or_already_development_main", async () => {
    for (const version of ["1.2.4", "1.2.3+dev", "0.0.0-dev"]) {
        const { calls } = await scenario({ version });
        assert.ok(!calls.some(c => ["push", "commit", "switch", "pr"].includes(c[1])));
    }
});

test("development_pr_reruns_reuse_existing_pr_or_pushed_branch", async () => {
    const existing = await scenario({ existing: [{ url: "https://example.com/pr/1" }] });
    assert.ok(!existing.calls.some(c => ["push", "commit"].includes(c[1])));
    const interrupted = await scenario({ branch: true });
    assert.ok(!interrupted.calls.some(c => ["push", "commit"].includes(c[1])));
    assert.equal(interrupted.calls.filter(c => c[1] === "pr" && c[2] === "create").length, 1);
});

test("development_pr_aborts_if_main_moves_before_push", async () => {
    const { calls } = await scenario({ changed: true });
    assert.ok(!calls.some(c => c[1] === "push" || c[1] === "pr" && c[2] === "create"));
});

test("development_pr_only_runs_after_successful_release_and_homebrew_publication", async () => {
    const workflow = await readFile(new URL("../../../.github/workflows/release.yml", import.meta.url), "utf8");
    const job = workflow.split("  prepare-development:")[1];
    assert.ok(job);
    assert.match(job, /- publish-release/);
    assert.match(job, /- publish-homebrew/);
    assert.doesNotMatch(job, /always\(\)/);
    assert.match(job, /pull-requests: write/);
});
