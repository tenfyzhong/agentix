import {
    appendFileSync,
    mkdirSync,
    readFileSync,
    truncateSync,
    writeFileSync,
    renameSync,
} from 'node:fs';
import { createHash, randomBytes } from 'node:crypto';
import { join } from 'node:path';
import { DurableQueue } from '../queue.mjs';

// Match OpenCode's ascending opaque ID format, including its timestamp prefix.
let counter = 0,
    lastTimestamp = 0;
export function messageId() {
    const timestamp = Date.now();
    if (timestamp !== lastTimestamp) {
        lastTimestamp = timestamp;
        counter = 0;
    }
    const time =
        (BigInt(timestamp) * 4096n + BigInt(++counter)) & ((1n << 48n) - 1n);
    return (
        'msg_' +
        time.toString(16).padStart(12, '0') +
        randomBytes(7).toString('hex')
    );
}

export function openQueue(directory, session, changed, options = {}) {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const file = join(
        directory,
        createHash('sha256').update(session).digest('hex') + '.jsonl',
    );
    let bytes;
    try {
        bytes = readFileSync(file);
    } catch (error) {
        if (error.code !== 'ENOENT') throw error;
    }
    const records = [];
    if (bytes?.length) {
        const end = bytes.lastIndexOf(10) + 1;
        for (const line of bytes
            .subarray(0, end)
            .toString('utf8')
            .split('\n')
            .filter(Boolean))
            records.push(JSON.parse(line));
        if (end !== bytes.length) truncateSync(file, end);
    }
    let queue,
        recordsSinceCheckpoint = 0,
        journalBytes = bytes?.length ?? 0;
    const restored = DurableQueue.restore(records);
    const archiveDir = file + '.receipts';
    mkdirSync(archiveDir, { recursive: true, mode: 0o700 });
    const receiptFile = (id) =>
        join(
            archiveDir,
            createHash('sha256').update(id).digest('hex') + '.json',
        );
    const atomicWrite = (target, content) => {
        const temporary =
            target + '.' + randomBytes(8).toString('hex') + '.tmp';
        writeFileSync(temporary, content, { mode: 0o600 });
        renameSync(temporary, target);
    };
    const archive = (id, value) => {
        const target = receiptFile(id),
            content = JSON.stringify({ id, ...value });
        try {
            if (readFileSync(target, 'utf8') === content) return;
        } catch (error) {
            if (error.code !== 'ENOENT') throw error;
        }
        atomicWrite(target, content);
    };
    const lookup = (id) => {
        try {
            const value = JSON.parse(readFileSync(receiptFile(id), 'utf8'));
            if (value.id !== id)
                throw new Error('Delivery receipt identity mismatch');
            return value;
        } catch (error) {
            if (error.code !== 'ENOENT') throw error;
        }
    };
    const persist = (record) => {
        if (queue) queue.trimReceipts(options.maxReceipts ?? 2048, archive);
        const delta = record.queue_delta;
        const id = delta?.id ?? delta?.item?.id;
        const admission = delta && ['enqueue', 'remember'].includes(delta.op);
        const line = JSON.stringify(record) + '\n';
        if (admission) {
            const text = delta.item?.text ?? delta.intent?.text ?? '';
            if (Buffer.byteLength(text) > 256 * 1024)
                throw new Error(
                    'Prompt exceeds OpenCode delivery capacity (256 KiB)',
                );
            if (delta.op === 'enqueue') {
                const items = queue.view().items;
                if (
                    items.length >= 128 ||
                    items.reduce(
                        (n, item) => n + Buffer.byteLength(item.text),
                        0,
                    ) +
                        Buffer.byteLength(text) >
                        8 * 1024 * 1024
                )
                    throw new Error(
                        'OpenCode queue capacity reached (128 prompts or 8 MiB)',
                    );
            }
        }
        if (
            queue &&
            (recordsSinceCheckpoint >= (options.checkpointRecords ?? 4096) ||
                journalBytes >= 1024 * 1024)
        ) {
            queue.trimReceipts(options.maxReceipts ?? 2048, archive);
            const checkpoint = JSON.stringify(queue.state()) + '\n';
            atomicWrite(file, checkpoint);
            journalBytes = 0;
            recordsSinceCheckpoint = 0;
        }
        appendFileSync(file, line, { mode: 0o600 });
        journalBytes += Buffer.byteLength(line);
        recordsSinceCheckpoint++;
        if (record.queue_delta) changed?.();
    };
    if (!bytes) persist({ queue: [], session_id: session });
    queue = new DurableQueue(restored, persist, lookup);
    queue.trimReceipts(options.maxReceipts ?? 2048, archive);
    return { file, queue };
}
