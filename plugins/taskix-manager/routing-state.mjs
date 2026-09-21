import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Hooks are separate processes. Persist only an expiring receipt, never prompts,
// credentials, model responses or task leases. An unreadable receipt is a miss.
function receiptPath(event, directory) {
    const key = createHash("sha256").update(JSON.stringify([event.session_id, event.cwd || "", process.env.TASKIX_CONFIG || ""])).digest("hex");
    return join(directory, `${key}.json`);
}
export async function routingReceipt(event, action, directory = join(tmpdir(), `taskix-routing-${process.getuid?.() ?? "user"}`)) {
    const path = receiptPath(event, directory);
    try {
        if (action === "clear") {
            await rm(path, { force: true });
            return false;
        }
        if (action === "write") {
            await mkdir(directory, { recursive: true, mode: 0o700 });
            const temporary = `${path}.${randomUUID()}`;
            try {
                await writeFile(temporary, JSON.stringify({ turn: event.turn_id ?? null, expires: Date.now() + 3600000 }), { mode: 0o600, flag: "wx" });
                await rename(temporary, path);
            } finally {
                await rm(temporary, { force: true });
            }
            return true;
        }
        const receipt = JSON.parse(await readFile(path, "utf8"));
        return receipt.expires > Date.now() && receipt.turn === (event.turn_id ?? null);
    } catch { return false; }
}
