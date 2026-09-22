// Offline reconstruction only. Never infer a historical owner from the source
// label or turn completed Jobs back into active candidates.
import { visibleMessage } from "../conversation.mjs";

// Legacy Job capture sometimes stores just the caption of an attached image.
// Match only complete leading transport wrappers backed by real image parts;
// never interpret the image, fuzzy-match the caption, or remove literal markup.
export function imageCaptionHistory(rows, sample) {
    if (!sample.prompt?.trim()) return undefined;
    const matches = rows.flatMap((row, index) => {
        const raw = row.payload, time = Date.parse(row.timestamp) / 1000;
        if (row.type !== "response_item" || raw?.type !== "message" || raw.role !== "user" ||
            !Array.isArray(raw.content) || !raw.content.some(p => p.type === "input_image")) return [];
        const message = visibleMessage(raw);
        const caption = message?.text.replace(/^(?:\s*<image name=\[Image #\d+\] path="[^"\n]+">\s*<\/image>\s*)+/, "");
        return caption !== message?.text && caption?.trim() === sample.prompt.trim() && Number.isFinite(time) &&
            (!Number.isFinite(sample.message_time) || Math.abs(time - sample.message_time) <= 10)
            ? [{ index, time }] : [];
    });
    if (matches.length !== 1) return undefined;
    const { index, time } = matches[0];
    const history = rows.slice(0, index).flatMap((row, i) => {
        if (row.type !== "response_item" || row.payload?.type !== "message") return [];
        const message = visibleMessage(row.payload, `${i}:${row.timestamp}`);
        return message ? [message] : [];
    }).slice(-64);
    return { message_time: time, history };
}

// Goal creation is a host-recorded user request, unlike recurring continuation
// wrappers. Only an unambiguous, fresh creation can recover a missing timestamp.
export function initialGoalHistory(rows, sample) {
    if (!sample.session_ref || !sample.prompt?.trim()) return undefined;
    const matches = rows.flatMap((row, index) => {
        const goal = row.payload?.goal, time = Date.parse(row.timestamp) / 1000;
        return row.type === "event_msg" && row.payload.type === "thread_goal_updated" &&
            row.payload.threadId === sample.session_ref && goal?.threadId === sample.session_ref &&
            goal.status === "active" && goal.tokensUsed === 0 && goal.timeUsedSeconds === 0 &&
            goal.objective?.trim() === sample.prompt.trim() && Number.isFinite(time) &&
            Number.isFinite(goal.createdAt) && time >= goal.createdAt && time - goal.createdAt < 1 &&
            (!Number.isFinite(sample.message_time) || Math.abs(time - sample.message_time) <= 10)
            ? [{ index, time }] : [];
    });
    if (matches.length !== 1) return undefined;
    const { index, time } = matches[0];
    const history = rows.slice(0, index).flatMap((row, i) => {
        if (row.type !== "response_item" || row.payload?.type !== "message") return [];
        const message = visibleMessage(row.payload, `${i}:${row.timestamp}`);
        return message ? [message] : [];
    }).slice(-64);
    return { message_time: time, history };
}

// Offline replay only: locate the actual user record before taking its prefix.
// A timestamp narrows duplicate text; an ambiguous match must not borrow future
// advice. The production router applies its normal message and byte budgets.
export function transcriptHistory(rows, sample) {
    const messages = rows.flatMap((row, index) => {
        if (row.type !== "response_item" || row.payload?.type !== "message") return [];
        const message = visibleMessage(row.payload, `${index}:${row.timestamp}`);
        return message ? [{ ...message, time: Date.parse(row.timestamp) / 1000 }] : [];
    });
    const matches = messages.flatMap((message, index) => message.role === "user" &&
        message.text.trim() === sample.prompt.trim() && Number.isFinite(message.time) &&
        (!Number.isFinite(sample.message_time) || Math.abs(message.time - sample.message_time) <= 10)
        ? [index] : []);
    if (matches.length !== 1) return undefined;
    const index = matches[0];
    return { message_time: messages[index].time,
        history: messages.slice(Math.max(0, index - 64), index).map(({ time, ...message }) => message) };
}

export function messageTime(message) {
    const pieces = String(message?.id || "").split(/:(?:msg_)?/).reverse();
    for (const piece of pieces) {
        if (!/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(piece)) continue;
        const seconds = parseInt(piece.replaceAll("-", "").slice(0, 12), 16) / 1000;
        if (seconds >= 1577836800 && seconds <= message.recorded_at + 1) return seconds;
    }
}

export function historicalContext(events, time, project) {
    const jobs = new Map(), tasks = new Map();
    if (Number.isFinite(time)) for (const event of events) {
        // Event timestamps have second precision: exclude the entire current
        // second because its ordering relative to the prompt is unknowable.
        if (event.occurred_at >= Math.floor(time)) continue;
        const value = event.payload;
        if (value?.id?.startsWith("job_") && value.project_id === project && value.status) jobs.set(value.id, value);
        if (value?.id?.startsWith("task_") && value.job_id && value.status) tasks.set(value.id, value);
    }
    const candidates = [...jobs.values()].filter(job => !job.archived_at && ["ACTIVE", "PENDING_REVIEW"].includes(job.status))
        .map(job => ({ job, tasks: [...tasks.values()].filter(task => task.job_id === job.id) }));
    return { project_id: project, previous_job: null, inbox_todos: [],
        routing: { complete: Number.isFinite(time), candidates } };
}

export function reconstructHistory(data, jobs, events) {
    const byId = new Map(jobs.map(job => [job.id, job]));
    return { ...data,
        method: { source: "Historical event snapshots strictly before each user message", original_method: data.method,
            limitations: "No historical Inbox, assignment lease or previous_job reconstruction; same-second events excluded; unavailable message times stay in denominator as incomplete snapshots" },
        cases: data.cases.map(sample => {
            const conversation = byId.get(sample.source_job_id)?.conversation || [];
            const previousId = sample.history?.at(-1)?.id;
            const start = previousId ? conversation.findIndex(message => message.id === previousId) + 1 : 0;
            const message = conversation.slice(start).find(message => message.role === "user" && message.text === sample.prompt);
            const time = messageTime(message);
            const turn = message?.id?.split(":")[0];
            const history = (sample.history || []).filter(prior =>
                (!message?.session_id || prior.session_id === message.session_id) &&
                (!turn || prior.id?.split(":")[0] !== turn));
            return { ...sample, history, message_time: time, timestamp_available: Number.isFinite(time),
                session_ref: message?.session_id || sample.session_ref,
                context: historicalContext(events, time, sample.context.project_id) };
        }),
    };
}
