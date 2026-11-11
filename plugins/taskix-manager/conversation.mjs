import { createHash } from "node:crypto";
import { mkdtemp, open, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** Unwrap native user input without treating ordinary inline tags as an envelope. */
function nativeUserText(text) {
    const envelope = /^\s*<USER_REQUEST>([\s\S]*?)<\/USER_REQUEST>(?:\s*<(ADDITIONAL_METADATA|USER_SETTINGS_CHANGE)>[\s\S]*?<\/\2>)*\s*$/.exec(text);
    return envelope ? envelope[1].trim() : text;
}

function userText(text) {
    // Hosts may put their context in user-role messages. Match complete leading
    // wrappers, not mentions of AGENTS.md in an actual request.
    for (;;) {
        const value = text.trimStart();
        let block = value;
        if (/^# AGENTS\.md instructions(?: for [^\n]+)?\n/.test(value)) {
            block = value.slice(value.indexOf("\n") + 1).trimStart();
            if (!block.startsWith("<INSTRUCTIONS>")) return text;
        }
        const tag = ["INSTRUCTIONS", "environment_context", "system-reminder", "turn_aborted",
            "recommended_plugins", "codex_internal_context"]
            .find(tag => block.startsWith(`<${tag}>`) ||
                (tag === "codex_internal_context" && /^<codex_internal_context\s[^>]*>/.test(block)));
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

class TranscriptBudgetError extends Error {
    constructor() { super("Transcript routing budget exceeded"); }
}

async function* reverseLines(path, { signal, maxBytes = Infinity } = {}) {
    signal?.throwIfAborted();
    const file = await open(path, "r");
    try {
        let position = (await file.stat()).size;
        let fragments = [];
        let remaining = maxBytes;
        while (position > 0) {
            signal?.throwIfAborted();
            if (remaining <= 0) throw new TranscriptBudgetError();
            const length = Math.min(position, 64 * 1024, remaining);
            remaining -= length;
            position -= length;
            const buffer = Buffer.allocUnsafe(length);
            let filled = 0;
            while (filled < length) {
                const {bytesRead} = await file.read(buffer, filled, length - filled, position + filled);
                if (!bytesRead) break;
                filled += bytesRead;
            }
            let end = filled;
            signal?.throwIfAborted();
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
    const completed = rows.find(row => row.type === "event_msg" && row.payload?.type === "task_complete" && (!row.payload.turn_id || row.payload.turn_id === turn));
    const questions = new Map();
    let nativeQuestions = [];
    const messages = rows.reverse().flatMap(row => {
        if (row.antigravity) {
            const step = row.antigravity, messages = [];
            if (step.type === "PLANNER_RESPONSE") {
                nativeQuestions = [];
                if (typeof step.content === "string" && step.content.trim()) messages.push({ id: row.uuid, role: "assistant", text: step.content });
                for (const tool of Array.isArray(step.tool_calls) ? step.tool_calls : []) {
                    if (tool.name !== "ask_question") continue;
                    const list = parsed(tool.args?.questions);
                    if (!Array.isArray(list)) continue;
                    nativeQuestions = list.filter(question => typeof question.question === "string");
                    for (const [index, question] of nativeQuestions.entries()) {
                        const options = (Array.isArray(question.options) ? question.options : []).filter(value => typeof value === "string");
                        messages.push({ id: `${row.uuid}:question:${index}`, role: "assistant",
                            text: question.question + (options.length ? "\n\n" + options.map(value => `- ${value}`).join("\n") : "") });
                    }
                }
            } else if (step.type === "GENERIC" && typeof step.content === "string" && nativeQuestions.length) {
                for (const match of step.content.matchAll(/(?:^|\n)A(\d+): ([\s\S]*?)(?=\nA\d+: |$)/g)) {
                    const index = Number(match[1]) - 1, question = nativeQuestions[index];
                    if (question && match[2].trim()) messages.push({ id: `${row.uuid}:answer:${index}`, role: "user", text: `${question.question}\n${match[2].trim()}` });
                }
                nativeQuestions = [];
            }
            return messages;
        }
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
        if (message.role === "user") {
            const timestamp = raw.timestamp ?? row.timestamp;
            const millis = typeof timestamp === "number" ? timestamp : Date.parse(timestamp);
            if (Number.isSafeInteger(millis) && millis >= 0) message.completed_at_ms = millis;
        }
        return [message];
    });
    const last = messages.findLast(message => message.role === "assistant");
    const finished = Date.parse(completed?.timestamp);
    if (last && Number.isSafeInteger(finished) && finished >= 0) last.completed_at_ms = finished;
    return messages;
}

export async function transcriptConversation(path, options) {
    let rows = [], current, planning = [], mode, turn = "", currentTurn;
    const nativeSteps = new Set();
    let needsTurn = false, foundUser = false, recovering = false;
    const recentLimit = !options?.currentOnly && Number.isInteger(options?.recentTurns)
        ? Math.min(8, Math.max(1, options.recentTurns)) : 0;
    let recent = [], recentCount = 0;
    let historyTruncated = false;
    try {
        for await (const line of reverseLines(path, options)) {
            if (!line.trim()) continue;
            let row = parsed(line);
            if (!row) continue;
            if (Number.isSafeInteger(row.step_index) && row.step_index >= 0) {
                if (nativeSteps.has(row.step_index)) continue;
                nativeSteps.add(row.step_index);
                const user = row.type === "USER_INPUT" && row.source === "USER_EXPLICIT";
                const assistant = ["PLANNER_RESPONSE", "GENERIC"].includes(row.type) && row.source === "MODEL";
                if (!user && !assistant) continue;
                const id = `${options?.sessionId ?? "antigravity"}:step:${row.step_index}`;
                if (user) {
                    if (typeof row.content !== "string") continue;
                    turn = id;
                    row = { uuid: id, message: { role: "user", content: nativeUserText(row.content) } };
                } else row = { uuid: id, antigravity: row };
            }
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
                if (recentLimit) {
                    currentTurn ??= turn || messages.find(message => message.role === "user")?.id;
                    recent = [...messages, ...recent];
                    rows = []; mode = undefined; turn = ""; needsTurn = false; foundUser = false;
                    if (++recentCount >= recentLimit) break;
                    continue;
                }
                if (!current) {
                    current = messages;
                    currentTurn = turn || messages.find(message => message.role === "user")?.id;
                    const input = messages.find(message => message.role === "user")?.text.trim();
                    // Codex's native accept-plan action submits this exact prompt.
                    if (options?.currentOnly || !boundary || input !== "Implement the plan." || mode === "plan") break;
                    recovering = true;
                } else {
                    if (mode !== "plan") break;
                    planning = [...messages, ...planning];
                }
                rows = []; mode = undefined; turn = ""; needsTurn = false; foundUser = false;
            }
        }
    } catch (error) {
        if (!(error instanceof TranscriptBudgetError) || !recentLimit) throw error;
        // Every yielded row is a complete JSON record, even when the beginning
        // of its turn is outside the budget. Preserve visible messages already
        // read; incomplete JSON fragments never reach turnMessages.
        const partial = turnMessages(rows, turn);
        if (!recentCount && !partial.length) throw error;
        recent = [...partial, ...recent];
        rows = [];
        historyTruncated = true;
    }
    if (recentLimit) return { messages: [...turnMessages(rows, turn), ...recent], turn_id: currentTurn,
        ...(historyTruncated ? { history_truncated: true } : {}) };
    current ??= turnMessages(rows, turn);
    if (planning.some(message => message.role === "assistant" && message.text.includes("<proposed_plan>"))) {
        const prompt = planning.find(message => message.role === "user")?.text;
        if (prompt) return {messages:[...planning, ...current],planning:{prompt,implementation_prompt:"Implement the plan."}};
    }
    return {messages:current, turn_id:currentTurn || turn || current.find(message => message.role === "user")?.id};
}

export async function transcriptMessages(path) {
    return (await transcriptConversation(path)).messages;
}

export async function recordMessages(messages, runner, options, planning, capture) {
    if (!messages.length) return;
    const directory = await mkdtemp(join(tmpdir(), "taskix-conversation-"));
    try {
        const path = join(directory, "messages.json");
        await writeFile(path, JSON.stringify(capture ? {messages, ...capture} : planning ? {messages, planning} : messages), {mode:0o600});
        const response = await runner(["hook", "record", "--file", path], options);
        if (response.projection_pending) throw new Error(`Conversation saved; document synchronization is pending: ${response.projection_pending}`);
    } finally {
        await rm(directory, {recursive:true, force:true});
    }
}
