import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, readlink, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const unix = {skip: process.platform === "win32"};
async function fixture(t, missing = false, native = false, exact = false) {
    const dir = await realpath(await mkdtemp(join(tmpdir(), "brew-local-switch-")));
    t.after(() => rm(dir, {recursive: true, force: true}));
    const log = join(dir, "calls");
    const cellar = join(dir, "Cellar");
    const linked = join(dir, "linked");
    await mkdir(linked);
    await mkdir(join(dir, "bin"));
    await mkdir(join(dir, "opt"));
    await mkdir(join(dir, "locks"));
    await writeFile(log, "");
    for (const name of ["agentix", "taskix"]) {
        for (const [version, time] of [["0.4.12", 100], ["HEAD-abc", 200], ["0.0.0-local.zzz.release", 300], ["0.0.0-local.aaa.release", 400], ["0.0.0-local.bbb.debug", 500]]) {
            if (missing && name === "taskix" && version.includes("local")) continue;
            const keg = join(cellar, name, version);
            await mkdir(keg, {recursive: true});
            await mkdir(join(keg, "bin"));
            await writeFile(join(keg, "bin", name), "fixture\n");
            await chmod(join(keg, "bin", name), 0o755);
            await writeFile(join(keg, "INSTALL_RECEIPT.json"), JSON.stringify({time, source: {tap: "tenfyzhong/tap"}}));
            await writeFile(join(keg, "receipt.json"), JSON.stringify({time, tap: "tenfyzhong/tap"}));
        }
        await symlink(join(cellar, name, "HEAD-abc"), join(linked, name));
        await symlink(join(cellar, name, "HEAD-abc", "bin", name), join(dir, "bin", name));
        await symlink(join(cellar, name, "HEAD-abc"), join(dir, "opt", name));
    }
    await writeFile(join(dir, "keg.rb"), `require "pathname"
require "json"
require "ostruct"
HOMEBREW_CELLAR = Pathname(ENV.fetch("TEST_CELLAR"))
HOMEBREW_LINKED_KEGS = Pathname(ENV.fetch("TEST_LINKED"))
class Pathname
  def subdirs; children.select(&:directory?); end
end
class Keg
  attr_reader :path
  def initialize(path); @path = path; end
  def name; path.parent.basename.to_s; end
  def to_path; path.to_s; end
  def version; path.basename.to_s; end
  def tab
    data = JSON.parse((path/"receipt.json").read)
    OpenStruct.new(time: data.fetch("time"), tap: OpenStruct.new(name: data.fetch("tap")))
  end
  def lock
    yield
  end
  def link
    File.open(ENV.fetch("TEST_LOG"), "a") { |f| f.puts "link:#{path.parent.basename}/#{version}" }
    File.symlink(path, HOMEBREW_LINKED_KEGS/path.parent.basename)
  end
end
`);
    await writeFile(join(dir, "formulary.rb"), `module Formulary
  def self.factory(name, spec)
    raise "Expected stable artifact spec" unless spec == :stable
    version = ENV.fetch("HOMEBREW_AGENTIX_LOCAL_PROFILE", "release") == "debug" ? "0.0.0-local.bbb.debug" : "0.0.0-local.zzz.release"
    OpenStruct.new(prefix: HOMEBREW_CELLAR/name.split("/").last/version)
  end
end
`);
    await writeFile(join(dir, "unlink.rb"), `module Homebrew::Unlink
  def self.unlink(keg)
    File.open(ENV.fetch("TEST_LOG"), "a") { |f| f.puts "unlink:#{keg.path.parent.basename}/#{keg.version}" }
    File.unlink(HOMEBREW_LINKED_KEGS/keg.path.parent.basename)
  end
end
`);
    // Homebrew defines its namespace before loading Ruby commands.
    await writeFile(join(dir, "bootstrap.rb"), "module Homebrew; end\n");
    if (native) {
        await writeFile(join(dir, "bootstrap.rb"), `{
  HOMEBREW_PREFIX: Pathname(${JSON.stringify(dir)}),
  HOMEBREW_CELLAR: Pathname(${JSON.stringify(cellar)}),
  HOMEBREW_LINKED_KEGS: Pathname(${JSON.stringify(linked)}),
  HOMEBREW_LOCKS: Pathname(${JSON.stringify(join(dir, "locks"))})
}.each { |name, value| Object.send(:remove_const, name); Object.const_set(name, value) }
`);
    }
    const brew = join(dir, "brew");
    await writeFile(brew, native ? '#!/bin/sh\n[ "$1" = ruby ] || exit 91\nshift\nexec "$TEST_REAL_BREW" ruby -r "$TEST_BOOTSTRAP" "$@"\n' : '#!/bin/sh\n[ "$1" = ruby ] || exit 91\nshift\nexec ruby -r "$TEST_BOOTSTRAP" "$@"\n');
    await chmod(brew, 0o755);
    return {
        prefix: dir,
        run: (...args) => spawnSync("make", ["switch", "VERSION=local", `BREW=${brew}`, ...args], {cwd: root, encoding: "utf8", env: {...process.env, HOMEBREW_AGENTIX_LOCAL_SOURCE: exact ? dir : "", RUBYLIB: native ? "" : dir, TEST_PREFIX: dir, TEST_REAL_BREW: native ? spawnSync("which", ["brew"], {encoding: "utf8"}).stdout.trim() : "", HOMEBREW_NO_AUTO_UPDATE: "1", TEST_BOOTSTRAP: join(dir, "bootstrap.rb"), TEST_CELLAR: cellar, TEST_LINKED: linked, TEST_LOG: log}}),
        calls: async () => (await readFile(log, "utf8")).trim().split("\n").filter(Boolean),
        linked: name => readlink(join(linked, name)),
    };
}
for (const [profile, version] of [["release", "0.0.0-local.aaa.release"], ["debug", "0.0.0-local.bbb.debug"]]) {
    test(`switch_local_selects_latest_installed_${profile}_keg`, unix, async t => {
        const f = await fixture(t);
        const result = f.run(`PROFILE=${profile}`);
        assert.equal(result.status, 0, result.stderr);
        assert.deepEqual(await f.calls(), ["unlink:agentix/HEAD-abc", "unlink:taskix/HEAD-abc", `link:agentix/${version}`, `link:taskix/${version}`]);
        assert.equal((await f.linked("agentix")).split("/").at(-1), version);
    });
}
test("switch_local_uses_exact_artifact_inputs_even_when_a_newer_local_keg_exists", unix, async t => {
    const f = await fixture(t, false, false, true);
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), ["unlink:agentix/HEAD-abc", "unlink:taskix/HEAD-abc", "link:agentix/0.0.0-local.zzz.release", "link:taskix/0.0.0-local.zzz.release"]);
});
test("switch_local_uses_requested_profile_for_exact_artifact_inputs", unix, async t => {
    const f = await fixture(t, false, false, true);
    const result = f.run("PROFILE=debug");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await f.calls(), ["unlink:agentix/HEAD-abc", "unlink:taskix/HEAD-abc", "link:agentix/0.0.0-local.bbb.debug", "link:taskix/0.0.0-local.bbb.debug"]);
});
test("switch_local_validates_all_exact_artifacts_before_unlinking", unix, async t => {
    const f = await fixture(t, true, false, true);
    assert.notEqual(f.run().status, 0);
    assert.deepEqual(await f.calls(), []);
    assert.equal((await f.linked("agentix")).split("/").at(-1), "HEAD-abc");
});
test("switch_local_validates_both_kegs_before_unlinking", unix, async t => {
    const f = await fixture(t, true);
    const result = f.run();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /taskix.*local.*not installed/);
    assert.deepEqual(await f.calls(), []);
    assert.equal((await f.linked("agentix")).split("/").at(-1), "HEAD-abc");
});
test("switch_local_selects_only_requested_cli", unix, async t => {
    const f = await fixture(t, true);
    assert.equal(f.run("FORMULAE=agentix").status, 0);
    assert.deepEqual(await f.calls(), ["unlink:agentix/HEAD-abc", "link:agentix/0.0.0-local.aaa.release"]);
    assert.equal((await f.linked("taskix")).split("/").at(-1), "HEAD-abc");
});
for (const arg of ["PROFILE=bad", "FORMULAE=unknown", "FORMULAE="]) {
    test(`switch_local_rejects_${arg}_before_unlinking`, unix, async t => {
        const f = await fixture(t);
        assert.notEqual(f.run(arg).status, 0);
        assert.deepEqual(await f.calls(), []);
    });
}

test("real_homebrew_switch_links_exact_local_keg_in_isolated_prefix", {skip: process.platform === "win32" || process.env.AGENTIX_TEST_HOMEBREW !== "1"}, async t => {
    const f = await fixture(t, false, true);
    for (const [profile, version] of [["release", "0.0.0-local.aaa.release"], ["debug", "0.0.0-local.bbb.debug"], ["release", "0.0.0-local.aaa.release"]]) {
        const result = f.run(`PROFILE=${profile}`);
        assert.equal(result.status, 0, result.stderr);
        for (const name of ["agentix", "taskix"]) {
            const keg = join(f.prefix, "Cellar", name, version);
            assert.equal(await realpath(join(f.prefix, "bin", name)), join(keg, "bin", name));
            assert.equal(await realpath(join(f.prefix, "opt", name)), keg);
        }
    }
});
