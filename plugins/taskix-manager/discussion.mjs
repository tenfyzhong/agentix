import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { classifyDiscussionTurns, jevConfig } from "./jev.mjs";
import { transcriptConversation, recordMessages } from "./conversation.mjs";

export function discussionNotice(turn) {
    return `Current discussion turn: ${JSON.stringify(turn)}. Before creating, following up, or continuing a Job, select its related pending turns using the taskix-manager discussion workflow. Jev first: node ${JSON.stringify(fileURLToPath(import.meta.url))} SESSION_ID, with JSON {"current_turn":TURN_ID,"target":{"title":"delivery title","goal":"acceptance goal","prompt":"current verbatim prompt","job_id":"existing Job only"}} on stdin. This helper only classifies; use its guarded arguments for the actual write. If it returns status=agent, read pending turns and decide their ownership. Always include the current turn.`;
}

// Cache only a turn identity after successful staging, never message contents.
export async function stageTranscript(event, runner, options, directory = join(tmpdir(), `taskix-discussion-${process.getuid?.() ?? "user"}`)) {
    if (!event.transcript_path) return;
    const key = createHash("sha256").update(JSON.stringify([options.session,options.cwd,event.transcript_path])).digest("hex");
    const path = join(directory,`${key}.capture.json`);
    let receipt;
    try { receipt = JSON.parse(await readFile(path,"utf8")); } catch {}
    const initial = event.hook_event_name === "PreToolUse";
    if (initial && event.turn_id && receipt?.turn === event.turn_id && receipt.expires > Date.now()) return;
    const capture = await transcriptConversation(event.transcript_path,{currentOnly:true});
    if (!capture.turn_id || !capture.messages.length) return;
    if (initial && receipt?.turn === capture.turn_id && receipt.expires > Date.now()) return;
    await recordMessages(capture.messages,runner,options,undefined,{turn_id:capture.turn_id,source:"transcript"});
    if (initial) {
        await mkdir(directory,{recursive:true,mode:0o700});
        const temporary = `${path}.${process.pid}.tmp`;
        try {
            await writeFile(temporary,JSON.stringify({turn:capture.turn_id,expires:Date.now()+3600000}),{mode:0o600});
            await rename(temporary,path);
        } finally { await rm(temporary,{force:true}); }
        return discussionNotice(capture.turn_id);
    }
}

export async function selectDiscussion({target,current_turn:currentTurn}, options, runner, settings = {}) {
    if (!target?.title?.trim() || !target?.prompt?.trim() || typeof currentTurn !== "string") return {status:"agent",reason:"missing_target"};
    const controller = new AbortController();
    const timer = setTimeout(()=>controller.abort(),8000);
    const scoped = {...options,signal:options.signal ? AbortSignal.any([options.signal,controller.signal]) : controller.signal};
    try {
        let job;
        if (target.job_id) {
            job = (await runner(["job","show",target.job_id],scoped)).result;
            if (!job || !["ACTIVE","PENDING_REVIEW"].includes(job.status) || job.archived_at) return {status:"agent",reason:"invalid_target"};
        }
        const pending = (await runner(["conversation","list","--limit","100"],scoped)).result;
        let result;
        if (pending?.complete !== true || !Number.isSafeInteger(pending.revision) || !pending.turns?.some(turn=>turn.turn_id===currentTurn)) {
            result = {status:"agent",reason:"incomplete_context"};
        } else if (pending.turns.length === 1) {
            result = {status:"selected",turn_ids:[currentTurn],revision:pending.revision};
        } else if (!jevConfig(settings.env)) {
            result = {status:"agent",reason:"disabled"};
        } else if (pending.turns.reduce((sum,turn)=>sum+(turn.bytes || 0),0)>24000) {
            result = {status:"agent",reason:"context_too_large"};
        } else {
            const full = (await runner(["conversation","list","--limit","100","--full"],scoped)).result;
            result = full.revision !== pending.revision ? {status:"agent",reason:"candidates_changed"}
                : await classifyDiscussionTurns({target:job ? {...target,job} : target,pending:full,currentTurn,...settings,signal:scoped.signal});
        }
        if (result.status !== "selected") return {...result,revision:pending?.revision,read_args:["conversation","list","--limit","100"]};
        // Classification cannot authorize stale draft ownership or a changed Job.
        const fresh = (await runner(["conversation","list","--limit","1"],scoped)).result;
        if (fresh.revision !== result.revision) return {status:"agent",reason:"candidates_changed"};
        if (job) {
            const current = (await runner(["routing","revision",job.id],scoped)).result;
            if (current?.revision !== job.revision || current.status !== job.status || current.archived_at) return {status:"agent",reason:"target_changed"};
        }
        const flag = job?.status === "ACTIVE" ? "--turn" : "--conversation-turn";
        const targetGuard = !job ? ["--conversation-target",createHash("sha256").update(JSON.stringify([target.title,target.goal || "",target.prompt])).digest("hex")] : [];
        return {...result,target,job_revision:job?.revision,
            args:[...targetGuard,"--conversation-revision",String(result.revision),...(job?["--expect-revision",String(job.revision)]:[]),...result.turn_ids.flatMap(id=>[flag,id])]};
    } catch { return {status:"agent",reason:"discussion_unavailable"}; }
    finally { clearTimeout(timer); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    try {
        const session = process.argv[2];
        if (!session || process.argv.length !== 3) throw new Error("Session required");
        let input="";
        for await (const chunk of process.stdin) { input+=chunk; if (Buffer.byteLength(input)>1024*1024) throw new Error("Input too large"); }
        const {runTaskix} = await import("./runtime.mjs");
        process.stdout.write(JSON.stringify(await selectDiscussion(JSON.parse(input),{session,cwd:process.cwd()},runTaskix))+"\n");
    } catch { process.stdout.write(JSON.stringify({status:"agent",reason:"invalid_input"})+"\n"); process.exitCode=1; }
}
