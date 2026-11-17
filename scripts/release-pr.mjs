import { execFileSync } from "node:child_process";
import { appendFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Prove the entire PR tree is exactly a prepared release or development version.
// A branch name or a file allowlist alone cannot exclude changes inside manifests.
export async function isReleaseOnly({ dir = process.cwd(), base, head, branch, event, sameRepository }) {
    const match = /^(release|development)\/((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))$/.exec(branch ?? "");
    if (event !== "pull_request" || !sameRepository || !match) return false;
    const development = match[1] === "development";
    const version = match[2] + (development ? "+dev" : "");
    if (![base, head].every(sha => /^[0-9a-f]{40}$/.test(sha ?? ""))) return false;
    const git = (...args) => execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], { cwd: dir, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
    const temporary = await mkdtemp(join(tmpdir(), "release-only-"));
    const worktree = join(temporary, "source");
    let added = false;
    try {
        const ancestor = git("merge-base", base, head);
        const baseVersion = git("show", `${ancestor}:VERSION`);
        if (baseVersion === version || (development && baseVersion !== match[2])) return false;
        git("worktree", "add", "--detach", worktree, ancestor);
        added = true;
        const script = join(worktree, "scripts/plugin-version.mjs");
        for (const args of [["check"], ["prepare", version], development ? ["check"] : ["check", version]]) {
            execFileSync(process.execPath, [script, ...args], { cwd: worktree, stdio: "pipe" });
        }
        git("-C", worktree, "add", "--update");
        return git("-C", worktree, "write-tree") === git("rev-parse", `${head}^{tree}`);
    } catch (error) {
        console.error(`Cannot prove release-only changes; running tests: ${error.message.split("\n")[0]}`);
        return false;
    } finally {
        if (added) git("worktree", "remove", "--force", worktree);
        await rm(temporary, { recursive: true, force: true });
    }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    const skip = await isReleaseOnly({
        base: process.env.PR_BASE_SHA, head: process.env.PR_HEAD_SHA,
        branch: process.env.PR_HEAD_REF, event: process.env.GITHUB_EVENT_NAME,
        sameRepository: Boolean(process.env.GITHUB_REPOSITORY) && process.env.PR_HEAD_REPO === process.env.GITHUB_REPOSITORY,
    });
    const output = `run-tests=${!skip}\n`;
    if (process.env.GITHUB_OUTPUT) await appendFile(process.env.GITHUB_OUTPUT, output);
    console.log(skip ? "Release-only PR verified; test jobs will be skipped." : "Run the test jobs.");
}
