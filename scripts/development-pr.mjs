import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const execute = (command, args) => execFileSync(command, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();

// Run only in a disposable CI checkout. Never push main or rewrite a remote branch.
export function createDevelopmentPr(tag, { run = execute } = {}) {
    if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag)) throw new Error("Expected a numeric stable release tag");
    const git = (...args) => run("git", args).trim();
    const branch = `development/${tag}`;
    const version = `${tag}+dev`;
    git("fetch", "origin", "main", "--tags");
    if (git("show", "origin/main:VERSION") !== tag) return "Skipped: main already has another version";
    git("merge-base", "--is-ancestor", `refs/tags/${tag}`, "origin/main");
    const base = git("rev-parse", "origin/main");
    const existing = JSON.parse(run("gh", ["pr", "list", "--base", "main", "--head", branch, "--state", "all", "--json", "url"]));
    if (existing.length) return `Existing PR: ${existing[0].url}`;
    if (git("ls-remote", "--heads", "origin", `refs/heads/${branch}`)) {
        git("fetch", "origin", `${branch}:refs/remotes/origin/${branch}`);
        if (git("show", `origin/${branch}:VERSION`) !== version) throw new Error("Existing development branch has an unexpected version");
        git("merge-base", "--is-ancestor", base, `origin/${branch}`);
    } else {
        git("switch", "--create", branch, "origin/main");
        run("node", ["scripts/plugin-version.mjs", "check", tag]);
        run("node", ["scripts/plugin-version.mjs", "prepare", version]);
        run("node", ["scripts/plugin-version.mjs", "check"]);
        git("add", "--update");
        git("-c", "user.name=github-actions[bot]", "-c", "user.email=41898282+github-actions[bot]@users.noreply.github.com", "commit", "-s", "-m", `chore: start ${version} development`);
        git("fetch", "origin", "main");
        if (git("rev-parse", "origin/main") !== base) return "Skipped: main moved; rerun against its current version";
        git("push", "origin", `HEAD:refs/heads/${branch}`);
    }
    return run("gh", ["pr", "create", "--base", "main", "--head", branch,
        "--title", `chore: start ${version} development`,
        "--body", `Release ${tag} has been published. Set component versions to ${version} without choosing the next release number.\n\nValidation: component versions checked before and after the update.\n\nMerge only while main still has version ${tag}. This PR does not change the release tag.`]);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    try { console.log(createDevelopmentPr(process.argv[2])); }
    catch (error) { console.error(error.stderr?.toString() || error.message); process.exitCode = 1; }
}
