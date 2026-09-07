import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runHook, registerExtension } from "../runtime.mjs";
import { visibleMessage } from "../conversation.mjs";

test("user context wrappers are excluded while real requests about AGENTS.md survive", () => {
    const context = "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nInjected rules\n</INSTRUCTIONS><environment_context>\ncwd: /work\n</environment_context>";
    for (const content of [context, "<environment_context>cwd: /work</environment_context>", "<system-reminder>Injected reminder</system-reminder>"]) {
        assert.equal(visibleMessage({role:"user",content}), undefined);
    }
    assert.equal(visibleMessage({role:"user",content:context + "\n\nPlease update AGENTS.md."}).text, "Please update AGENTS.md.");
    for (const text of ["Please update AGENTS.md.", "# AGENTS.md instructions\nExplain this heading.", "Show `<environment_context>` as an example."]) {
        assert.equal(visibleMessage({role:"user",content:text}).text, text);
    }
});

async function captureHook(t, records) {
    const directory = await mkdtemp(join(tmpdir(), "taskcli-transcript-test-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const transcript = join(directory, "session.jsonl");
    await writeFile(transcript, records.map(record => JSON.stringify(record)).join("\n") + "\n");
    const batches = [];
    const runner = async (args) => {
        if (args[1] === "record") batches.push(JSON.parse(await readFile(args[args.indexOf("--file") + 1], "utf8")));
        return { result: {} };
    };
    await runHook({ hook_event_name: "Stop", session_id: "s", transcript_path: transcript }, runner);
    return batches;
}

test("Codex Stop records the current turn's visible messages without tools or context", async (t) => {
    const message = (role, text, id) => ({ type: "response_item", payload: { type: "message", id, role, content: [{type: role === "assistant" ? "output_text" : "input_text", text}] } });
    const batches = await captureHook(t, [
        {type:"event_msg", payload:{type:"task_started",turn_id:"old"}},
        message("assistant", "Old answer", "old"),
        {type:"event_msg", payload:{type:"task_started",turn_id:"current"}},
        message("developer", "Injected instructions", "system"),
        message("user", "# AGENTS.md instructions\n<INSTRUCTIONS>Injected rules</INSTRUCTIONS><environment_context>cwd: /work</environment_context>", "rules"),
        message("user", "Exact request", "u"),
        {type:"response_item",payload:{type:"function_call_output",output:"Secret tool log"}},
        {type:"response_item",payload:{type:"reasoning",summary:"Private reasoning"}},
        message("assistant", "Visible reply", "a"),
    ]);
    assert.equal(batches.length, 1);
    assert.deepEqual(batches[0].map(message => message.id), ["current:u", "current:a"]);
    assert.deepEqual(batches[0].map(({role, text}) => ({role, text})), [
        {role:"user",text:"Exact request"}, {role:"assistant",text:"Visible reply"}
    ]);
});

test("Claude Stop filters tool blocks and tool-result user messages", async (t) => {
    const batches = await captureHook(t, [
        {type:"user",uuid:"u",message:{role:"user",content:"Request"}},
        {type:"assistant",uuid:"a",message:{role:"assistant",content:[{type:"text",text:"Reply"},{type:"tool_use",name:"exec",input:{command:"private"}}]}},
        {type:"user",uuid:"tool",message:{role:"user",content:[{type:"tool_result",content:"private result"}]}},
    ]);
    assert.equal(batches.length, 1);
    assert.deepEqual(batches[0].map(message => message.text), ["Request", "Reply"]);
});

test("Claude transcript entries sharing an API message ID retain each visible block", async (t) => {
    const batches = await captureHook(t, [
        {type:"user",uuid:"u",message:{role:"user",content:"Request"}},
        {type:"assistant",uuid:"a1",message:{id:"api-message",role:"assistant",content:[{type:"text",text:"First"}]}},
        {type:"assistant",uuid:"a2",message:{id:"api-message",role:"assistant",content:[{type:"text",text:"Second"}]}},
    ]);
    assert.equal(new Set(batches[0].map(message => message.id)).size, 3);
});

test("Pi and OMP agent_end persist prompt and visible output only", async () => {
    for (const host of ["pi", "omp"]) {
        const handlers = new Map(), batches = [];
        const runner = async (args) => {
            if (args[1] === "record") batches.push(JSON.parse(await readFile(args[args.indexOf("--file") + 1], "utf8")));
            return {result:{}};
        };
        registerExtension({ on:(name, fn) => handlers.set(name,fn),registerTool(){} }, host, runner,
            {setInterval(){return 1;},clearInterval(){}});
        const ctx = {cwd:"/work",sessionManager:{getSessionId:()=>"s"}};
        await handlers.get("session_start")({},ctx);
        await handlers.get("before_agent_start")({prompt:"User prompt"},ctx);
        await handlers.get("agent_end")({messages:[
            {role:"assistant",timestamp:1,content:[{type:"thinking",thinking:"private"},{type:"toolCall",name:"exec"},{type:"text",text:"Agent reply"}]},
            {role:"toolResult",content:[{type:"text",text:"private tool output"}]},
        ]},ctx);
        assert.equal(batches.length,1);
        assert.deepEqual(batches[0].map(({role,text})=>({role,text})),[{role:"user",text:"User prompt"},{role:"assistant",text:"Agent reply"}]);
        await handlers.get("session_shutdown")({},ctx);
    }
});
