import { appendFileSync, readFileSync, truncateSync } from 'node:fs';
import { atomicJson, readJson } from './mailbox.mjs';

/** A legacy checkpoint followed by per-turn/per-receipt updates, never replayed as prompts. */
export class ClaudeStateStore {
    constructor(path) { this.path = path; this.journal = path + 'l'; this.size = 0; }
    load(initialTurns = () => []) {
        let saved = readJson(this.path);
        if (!saved) {
            saved = { turns: initialTurns(), receipts: [] };
            atomicJson(this.path, saved);
        }
        const turns = new Map(saved.turns.map(turn => [turn.id, turn]));
        const receipts = new Map(saved.receipts ?? []);
        let bytes;
        try { bytes = readFileSync(this.journal); }
        catch (error) { if (error.code !== 'ENOENT') throw error; bytes = Buffer.alloc(0); }
        const end = bytes.lastIndexOf(10) + 1;
        // A killed append leaves an incomplete final record. Never append onto that fragment.
        for (const line of bytes.subarray(0, end).toString('utf8').split('\n').filter(Boolean)) {
            const record = JSON.parse(line);
            if (record.turn) turns.set(record.turn.id, record.turn);
            for (const [id, receipt] of record.receipts ?? []) {
                if (receipt === null) receipts.delete(id);
                else receipts.set(id, receipt);
            }
        }
        if (end !== bytes.length) truncateSync(this.journal, end);
        this.size = end;
        return { turns: [...turns.values()], receipts: [...receipts] };
    }
    append(record) {
        const line = JSON.stringify(record) + '\n';
        try { appendFileSync(this.journal, line, { mode: 0o600 }); }
        catch (error) {
            // Keep the journal usable if a write failed after writing only part of a record.
            try { truncateSync(this.journal, this.size); } catch {}
            throw error;
        }
        this.size += Buffer.byteLength(line);
    }
}
