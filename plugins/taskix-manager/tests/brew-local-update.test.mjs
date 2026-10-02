import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
test("real_homebrew_local_update_handles_head_links_with_stable_opt_and_preserves_links_on_failure", {
    skip: process.platform === "win32" || process.env.AGENTIX_TEST_HOMEBREW !== "1",
    timeout: 300_000,
}, async () => {
    const dir = await realpath(await mkdtemp(join(tmpdir(), "brew-local-update-")));
    const source = join(dir, "local source");
    const name = `taskix-link-fixture-${process.pid}`;
    const tap = `codex-fixture/local-update-${process.pid}`;
    const env = {...process.env, HOMEBREW_NO_AUTO_UPDATE: "1", HOMEBREW_NO_INSTALL_CLEANUP: "1", HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK: "1", HOMEBREW_AGENTIX_LOCAL_SOURCE: "", HOMEBREW_AGENTIX_LOCAL_PROFILE: "", HOMEBREW_AGENTIX_LOCAL_TARGET_DIR: "", HOMEBREW_CACHE: join(dir, "cache")};
    const brew = (...args) => execFileSync("brew", args, {env, encoding: "utf8", timeout: 90_000});
    let tapPath;
    try {
        const productionTap = process.env.AGENTIX_TEST_TAP_REPOSITORY || brew("--repository", "tenfyzhong/tap").trim();
        tapPath = brew("--repository", tap).trim();
        for (const path of ["Formula", "lib"]) await mkdir(join(tapPath, path), {recursive: true});
        execFileSync("git", ["init", "--quiet", "--initial-branch=test/local-update", tapPath]);
        await writeFile(join(tapPath, "lib/agentix_local_build.rb"), await readFile(join(productionTap, "lib/agentix_local_build.rb")));
        for (const path of ["config", "completions", "src"]) await mkdir(join(source, path), {recursive: true});
        await writeFile(join(source, "Cargo.toml"), `[package]\nname = "${name}"\nversion = "1.2.3"\nedition = "2021"\n`);
        const main = join(source, "src/main.rs");
        await writeFile(main, `fn main() { println!("${name} bootstrap"); }\n`);
        await writeFile(join(source, "config", `${name}.example.toml`), "fixture = true\n");
        for (const file of [`${name}.bash`, `_${name}`, `${name}.fish`]) await writeFile(join(source, "completions", file), "# fixture\n");
        const build = profile => execFileSync("cargo", ["build", "--manifest-path", join(source, "Cargo.toml"), ...(profile === "release" ? ["--release"] : [])]);
        build("release");
        execFileSync("git", ["init", "--quiet", "--initial-branch=main", source]);
        execFileSync("git", ["-C", source, "add", "Cargo.toml", "Cargo.lock", "src", "config", "completions"]);
        execFileSync("git", ["-C", source, "add", "--force", `target/release/${name}`]);
        execFileSync("git", ["-C", source, "-c", "user.name=Homebrew Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-s", "--quiet", "-m", "test: bootstrap isolated HEAD fixture"]);
        const head = join(dir, "head.git");
        execFileSync("git", ["clone", "--bare", "--quiet", source, head]);
        const archive = join(dir, "bootstrap-1.2.3.tar.gz");
        execFileSync("tar", ["-czf", archive, "-C", source, `target/release/${name}`, "config", "completions"]);
        const checksum = createHash("sha256").update(await readFile(archive)).digest("hex");
        const klass = name.split("-").map(word => word[0].toUpperCase() + word.slice(1)).join("");
        const formulaPath = join(tapPath, "Formula", `${name}.rb`);
        await writeFile(formulaPath, `class ${klass} < Formula\n  desc "Local update link fixture"\n  homepage "https://example.invalid"\n  url "file://${archive}"\n  sha256 "${checksum}"\n  head "file://${head}", using: :git, branch: "main"\n  def install\n    bin.install "target/release/${name}"\n  end\nend\n`);
        brew("trust", tap);
        brew("install", `${tap}/${name}`);
        const prefix = brew("--prefix").trim();
        const command = join(prefix, "bin", name);
        const opt = join(prefix, "opt", name);
        const linked = join(prefix, "var/homebrew/linked", name);
        const stable = await realpath(opt);
        brew("install", "--HEAD", "--skip-link", `${tap}/${name}`);
        brew("ruby", "-e", 'require "formulary"; require "unlink"; f = Formulary.factory(ARGV[0], :head); ref = HOMEBREW_LINKED_KEGS/f.name; Homebrew::Unlink.unlink(Keg.new(ref.realpath)); keg = Keg.new(f.latest_head_prefix); keg.lock { keg.link }', `${tap}/${name}`);
        const headKeg = await realpath(linked);
        assert.match(headKeg, /HEAD-/);
        // Reproduce divergent opt/linked state, including an installed stable keg.
        await rm(opt);
        await symlink(stable, opt);
        assert.equal(await realpath(opt), stable);
        assert.equal(await realpath(command), join(headKeg, "bin", name));
        const template = await readFile(join(productionTap, "Formula/taskix.rb"), "utf8");
        const formula = template.replaceAll("taskix", name).replace("class Taskix", `class ${klass}`)
            .replace(/  bottle do\n.*?  end\n/s, "");
        await writeFile(formulaPath, formula);
        const makefile = (await readFile(join(root, "Makefile"), "utf8"))
            .replaceAll("agentix", `agentix-link-fixture-${process.pid}`).replaceAll("taskix", name).replaceAll("tenfyzhong/tap", tap);
        await writeFile(join(source, "Makefile"), makefile);
        const update = profile => spawnSync("make", ["update", "VERSION=local", `PROFILE=${profile}`, `FORMULAE=${name}`, "CARGO=/nonexistent-cargo"], {cwd: source, env, encoding: "utf8", timeout: 90_000});
        let previous;
        for (const [iteration, profile] of [[1, "release"], [1, "release"], [2, "release"], [3, "debug"]]) {
            await writeFile(main, `fn main() { println!("${name} local-${iteration}-${profile}"); }\n`);
            build(profile);
            const result = update(profile);
            assert.equal(result.status, 0, result.stdout + result.stderr);
            const keg = await realpath(opt);
            assert.match(keg, new RegExp(`0\\.0\\.0-local\\..*\\.${profile}$`));
            if (iteration === 1 && previous) assert.equal(keg, previous, "Identical artifacts are reused");
            previous = keg;
            assert.equal(await realpath(linked), keg);
            assert.equal(await realpath(command), join(keg, "bin", name));
            assert.deepEqual(await readFile(command), await readFile(join(source, "target", profile, name)));
            assert.equal(execFileSync(command, [], {encoding: "utf8"}).trim(), `${name} local-${iteration}-${profile}`);
            assert.ok(await readFile(join(stable, "bin", name)));
            assert.ok(await readFile(join(headKeg, "bin", name)));
            assert.equal(await readFile(formulaPath, "utf8"), formula);
            brew("linkage", "--test", `${tap}/${name}`);
        }
        // A new artifact that fails inside Homebrew must not switch old links.
        await writeFile(join(source, "config", `${name}.example.toml`), "fixture = false\n");
        await writeFile(formulaPath, formula.replace(`bin.install "bin/${name}"`, 'raise "fixture install failure"'));
        const failed = update("release");
        assert.notEqual(failed.status, 0);
        assert.match(failed.stdout + failed.stderr, /fixture install failure/);
        assert.equal(await realpath(linked), previous);
        assert.equal(await realpath(command), join(previous, "bin", name));
    } finally {
        try { brew("uninstall", "--force", `${tap}/${name}`); } catch { /* Setup may have failed. */ }
        try { brew("untrust", "--formula", `${tap}/${name}`); brew("untrust", tap); } catch { /* Setup may have failed. */ }
        if (tapPath) await rm(tapPath, {recursive: true, force: true});
        await rm(dir, {recursive: true, force: true});
    }
});
