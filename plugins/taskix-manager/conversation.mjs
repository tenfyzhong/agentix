import { createHash } from "node:crypto";
import { mkdtemp, open, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

function userText(text) {
    // Hosts may put their context in user-role messages. Match complete leading
    // wrappers, not mentions of AGENTS.md in an actual request.
    for (;;) {
        const value = text.trimStart();
        let block = value;
        if (value.startsWith("# AGENTS.md instructions\n")) {
            block = value.slice(value.indexOf("\n") + 1).trimStart();
            if (!block.startsWith("<INSTRUCTIONS>")) return text;
        }
        const tag = ["INSTRUCTIONS", "environment_context", "system-reminder", "turn_aborted"]
            .find(tag => block.startsWith(`<${tag}>`));
        if (!tag || (tag === "INSTRUCTIONS" && block === value)) return text;
        const end = block.indexOf(`</${tag}>`);
        if (end < 0) return text;
        text = block.slice(end + tag.length + 3).trimStart();
    }
}

export function visibleMessage(message, identity = "") {
    if (!message || !["user", "assistant"].includes(message.role)) return undefined;
    if (message.recipient && message.recipient !== "all") return undefined;
    if (["analysis", "summary"].includes(message.channel || message.phase)) return undefined;
    let text = typeof message.content === "string" ? message.content :
        (message.content || []).filter(part => ["text", "input_text", "output_text"].includes(part.type))
            .map(part => part.text || "").join("\n");
    if (message.role === "user") text = userText(text);
    if (!text.trim()) return undefined;
    const id = message.id || createHash("sha256").update(JSON.stringify([identity, message.timestamp, message.role, text])).digest("hex");
    return { id, role: message.role, text };
}

async function* reverseLines(path) {
    const file = await open(path, "r");
    try {
        let position = (await file.stat()).size;
        let fragments = [];
        while (position > 0) {
            const length = Math.min(position, 64 * 1024);
            position -= length;
            const buffer = Buffer.allocUnsafe(length);
            let filled = 0;
            while (filled < length) {
                const {bytesRead} = await file.read(buffer, filled, length - filled, position + filled);
                if (!bytesRead) break;
                filled += bytesRead;
            }
            let end = filled;
            for (let index = filled - 1; index >= 0; index--) {
                if (buffer[index] !== 10) continue;
                fragments.push(buffer.subarray(index + 1, end));
                // Decode complete lines so chunk boundaries cannot split UTF-8.
                yield Buffer.concat(fragments.reverse()).toString("utf8");
                fragments = [];
                end = index;
            }
            fragments.push(buffer.subarray(0, end));
        }
        if (fragments.length) yield Buffer.concat(fragments.reverse()).toString("utf8");
    } finally {
        await file.close();
    }
}

function parsed(value) {
    try { return typeof value === "string" ? JSON.parse(value) : value; } catch { return undefined; }
}

function turnMessages(rows, turn) {
    const questions = new Map();
    return rows.reverse().flatMap(row => {
        const item = row.type === "response_item" ? row.payload : undefined;
        if (item?.type === "function_call" && item.name === "request_user_input" && item.call_id) {
            const list = parsed(item.arguments)?.questions;
            if (!Array.isArray(list)) return [];
            const valid = list.filter(q => typeof q.id === "string" && typeof q.question === "string");
            questions.set(item.call_id, valid);
            const text = valid.map(q => {
                const options = (Array.isArray(q.options) ? q.options : [])
                    .filter(option => typeof option.label === "string")
                    .map(option => `- ${option.label}${typeof option.description === "string" ? `: ${option.description}` : ""}`);
                return q.question + (options.length ? `\n\n${options.join("\n")}` : "");
            }).join("\n\n");
            return text ? [{id:`${turn}:${item.call_id}:question`,role:"assistant",text}] : [];
        }
        if (item?.type === "function_call_output" && questions.has(item.call_id)) {
            const answers = parsed(item.output)?.answers;
            const text = questions.get(item.call_id).flatMap(q => {
                const values = answers?.[q.id]?.answers;
                if (!Array.isArray(values)) return [];
                const strings = values.filter(value => typeof value === "string");
                return strings.length ? [`${q.question}\n${strings.join("\n")}`] : [];
            }).join("\n\n");
            return text ? [{id:`${turn}:${item.call_id}:answer`,role:"user",text}] : [];
        }
        const codex = item?.type === "message";
        const raw = codex ? item : row.message;
        const message = !row.isMeta && visibleMessage(raw, row.uuid || `${turn}:${row.timestamp}`);
        if (!message) return [];
        if (codex && raw.id && turn) message.id = `${turn}:${raw.id}`;
        if (!codex && row.uuid) message.id = row.uuid;
        return [message];
    });
}

export async function transcriptConversation(path) {
    let rows = [], current, planning = [], mode, turn = "";
    let needsTurn = false, foundUser = false, recovering = false;
    for await (const line of reverseLines(path)) {
        if (!line.trim()) continue;
        const row = parsed(line);
        if (!row) continue;
        if (row.type === "turn_context") {
            mode = row.payload?.collaboration_mode?.mode;
            // Never traverse an older execution turn to recover a plan.
            if (recovering && mode !== "plan") break;
        }
        const boundary = row.type === "event_msg" && row.payload?.type === "task_started";
        if (boundary) turn = row.payload.turn_id || row.timestamp;
        const codex = row.type === "response_item";
        const raw = codex ? row.payload : row.message;
        const message = !foundUser && !row.isMeta && visibleMessage(raw);
        if (!foundUser) rows.push(row);
        if (message) {
            needsTurn ||= (codex && !!raw.id) || (!raw.id && !row.uuid);
            if (!codex && message.role === "user") foundUser = true;
        }
        if (boundary || (foundUser && !needsTurn)) {
            const messages = turnMessages(rows, turn);
            if (!current) {
                current = messages;
                const input = messages.find(message => message.role === "user")?.text.trim();
                // Codex's native accept-plan action submits this exact prompt.
                if (!boundary || input !== "Implement the plan." || mode === "plan") break;
                recovering = true;
            } else {
                if (mode !== "plan") break;
                planning = [...messages, ...planning];
            }
            rows = []; mode = undefined; turn = ""; needsTurn = false; foundUser = false;
        }
    }
    current ??= turnMessages(rows, turn);
    if (planning.some(message => message.role === "assistant" && message.text.includes("<proposed_plan>"))) {
        const prompt = planning.find(message => message.role === "user")?.text;
        if (prompt) return {messages:[...planning, ...current],planning:{prompt,implementation_prompt:"Implement the plan."}};
    }
    return {messages:current};
}

export async function transcriptMessages(path) {
    return (await transcriptConversation(path)).messages;
}

export async function recordMessages(messages, runner, options, planning) {
    if (!messages.length) return;
    const directory = await mkdtemp(join(tmpdir(), "taskix-conversation-"));
    try {
        const path = join(directory, "messages.json");
        await writeFile(path, JSON.stringify(planning ? {messages, planning} : messages), {mode:0o600});
        const response = await runner(["hook", "record", "--file", path], options);
        if (response.projection_pending) throw new Error(`Conversation saved; document synchronization is pending: ${response.projection_pending}`);
    } finally {
        await rm(directory, {recursive:true, force:true});
    }
}
