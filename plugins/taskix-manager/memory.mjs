import { parse } from "./vendor/smol-toml.mjs";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, open, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir, homedir } from "node:os";
import { join } from "node:path";

const pendingReceipts = new Map();

// Independent of routing and extraction. Failures must never interrupt host work.
export async function memoryContext(prompt, turn, options, runner, { cacheDir, timeoutMs = 1500, requireConfig = false, memoryConfigPath } = {}) {
    if (typeof prompt !== "string" || !prompt.trim() || !options.session) return "";
    const controller = new AbortController();
    const signal = options.signal ? AbortSignal.any([options.signal, controller.signal]) : controller.signal;
    let timer;
    try {
        const prepared = await Promise.race([
            new Promise(resolve => { timer = setTimeout(() => { controller.abort(); resolve(""); }, timeoutMs); }),
            (async () => {
                if (requireConfig && !await memoryConfigured(memoryConfigPath)) return "";
                signal.throwIfAborted();
                // Hosts without native turn IDs get a fresh delivery, never a prompt hash
                // that would conflate intentional repetitions across turns.
                const turnId = turn || randomUUID();
                const query = [...prompt].slice(0, 1000).join("");
                const packet = (await runner(["memory", "context", query, "--turn", turnId, "--budget", "6400"], { ...options, signal })).result;
                signal.throwIfAborted();
                if (!packet?.text || Buffer.byteLength(packet.text) > 6400 || !Array.isArray(packet.items)) return "";
                return await deduplicate(packet, turnId, options, cacheDir, signal);
            })(),
        ]);
        if (!prepared) return "";
        signal.throwIfAborted();
        // Accept the delivery before publishing its receipt. There is no await
        // between this decision and returning text, so a timer cannot discard it.
        prepared.commit();
        return prepared.text;
    } catch { return ""; }
    finally { clearTimeout(timer); controller.abort(); }
}

async function deduplicate(packet, turn, options, directory = join(tmpdir(), `taskix-memory-context-${process.getuid?.() ?? "user"}`), signal) {
    const key = createHash("sha256").update(JSON.stringify([options.session, options.cwd || "", process.env.TASKIX_CONFIG || ""])).digest("hex");
    const path = join(directory, `memory-${key}.json`);
    await pendingReceipts.get(path);
    let seen = {};
    try {
        const value = JSON.parse(await readFile(path, "utf8"));
        if (value.expires > Date.now()) seen = value.items || {};
    } catch { /* Missing receipts are safe misses. */ }
    const included = new Set(packet.items.filter(item => typeof item.id === "string" && Number.isSafeInteger(item.revision) &&
        (!seen[item.id] || seen[item.id].revision < item.revision || seen[item.id].turn === turn)).map(item => item.id));
    const lines = packet.text.trimEnd().split("\n");
    const body = lines.slice(1).filter(line => { try { return included.has(JSON.parse(line).id); } catch { return false; } });
    if (!body.length) return "";
    for (const item of packet.items) if (included.has(item.id)) seen[item.id] = { revision: item.revision, turn };
    seen = Object.fromEntries(Object.entries(seen).slice(-2048));
    signal.throwIfAborted();
    return {
        text: `${lines[0]}\n${body.join("\n")}\n`,
        commit() {
            // Failed or dropped persistence can only cause duplicate delivery.
            if (pendingReceipts.size >= 64 && !pendingReceipts.has(path)) return;
            const pending = (async () => {
                await pendingReceipts.get(path);
                await mkdir(directory, { recursive: true, mode: 0o700 });
                const temporary = `${path}.${randomUUID()}`;
                try {
                    await writeFile(temporary, JSON.stringify({ expires: Date.now() + 30 * 86400000, items: seen }), { mode: 0o600, flag: "wx" });
                    await rename(temporary, path);
                } finally { await rm(temporary, { force: true }); }
            })().catch(() => {}).finally(() => {
                if (pendingReceipts.get(path) === pending) pendingReceipts.delete(path);
            });
            pendingReceipts.set(path, pending);
        },
    };
}

export async function memoryConfigured(path = process.env.TASKIX_CONFIG || join(homedir(), ".config", "taskix", "config.toml")) {
    let file;
    try {
        if (path.startsWith("~/")) path = join(homedir(), path.slice(2));
        file = await open(path, "r");
        const bytes = Buffer.alloc(256 * 1024 + 1);
        const { bytesRead } = await file.read(bytes, 0, bytes.length, 0);
        return bytesRead <= 256 * 1024 && parse(bytes.subarray(0, bytesRead).toString("utf8")).memory?.enabled === true;
    } catch { return false; }
    finally { await file?.close(); }
}
