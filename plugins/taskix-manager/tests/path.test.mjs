import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runTaskix } from "../runtime.mjs";

test("runTaskix resolves PATH even when legacy TASKIX_BIN is set", { skip: process.platform === "win32" }, async (t) => {
    const dir = await mkdtemp(join(tmpdir(), "taskix-path-"));
    const previous = { PATH: process.env.PATH, TASKIX_BIN: process.env.TASKIX_BIN };
    t.after(async () => {
        for (const [key, value] of Object.entries(previous)) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
        await rm(dir, { recursive: true, force: true });
    });
    await writeFile(join(dir, "taskix"), '#!/bin/sh\nprintf \'%s\\n\' \'{"schema_version":1,"ok":true,"result":"from-path"}\'\n', { mode: 0o755 });
    process.env.PATH = dir;
    process.env.TASKIX_BIN = join(dir, "missing-legacy-binary");
    assert.equal((await runTaskix(["context"])).result, "from-path");
});
