import assert from "node:assert/strict";
import { test } from "node:test";
import { access, mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repository = new URL("../../../", import.meta.url);
const script = fileURLToPath(new URL(".github/scripts/update-homebrew-formula.rb", repository));
const source = "https://github.com/tenfyzhong/agentix/archive/refs/tags/1.2.3.tar.gz";
const checksum = "a".repeat(64);

// Homebrew runs on macOS; other CI runners need not install Ruby.
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
    const workflow = await readFile(new URL(".github/workflows/homebrew.yml", repository), "utf8");
    assert.ok(!workflow.includes("FORMULA_TEMPLATE"));
    assert.ok(workflow.includes("repository: tenfyzhong/homebrew-tap"));
});
