import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const repository = new URL("../../../", import.meta.url);
const executable = `version-probe${process.platform === "win32" ? ".exe" : ""}`;
const run = (cwd, command, args) => execFileSync(command, args, {
    cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, CARGO_TARGET_DIR: join(cwd, "target"), CARGO_NET_OFFLINE: "true", GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(cwd, ".gitconfig") },
}).trim();
const git = (cwd, ...args) => run(cwd, "git", args);

async function fixture(t, version = "0.0.0-dev") {
    const root = await mkdtemp(join(tmpdir(), "agentix-version-"));
    t.after(() => rm(root, { recursive: true, force: true }));
    await mkdir(join(root, "crates/probe/src"), { recursive: true });
    // Git for Windows rejects the device path returned by node:os devNull.
    await writeFile(join(root, ".gitconfig"), "");
    await writeFile(join(root, "Cargo.toml"), '[workspace]\nmembers = ["crates/probe"]\nresolver = "3"\n');
    await writeFile(join(root, "crates/probe/Cargo.toml"), `[package]\nname = "version-probe"\nversion = "${version}"\nedition = "2024"\nbuild = "../../build.rs"\n`);
    await writeFile(join(root, "crates/probe/src/main.rs"), 'fn main() { println!("{}", option_env!("AGENTIX_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))); }\n');
    const script = await readFile(new URL("build.rs", repository), "utf8");
    await writeFile(join(root, "build.rs"), script);
    await writeFile(join(root, ".gitignore"), "/target/\n/installed/\n");
    run(root, "cargo", ["generate-lockfile"]);
    git(root, "init", "-q", "-b", "fixture");
    git(root, "config", "user.email", "test@example.com");
    git(root, "config", "user.name", "Version Test");
    git(root, "add", ".");
    git(root, "-c", "commit.gpgsign=false", "commit", "-s", "-qm", "Initial fixture");
    return root;
}

function build(root, release = false) {
    run(root, "cargo", ["build", "--quiet", ...(release ? ["--release"] : [])]);
    return run(root, join(root, "target", release ? "release" : "debug", executable), []);
}
const expected = root => `0.0.0-dev+${git(root, "rev-parse", "--short=12", "HEAD")}`;

test("development builds refresh Git versions for unstaged, staged, untracked and committed changes", async t => {
    const root = await fixture(t);
    assert.equal(build(root), expected(root));
    await writeFile(join(root, "crates/probe/src/main.rs"), '\n', { flag: "a" });
    assert.equal(build(root), `${expected(root)}.dirty`);
    git(root, "add", ".");
    assert.equal(build(root), `${expected(root)}.dirty`);
    git(root, "-c", "commit.gpgsign=false", "commit", "-s", "-qm", "Source change");
    assert.equal(build(root), expected(root));
    await writeFile(join(root, "new-source.rs"), "// untracked source\n");
    assert.equal(build(root), `${expected(root)}.dirty`);
    await rm(join(root, "new-source.rs"));
    assert.equal(build(root), expected(root));
    git(root, "-c", "commit.gpgsign=false", "commit", "-s", "--allow-empty", "-qm", "New HEAD without source changes");
    assert.equal(build(root), expected(root));
    git(root, "checkout", "--detach", "HEAD~1");
    assert.equal(build(root), expected(root));
});

test("release profile and Homebrew-style cargo install preserve development Git metadata", async t => {
    const root = await fixture(t);
    assert.equal(build(root, true), expected(root));
    run(root, "cargo", ["install", "--path", "crates/probe", "--root", "installed", "--locked", "--offline"]);
    assert.equal(run(root, join(root, "installed/bin", executable), []), expected(root));
});

test("linked Git worktrees resolve their own HEAD and dirty state", async t => {
    const root = await fixture(t);
    const worktree = `${root}-worktree`;
    t.after(() => rm(worktree, { recursive: true, force: true }));
    git(root, "worktree", "add", "--detach", worktree);
    assert.equal(build(worktree), expected(worktree));
    await writeFile(join(worktree, "untracked.rs"), "// dirty\n");
    assert.equal(build(worktree), `${expected(worktree)}.dirty`);
    assert.equal(build(root), expected(root));
});

test("stable versions stay exact even with modified release metadata", async t => {
    const root = await fixture(t, "1.2.3");
    await writeFile(join(root, "untracked.rs"), "// dirty\n");
    assert.equal(build(root, true), "1.2.3");
});

test("development source archives explicitly report unavailable Git metadata", async t => {
    const root = await fixture(t);
    await rm(join(root, ".git"), { recursive: true, force: true });
    assert.equal(build(root), "0.0.0-dev+unknown");
});
