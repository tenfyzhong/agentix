import assert from "node:assert/strict";
import { test } from "node:test";
import { access, mkdir, mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = new URL("../../../", import.meta.url);
const script = fileURLToPath(new URL(".github/scripts/update-homebrew-formula.rb", repository));
const source = "https://github.com/tenfyzhong/agentix/archive/refs/tags/1.2.3.tar.gz";
const checksum = "a".repeat(64);

// Formula transformation tests use the system Ruby available on macOS.
test("Homebrew adds a stable release to the existing HEAD-only tap formula", { skip: process.platform !== "darwin" }, async () => {
    const directory = await mkdtemp(join(tmpdir(), "taskix-formula-"));
    try {
        const formula = join(directory, "taskix.rb");
        await writeFile(formula, 'class Taskix < Formula\n  homepage "https://github.com/tenfyzhong/agentix"\n  head "https://github.com/tenfyzhong/agentix.git", branch: "main"\n  license "MIT"\nend\n');
        execFileSync("ruby", [script], {
            env: { ...process.env, FORMULA_PATH: formula, SOURCE_URL: source, SOURCE_SHA256: checksum },
        });
        const rendered = await readFile(formula, "utf8");
        assert.ok(rendered.includes(`  url "${source}"`));
        assert.ok(rendered.includes(`  sha256 "${checksum}"`));
        assert.ok(rendered.includes("class Taskix < Formula"));
        assert.ok(rendered.includes('head "https://github.com/tenfyzhong/agentix.git", branch: "main"'));
        assert.ok(!rendered.includes("  bottle do"));
    } finally {
        await rm(directory, { recursive: true, force: true });
    }
});

test("Homebrew updates existing formulas and discards stale bottle and revision metadata", { skip: process.platform !== "darwin" }, async () => {
    const directory = await mkdtemp(join(tmpdir(), "taskix-formula-"));
    try {
        const formula = join(directory, "agentix.rb");
        await writeFile(formula, 'class Agentix < Formula\n  homepage "https://example.com"\n  url "https://example.com/1.tar.gz"\n  sha256 "old"\n  revision 2\n  bottle do\n    sha256 arm64_sequoia: "old"\n  end\n\n  license "MIT"\nend\n');
        execFileSync("ruby", [script], { env: { ...process.env, FORMULA_PATH: formula, SOURCE_URL: source, SOURCE_SHA256: checksum } });
        const rendered = await readFile(formula, "utf8");
        assert.ok(rendered.includes(`  url "${source}"`));
        assert.ok(rendered.includes(`  sha256 "${checksum}"`));
        assert.ok(!rendered.includes("revision"));
        assert.ok(!rendered.includes("bottle"));
        assert.ok(rendered.includes('license "MIT"'));
    } finally {
        await rm(directory, { recursive: true, force: true });
    }
});

test("Homebrew formulas belong exclusively to the tap", async () => {
    await assert.rejects(access(new URL("homebrew/taskix.rb", repository)), { code: "ENOENT" });
    const workflow = await readFile(new URL(".github/workflows/homebrew-publish.yml", repository), "utf8");
    assert.ok(!workflow.includes("FORMULA_TEMPLATE"));
    assert.ok(workflow.includes("repository: tenfyzhong/homebrew-tap"));
});

function job(workflow, name) {
    const body = workflow.replace(/\r\n/g, "\n").split(`\n  ${name}:\n`)[1];
    assert.ok(body, `missing job ${name}`);
    return body.split(/\n  [a-z][\w-]*:\n/)[0];
}

function stepScript(workflow, name) {
    const step = workflow.replace(/\r\n/g, "\n").split(`      - name: ${name}\n`)[1];
    assert.ok(step, `missing step ${name}`);
    return step.split(/\n      - /)[0].split("        run: |\n")[1]
        .split("\n").map(line => line.replace(/^          /, "")).join("\n");
}

test("Homebrew workflow parsing handles Windows checkout line endings", async () => {
    const workflow = (await readFile(new URL(".github/workflows/homebrew-publish.yml", repository), "utf8")).replace(/\r\n/g, "\n");
    const windowsWorkflow = workflow.replace(/\n/g, "\r\n");
    for (const name of ["publish-homebrew"]) {
        assert.equal(job(windowsWorkflow, name), job(workflow, name));
    }
    assert.equal(stepScript(windowsWorkflow, "Add bottle metadata to formula"), stepScript(workflow, "Add bottle metadata to formula"));
});

test("Homebrew publishes one PR per formula only after all platform bottles succeed", async () => {
    const manual = await readFile(new URL(".github/workflows/homebrew.yml", repository), "utf8");
    const prepare = await readFile(new URL(".github/workflows/homebrew-prepare.yml", repository), "utf8");
    const workflow = await readFile(new URL(".github/workflows/homebrew-publish.yml", repository), "utf8");
    const publish = job(workflow, "publish-homebrew");
    assert.match(prepare, /update-homebrew-formula\.rb/);
    assert.match(prepare, /name: homebrew-formula-\$\{\{ matrix\.formula \}\}/);
    assert.match(job(manual, "build-bottles"), /needs: prepare-homebrew/);
    assert.match(job(manual, "publish-homebrew"), /needs: build-bottles/);
    assert.doesNotMatch(manual + workflow, /always\(\)/);
    assert.match(publish, /pattern: homebrew-bottle-\$\{\{ matrix\.formula \}\}-\*/);
    assert.match(publish, /merge-multiple: true/);
    assert.equal((workflow.match(/uses: peter-evans\/create-pull-request@/g) ?? []).length, 1);
});

test("Homebrew merges every platform JSON and refuses incomplete bottle sets", { skip: process.platform === "win32" }, async () => {
    const workflow = await readFile(new URL(".github/workflows/homebrew-publish.yml", repository), "utf8");
    const merge = stepScript(workflow, "Add bottle metadata to formula");
    for (const formula of ["agentix", "taskix"]) {
        for (const count of [3, 2, 0]) {
            const directory = await mkdtemp(join(tmpdir(), "homebrew-merge-"));
            try {
                await mkdir(join(directory, "tap", "Formula"), { recursive: true });
                await mkdir(join(directory, "bottles"));
                await writeFile(join(directory, "tap", "Formula", `${formula}.rb`), "merged formula\n");
                const tags = ["arm64_sequoia", "x86_64_linux", "arm64_linux"].slice(0, count);
                const files = tags.map(tag => `bottles/${formula}--1.2.3.${tag}.bottle.json`);
                for (const file of files) await writeFile(join(directory, file), "{}\n");
                const fakeBrew = `brew() {
    if [[ "$1" == "--repository" ]]; then
        echo "$PWD/tap"
    else
        printf '%s\\n' "$@" > "$PWD/merge-args"
    fi
}
`;
                const run = () => execFileSync("bash", ["-euo", "pipefail", "-c", fakeBrew + merge], {
                    cwd: directory,
                    env: { ...process.env, FORMULA: formula, TAP_NAME: "tenfyzhong/tap", FORMULA_PATH: join(directory, "result.rb") },
                    stdio: "pipe",
                });
                if (count === 3) {
                    run();
                    const args = (await readFile(join(directory, "merge-args"), "utf8")).trim().split("\n");
                    assert.deepEqual(args.slice(0, 4), ["bottle", "--merge", "--write", "--no-commit"]);
                    assert.deepEqual(args.slice(4).sort(), files.sort());
                    assert.equal(await readFile(join(directory, "result.rb"), "utf8"), "merged formula\n");
                } else {
                    assert.throws(run, error => error.status !== 0, `must reject ${count} platform bottles`);
                    await assert.rejects(access(join(directory, "merge-args")), { code: "ENOENT" });
                    await assert.rejects(access(join(directory, "result.rb")), { code: "ENOENT" });
                }
            } finally {
                await rm(directory, { recursive: true, force: true });
            }
        }
    }
});

for (const path of [".github/workflows/homebrew-publish.yml", ".github/actions/build-bottles/action.yml"]) {
    test(`Homebrew trusts a fresh tap before syntax validation in ${path}`, { skip: process.platform === "win32" }, async () => {
        const workflow = await readFile(new URL(path, repository), "utf8");
        const initialization = workflow.split(/\r?\n/)
            .filter(line => /^\s*brew (tap|trust) "\$TAP_NAME"$/.test(line))
            .map(line => line.trim()).join("\n");
        assert.equal(initialization.split("\n").length, 2);
        const mock = `trusted=0
brew() {
    [[ "$2" == "$TAP_NAME" ]] || return 2
    case "$1" in
        trust) trusted=1 ;;
        tap) [[ "$trusted" == 1 ]] || { echo 'Refusing to load formula from untrusted tap' >&2; return 1; } ;;
        *) return 2 ;;
    esac
}
`;
        execFileSync("bash", ["-euo", "pipefail", "-c", mock + initialization], {
            env: { ...process.env, BASH_ENV: "/dev/null", TAP_NAME: "test/release-tap" }, stdio: "pipe",
        });
    });
}
