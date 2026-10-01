import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { buildVendor } from "../scripts/build-vendor.mjs";

test("vendored_dependencies_are_current_and_drift_checks_do_not_rewrite_files", async t => {
    await buildVendor({ check: true });
    const outputDirectory = await mkdtemp(join(tmpdir(), "taskix-vendor-check-"));
    t.after(() => rm(outputDirectory, { recursive: true, force: true }));
    await assert.rejects(buildVendor({ check: true, outputDirectory }), /missing or stale/);
    await buildVendor({ outputDirectory });
    await buildVendor({ check: true, outputDirectory });
    const path = join(outputDirectory, "smol-toml.mjs");
    await writeFile(path, "stale dependency\n");
    await assert.rejects(buildVendor({ check: true, outputDirectory }), /missing or stale: smol-toml.mjs/);
    assert.equal(await readFile(path, "utf8"), "stale dependency\n");
    await buildVendor({ outputDirectory });
    const license = join(outputDirectory, "typebox.LICENSE");
    await writeFile(license, "stale license\n");
    await assert.rejects(buildVendor({ check: true, outputDirectory }), /missing or stale: typebox.LICENSE/);
    assert.equal(await readFile(license, "utf8"), "stale license\n");
});
