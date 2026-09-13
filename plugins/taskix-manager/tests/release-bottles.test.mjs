import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtemp, writeFile, readFile, rm, chmod } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const overlay = join(root, ".github/scripts/prepare-bottle-formula.rb");
const rubyAvailable = process.platform !== "win32" && spawnSync("ruby", ["--version"]).status === 0;
const fixture = `class Agentix < Formula
  url "https://example.com/1.2.3.tar.gz"
  sha256 "${"a".repeat(64)}"
  depends_on "rust" => :build
  depends_on "protobuf" => :build
  depends_on "libiconv"
  def install
    system "bash", ".github/scripts/set-release-version.sh", version.to_s
    system "cargo", "install", *std_cargo_args(path: "crates/agentix")
    pkgshare.install "config/agentix.example.toml"
    bash_completion.install "completions/agentix.bash" => "agentix"
  end
  service do
    run [opt_bin/"agentix", "serve"]
  end
end
`;

test("bottle overlay replaces compilation but preserves source and tap installation customizations", { skip: !rubyAvailable }, async () => {
    const dir = await mkdtemp(join(tmpdir(), "release-bottle-"));
    try {
        const path = join(dir, "agentix.rb");
        const binary = join(dir, 'binary with spaces');
        await writeFile(binary, "binary");
        await chmod(binary, 0o755);
        await writeFile(path, fixture);
        execFileSync("ruby", [overlay], { env: { ...process.env, FORMULA_PATH: path, PREBUILT_BINARY: binary, FORMULA: "agentix" } });
        const actual = await readFile(path, "utf8");
        assert.match(actual, /bin.install .* => "agentix"/);
        assert.doesNotMatch(actual, /system "cargo"|set-release-version|depends_on "rust"|depends_on "protobuf"/);
        for (const line of fixture.split("\n").filter(line => /url |sha256 |libiconv|pkgshare|bash_completion|service|opt_bin/.test(line))) {
            assert.ok(actual.includes(line), `must preserve ${line}`);
        }
        execFileSync("ruby", ["-c", path]);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("bottle overlay refuses changed compiler recipes or missing binaries without modifying formula", { skip: !rubyAvailable }, async () => {
    const dir = await mkdtemp(join(tmpdir(), "release-bottle-"));
    try {
        const path = join(dir, "agentix.rb");
        const binary = join(dir, "agentix");
        await writeFile(binary, "binary");
        await chmod(binary, 0o755);
        for (const [body, input] of [
            [fixture.replace('system "cargo", "install", *std_cargo_args(path: "crates/agentix")', 'system "make"'), binary],
            [fixture, join(dir, "missing")],
        ]) {
            await writeFile(path, body);
            assert.throws(() => execFileSync("ruby", [overlay], { env: { ...process.env, FORMULA_PATH: path, PREBUILT_BINARY: input, FORMULA: "agentix" }, stdio: "pipe" }));
            assert.equal(await readFile(path, "utf8"), body);
        }
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

for (const [label, newline] of [["LF", "\n"], ["CRLF", "\r\n"]]) {
    test(`each release platform bottles its own binaries before the shared publication barrier (${label})`, async () => {
        const release = (await readFile(join(root, ".github/workflows/release.yml"), "utf8")).replace(/\r?\n/g, newline);
        const build = release.split(/\r?\n  build-release-binaries:\r?\n/)[1].split(/\r?\n  publish-release:\r?\n/)[0];
        assert.match(build, /uses: .\/packaging-automation\/.github\/actions\/build-bottles/);
        assert.match(build, /ref: \$\{\{ github.workflow_sha \}\}/);
        assert.doesNotMatch(build, /needs:.*publish-release/);
        assert.ok(build.indexOf("Build release binary") < build.indexOf("uses: ./packaging-automation/.github/actions/build-bottles"));
        assert.match(build, /if: runner.os != 'Windows'/);
        const manual = await readFile(join(root, ".github/workflows/homebrew.yml"), "utf8");
        assert.match(manual, /Download release binaries/);
        assert.match(manual, /verify-release-archive/);
        assert.doesNotMatch(manual, /cargo build|cargo install/);
    });
}

test("manual release reuse rejects missing, duplicate and corrupted archive checksums", { skip: !rubyAvailable }, async () => {
    const dir = await mkdtemp(join(tmpdir(), "release-checksum-"));
    try {
        const archive = join(dir, "agentix-1.2.3-aarch64-apple-darwin.tar.gz");
        const sums = join(dir, "SHA256SUMS");
        await writeFile(archive, "binary archive");
        const { createHash } = await import("node:crypto");
        const digest = createHash("sha256").update("binary archive").digest("hex");
        const entry = `${digest}  agentix-1.2.3-aarch64-apple-darwin.tar.gz\n`;
        for (const [content, valid] of [[entry, true], ["", false], [entry + entry, false], [entry.replace(digest, "0".repeat(64)), false]]) {
            await writeFile(sums, content);
            const result = spawnSync("ruby", [join(root, ".github/scripts/verify-release-archive.rb"), archive, sums], { encoding: "utf8" });
            assert.equal(result.status === 0, valid, result.stderr);
        }
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

// Opt-in because this creates and removes a uniquely named Homebrew fixture.
// No developer Agentix/Taskix installation or service is touched.
test("real Homebrew bottles a prebuilt executable, restores source metadata and pours it", {
    skip: process.env.AGENTIX_TEST_HOMEBREW !== "1",
    timeout: 300_000,
}, async () => {
    const { mkdir } = await import("node:fs/promises");
    const { createHash } = await import("node:crypto");
    const dir = await mkdtemp(join(tmpdir(), "bottle-integration-"));
    const name = `codex-bottle-fixture-${process.pid}`;
    const tap = `codex-fixture/release-${process.pid}`;
    const env = { ...process.env, HOMEBREW_NO_AUTO_UPDATE: "1", HOMEBREW_NO_INSTALL_CLEANUP: "1", HOMEBREW_NO_ENV_HINTS: "1" };
    const brew = (...args) => execFileSync("brew", args, { env, encoding: "utf8", stdio: "pipe", timeout: 240_000 });
    let tapPath;
    try {
        tapPath = brew("--repository", tap).trim();
        await mkdir(join(tapPath, "Formula"), { recursive: true });
        execFileSync("git", ["init", "--initial-branch=test/fixture", tapPath]);
        await mkdir(join(dir, "source", "config"), { recursive: true });
        await mkdir(join(dir, "source", "completions"));
        await writeFile(join(dir, "source", "config", `${name}.example.toml`), "fixture = true\n");
        await writeFile(join(dir, "source", "completions", `${name}.bash`), "# fixture\n");
        const archive = join(dir, "source-1.2.3.tar.gz");
        execFileSync("tar", ["-czf", archive, "-C", join(dir, "source"), "."]);
        const digest = createHash("sha256").update(await readFile(archive)).digest("hex");
        const c = join(dir, "fixture.c");
        const binary = join(dir, name);
        await writeFile(c, `#include <stdio.h>\nint main(void) { puts("${name} 1.2.3"); return 0; }\n`);
        execFileSync("cc", [c, "-o", binary]);
        const klass = name.split("-").map(word => word[0].toUpperCase() + word.slice(1)).join("");
        const formula = fixture.replaceAll("agentix", name).replace("class Agentix", `class ${klass}`)
            .replace('  depends_on "libiconv"\n', "")
            .replace('https://example.com/1.2.3.tar.gz', `file://${archive}`)
            .replace("a".repeat(64), digest)
            .replace(/  service do[\s\S]*?\n  end/, `  test do\n    assert_match "${name} 1.2.3", shell_output("#{bin}/${name} --version")\n  end`);
        const path = join(tapPath, "Formula", `${name}.rb`);
        const prepared = join(dir, `${name}.rb`);
        await writeFile(path, formula);
        await writeFile(prepared, formula);
        brew("trust", tap);
        // Source recipe deliberately cannot compile: the tarball contains no Cargo project.
        const result = spawnSync("bash", [join(root, ".github/scripts/build-homebrew-bottle.sh")], {
            cwd: dir,
            env: { ...env, FORMULA: name, FORMULA_PATH: prepared, PREBUILT_BINARY: binary, TAP_NAME: tap, RELEASE_TAG: "1.2.3", BOTTLE_ROOT_URL: "https://example.com/releases/1.2.3" },
            encoding: "utf8", timeout: 240_000,
        });
        assert.equal(result.status, 0, result.stdout + result.stderr);
        assert.equal(await readFile(path, "utf8"), formula);
        const prefix = brew("--prefix", `${tap}/${name}`).trim();
        assert.equal(await readFile(join(prefix, ".brew", `${name}.rb`), "utf8"), formula);
        const receipt = JSON.parse(await readFile(join(prefix, "INSTALL_RECEIPT.json"), "utf8"));
        assert.equal(receipt.poured_from_bottle, true);
        assert.equal(execFileSync(join(prefix, "bin", name), ["--version"], { encoding: "utf8" }).trim(), `${name} 1.2.3`);
    } finally {
        try { brew("uninstall", "--force", `${tap}/${name}`); } catch { /* Uninstalled on failure before pour. */ }
        try { brew("untrust", "--formula", `${tap}/${name}`); brew("untrust", tap); } catch { /* Fixture may not have been trusted yet. */ }
        if (tapPath) await rm(tapPath, { recursive: true, force: true });
        await rm(dir, { recursive: true, force: true });
    }
});

test("native archive checksums stay valid when a manual run replaces bottles", async () => {
    const release = await readFile(join(root, ".github/workflows/release.yml"), "utf8");
    assert.ok(release.indexOf("name: Create checksums") < release.indexOf("name: Collect bottle archives"),
        "SHA256SUMS must cover native archives before independently rebuildable bottles are collected");
    const manual = await readFile(join(root, ".github/workflows/homebrew.yml"), "utf8");
    assert.match(manual, /ref: \$\{\{ github.workflow_sha \}\}/, "older tags must use current packaging automation");
    assert.match(manual, /group: release-\$\{\{ inputs.tag \}\}/, "manual and automatic publication must share a tag lock");
});

test("real Homebrew trusts an absent tap before syntax validation", {
    skip: process.env.AGENTIX_TEST_HOMEBREW !== "1",
    timeout: 120_000,
}, async () => {
    const { mkdir } = await import("node:fs/promises");
    const dir = await mkdtemp(join(tmpdir(), "tap-trust-"));
    const remote = join(dir, "remote");
    const tap = `codex-fixture/trust-${process.pid}`;
    const env = { ...process.env, BASH_ENV: "/dev/null", XDG_CONFIG_HOME: join(dir, "config"),
        HOMEBREW_DEVELOPER: "", HOMEBREW_NO_AUTO_UPDATE: "1", HOMEBREW_NO_INSTALL_CLEANUP: "1" };
    const brew = (...args) => spawnSync("brew", args, { env, encoding: "utf8", timeout: 60_000 });
    let tapPath;
    try {
        await mkdir(join(remote, "Formula"), { recursive: true });
        await writeFile(join(remote, "Formula", "tap-trust-fixture.rb"), `class TapTrustFixture < Formula
  desc "Release tap trust regression fixture"
  homepage "https://example.com"
  url "https://example.com/fixture-1.2.3.tar.gz"
  sha256 "${"a".repeat(64)}"
  license "MIT"
  def install
    bin.install "fixture"
  end
end
`);
        const git = (...args) => execFileSync("git", ["-c", "user.name=Release Test", "-c", "user.email=release@example.com",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd: remote, stdio: "pipe" });
        git("init", "--initial-branch=test/tap-trust");
        git("add", "Formula");
        git("commit", "-s", "-m", "test: prepare tap trust fixture");
        tapPath = brew("--repository", tap).stdout.trim();
        assert.ok(tapPath.endsWith(`homebrew-trust-${process.pid}`));
        const untrusted = brew("tap", tap, `file://${remote}`);
        assert.notEqual(untrusted.status, 0);
        assert.match(untrusted.stdout + untrusted.stderr, /untrusted tap/);
        // The fixture has a custom local origin; trust that origin rather than GitHub shorthand.
        const trusted = brew("trust", `file://${remote}`);
        assert.equal(trusted.status, 0, trusted.stdout + trusted.stderr);
        const installed = brew("tap", tap, `file://${remote}`);
        assert.equal(installed.status, 0, installed.stdout + installed.stderr);
        assert.match(await readFile(join(tapPath, "Formula", "tap-trust-fixture.rb"), "utf8"), /class TapTrustFixture/);
    } finally {
        if (tapPath) await rm(tapPath, { recursive: true, force: true });
        await rm(dir, { recursive: true, force: true });
    }
});
