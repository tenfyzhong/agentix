import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { execFileSync, spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const read = async path => (await readFile(join(root, path), "utf8")).replace(/\r\n/g, "\n");
function stepScript(workflow, name) {
    const step = workflow.replace(/\r\n/g, "\n").split(`      - name: ${name}\n`)[1];
    assert.ok(step, `missing step ${name}`);
    const body = step.split(/\n      - /)[0];
    const inline = body.match(/        run: ([^|\n].*)/);
    return inline ? inline[1] : body.split("        run: |\n")[1].replace(/^          /gm, "");
}

test("backup_release_is_published_with_verified_script_and_three_platform_bottles", async () => {
    const release = await read(".github/workflows/release.yml");
    assert.match(release, /\n  package-backup:\n/);
    const publish = release.split("\n  publish-release:\n")[1].split("\n  publish-homebrew:\n")[0];
    assert.match(publish, /- package-backup/);
    for (const path of ["homebrew-prepare.yml", "homebrew-publish.yml"]) {
        const workflow = await read(`.github/workflows/${path}`);
        const expression = workflow.match(/formula: \$\{\{ fromJSON\((.*?)\) \}\}/)[1];
        for (const selection of ["all", "agentix", "taskix", "taskix-backup"]) {
            const matrices = Function("inputs", `return ${expression}`)({ formula: selection });
            assert.deepEqual(JSON.parse(matrices), selection === "all" ? ["agentix", "taskix", "taskix-backup"] : [selection]);
        }
    }
    const action = await read(".github/actions/build-bottles/action.yml");
    assert.match(action, /for formula in agentix taskix taskix-backup/);
    assert.match(action, /BOTTLE_INSTALL_KIND/);
    assert.match(action, /name: homebrew-bottle-taskix-backup-\$\{\{ inputs.artifact-suffix \}\}/);
    const manual = await read(".github/workflows/homebrew.yml");
    assert.match(manual, /          - taskix-backup/);
    // Script formulae install tagged source; the native download loop stays bounded.
    assert.match(manual, /for formula in agentix taskix; do/);
});

test("backup_archive_contains_only_the_executable_and_license_and_enters_checksums", { skip: process.platform === "win32" }, async t => {
    const directory = await mkdtemp(join(tmpdir(), "backup-release-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    await mkdir(join(directory, "scripts"));
    const script = await read("scripts/taskix-backup.py");
    await writeFile(join(directory, "scripts/taskix-backup.py"), script);
    await writeFile(join(directory, "LICENSE"), await read("LICENSE"));
    const release = await read(".github/workflows/release.yml");
    execFileSync("bash", ["-euo", "pipefail", "-c", stepScript(release, "Package backup tool")], {
        cwd: directory, env: { ...process.env, RELEASE_TAG: "1.2.3" },
    });
    const archive = join(directory, "dist/taskix-backup-1.2.3.tar.gz");
    const files = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" }).trim().split("\n").filter(p => !p.endsWith("/"));
    assert.deepEqual(files.map(p => p.replace(/^\.\//, "")).sort(), ["LICENSE", "taskix-backup"]);
    const unpack = join(directory, "unpacked");
    await mkdir(unpack);
    execFileSync("tar", ["-xzf", archive, "-C", unpack]);
    assert.equal(await readFile(join(unpack, "taskix-backup"), "utf8"), script);
    assert.match(execFileSync(join(unpack, "taskix-backup"), ["--help"], { encoding: "utf8" }), /--restore/);
    await writeFile(join(directory, "dist/agentix-1.2.3-fixture.tar.gz"), "fixture");
    execFileSync("bash", ["-euo", "pipefail", "-c", stepScript(release, "Create checksums")], { cwd: join(directory, "dist") });
    assert.match(await readFile(join(directory, "dist/SHA256SUMS"), "utf8"), /  taskix-backup-1.2.3.tar.gz/);
    execFileSync("sha256sum", ["--check", "SHA256SUMS"], { cwd: join(directory, "dist") });
});

for (const [runnerOs, legacyOpenSsl] of [["macOS", true], ["macOS", false], ["Linux", true], ["", true]]) {
    test(`script_bottles_install_and_pour_with_legacy_openssl_${runnerOs || "non_ci"}_${legacyOpenSsl}`, { skip: process.platform === "win32" }, async t => {
        const directory = await mkdtemp(join(tmpdir(), "backup-bottle-"));
        t.after(() => rm(directory, { recursive: true, force: true }));
        for (const path of ["tools", "tap/Formula", "keg/.brew", "keg/bin"]) await mkdir(join(directory, path), { recursive: true });
        const formula = 'class TaskixBackup < Formula\n  def install\n    bin.install "scripts/taskix-backup.py" => "taskix-backup"\n  end\nend\n';
        await writeFile(join(directory, "prepared.rb"), formula);
        await writeFile(join(directory, "keg/bin/taskix-backup"), "#!/bin/sh\necho '--restore'\n");
        await chmod(join(directory, "keg/bin/taskix-backup"), 0o755);
        await writeFile(join(directory, "tools/brew"), `#!/bin/bash
    printf '%s\\n' "$*" >> "$FIXTURE/calls"
    case "$1" in
        --repository) echo "$FIXTURE/tap" ;;
        list) [[ "$3" == openssl@1.1 && "$LEGACY_OPENSSL" == 1 ]] ;;
        unlink) [[ "$2" == openssl@1.1 ]] || exit 1; touch "$FIXTURE/unlinked-openssl" ;;
        install)
            if [[ "$2" == --build-bottle && "$RUNNER_OS" == macOS && "$LEGACY_OPENSSL" == 1 && ! -f "$FIXTURE/unlinked-openssl" ]]; then
                echo "Could not symlink bin/openssl: linked openssl@1.1" >&2
                exit 1
            fi ;;
        --prefix) echo "$FIXTURE/keg" ;;
        bottle) touch taskix-backup--1.2.3.arm64_sequoia.bottle.tar.gz ;;
    esac
    `);
        await chmod(join(directory, "tools/brew"), 0o755);
        const result = spawnSync("bash", [join(root, ".github/scripts/build-homebrew-bottle.sh")], {
            cwd: directory, encoding: "utf8", env: { ...process.env, PATH: `${directory}/tools:${process.env.PATH}`,
                FIXTURE: directory, RUNNER_OS: runnerOs, LEGACY_OPENSSL: legacyOpenSsl ? "1" : "0",
                FORMULA: "taskix-backup", FORMULA_PATH: join(directory, "prepared.rb"),
                BOTTLE_INSTALL_KIND: "script", TAP_NAME: "fixture/tap", RELEASE_TAG: "1.2.3", BOTTLE_ROOT_URL: "https://example.invalid" },
        });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(await readFile(join(directory, "tap/Formula/taskix-backup.rb"), "utf8"), formula);
        assert.equal(await readFile(join(directory, "keg/.brew/taskix-backup.rb"), "utf8"), formula);
        const calls = await readFile(join(directory, "calls"), "utf8");
        if (runnerOs === "macOS" && legacyOpenSsl) {
            assert.ok(calls.indexOf("unlink openssl@1.1\n") >= 0);
            assert.ok(calls.indexOf("unlink openssl@1.1\n") < calls.indexOf("install --build-bottle"));
        } else {
            assert.doesNotMatch(calls, /unlink /);
        }
        assert.equal(calls.split("\n").filter(line => line === "test fixture/tap/taskix-backup").length, 2);
        assert.match(calls, /install --build-bottle fixture\/tap\/taskix-backup/);
        assert.match(calls, /install --force-bottle/);
        assert.ok((await readdir(directory)).includes("taskix-backup-1.2.3.arm64_sequoia.bottle.tar.gz"));
    });
}
