import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runHook, registerExtension } from "../runtime.mjs";
import { visibleMessage, transcriptMessages } from "../conversation.mjs";

test("current-turn extraction does not parse historical transcript records", async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "taskix-transcript-budget-"));
    t.after(() => rm(directory, {recursive:true, force:true}));
    const path = join(directory, "session.jsonl");
    const history = Array.from({length:10000}, (_, id) => JSON.stringify({
        type:"response_item", payload:{type:"message",role:"assistant",id:`old-${id}`,content:"Old answer"},
    })).join("\n");
    for (const current of [
        [
            {type:"event_msg",payload:{type:"task_started",turn_id:"current"}},
            {type:"response_item",payload:{type:"message",role:"user",id:"u",content:"Request"}},
            {type:"response_item",payload:{type:"message",role:"assistant",id:"a",content:"Reply"}},
        ],
        [
            {type:"user",uuid:"u",message:{role:"user",content:"Request"}},
            {type:"assistant",uuid:"a",message:{role:"assistant",content:"Reply"}},
        ],
    ]) {
        await writeFile(path, history + "\n" + current.map(row => JSON.stringify(row)).join("\n"));
        const parse = JSON.parse;
        let parsed = 0;
        const mock = t.mock.method(JSON, "parse", (...args) => { parsed++; return parse(...args); });
        const messages = await transcriptMessages(path);
        mock.mock.restore();
        assert.deepEqual(messages.map(message => message.text), ["Request", "Reply"]);
        assert.ok(parsed <= 10, `parsed ${parsed} records for a two-message turn`);
    }
});

test("reverse transcript reads preserve long UTF-8 lines and partial tails", async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "taskix-transcript-lines-"));
    t.after(() => rm(directory, {recursive:true, force:true}));
    const path = join(directory, "session.jsonl");
    const content = "汉字🙂".repeat(25000);
    const records = [
        {type:"event_msg",payload:{type:"task_started",turn_id:"turn"}},
        {type:"response_item",payload:{type:"message",role:"user",id:"u",content:"Request"}},
        {type:"response_item",payload:{type:"message",role:"assistant",id:"a",content}},
    ];
    for (const ending of ["", "\r\n", '\r\n{"type":"response_item"']) {
        await writeFile(path, records.map(row => JSON.stringify(row)).join("\r\n") + ending);
        assert.deepEqual(await transcriptMessages(path), [
            {id:"turn:u",role:"user",text:"Request"},
            {id:"turn:a",role:"assistant",text:content},
        ]);
    }
    await writeFile(path, "");
    assert.deepEqual(await transcriptMessages(path), []);
});

test("fallback message IDs retain earlier turn context across a Claude user boundary", async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "taskix-transcript-identity-"));
    t.after(() => rm(directory, {recursive:true, force:true}));
    const path = join(directory, "session.jsonl");
    const rows = [
        {type:"event_msg",payload:{type:"task_started",turn_id:"turn"}},
        {type:"user",uuid:"old",message:{role:"user",content:"Old request"}},
        {type:"user",timestamp:"t1",message:{role:"user",content:"Request"}},
        {type:"assistant",timestamp:"t2",message:{role:"assistant",content:"Reply"}},
    ];
    await writeFile(path, rows.map(row => JSON.stringify(row)).join("\n"));
    assert.deepEqual(await transcriptMessages(path), [
        visibleMessage(rows[2].message, "turn:t1"),
        visibleMessage(rows[3].message, "turn:t2"),
    ]);
});

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
    const directory = await mkdtemp(join(tmpdir(), "taskix-transcript-test-"));
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

const codexMessage = (role, content, id) => ({type:"response_item",payload:{type:"message",role,content,id}});
const codexTurn = (id, mode, ...rows) => [
    {type:"event_msg",payload:{type:"task_started",turn_id:id}},
    {type:"turn_context",payload:{collaboration_mode:{mode}}},
    ...rows,
];
const planQuestions = {questions:[{id:"scope",header:"Scope",question:"Which scope?",options:[
    {label:"Local",description:"Only this project"}, {label:"Global",description:"All projects"},
]}]};
const questionCall = {type:"response_item",payload:{type:"function_call",name:"request_user_input",call_id:"choice",arguments:JSON.stringify(planQuestions)}};
const questionAnswer = {type:"response_item",payload:{type:"function_call_output",call_id:"choice",output:JSON.stringify({answers:{scope:{answers:["Local"]}}})}};

test("Codex planning captures questions, choices and answers but excludes other tools", async (t) => {
    const batches = await captureHook(t, codexTurn("plan", "plan",
        codexMessage("user", "Plan the change", "u"), questionCall, questionAnswer,
        {type:"response_item",payload:{type:"function_call",name:"exec_command",call_id:"shell",arguments:'{"cmd":"private"}'}},
        {type:"response_item",payload:{type:"function_call_output",call_id:"shell",output:'{"answers":{"secret":"private"}}'}},
        codexMessage("assistant", "<proposed_plan>\n# Plan\n\nUse local scope.\n</proposed_plan>", "a"),
    ));
    assert.deepEqual(batches[0].map(({role,text}) => ({role,text})), [
        {role:"user",text:"Plan the change"},
        {role:"assistant",text:"Which scope?\n\n- Local: Only this project\n- Global: All projects"},
        {role:"user",text:"Which scope?\nLocal"},
        {role:"assistant",text:"<proposed_plan>\n# Plan\n\nUse local scope.\n</proposed_plan>"},
    ]);
    assert.equal(new Set(batches[0].map(message => message.id)).size, 4);
});

test("implementing a plan recovers consecutive planning turns with stable identities", async (t) => {
    const plans = [
        ...codexTurn("p1", "plan", codexMessage("user", "Plan the change", "u"), questionCall, questionAnswer,
            codexMessage("assistant", "<proposed_plan>First plan</proposed_plan>", "a")),
        ...codexTurn("p2", "plan", codexMessage("user", "Use HOME as fallback", "u"),
            codexMessage("assistant", "<proposed_plan>Revised plan</proposed_plan>", "a")),
    ];
    const implementation = codexTurn("implementation", "default",
        codexMessage("user", "Implement the plan.", "u"), codexMessage("assistant", "Implemented", "a"));
    const prior = codexTurn("old", "default", codexMessage("user", "Unrelated work", "u"));
    const batches = await captureHook(t, [...prior, ...plans, ...implementation]);
    assert.equal(batches[0].messages[0].text, "Plan the change");
    assert.equal(batches[0].messages.at(-1).text, "Implemented");
    assert.equal(batches[0].messages.length, 8);
    assert.deepEqual(batches[0].planning, {prompt:"Plan the change",implementation_prompt:"Implement the plan."});
    const first = await captureHook(t, plans.slice(0, 6));
    assert.deepEqual(batches[0].messages.slice(0, 4), first[0]);
    const unrelated = await captureHook(t, [...plans, ...codexTurn("next", "default",
        codexMessage("user", "Fix a different bug", "u"), codexMessage("assistant", "Fixed", "a"))]);
    assert.deepEqual(unrelated[0].map(message => message.text), ["Fix a different bug", "Fixed"]);
});

test("planning recovery stops at execution turns and requires a proposed plan", async (t) => {
    for (const previous of [
        codexTurn("p", "plan", codexMessage("user", "Unfinished planning", "u")),
        [...codexTurn("p", "plan", codexMessage("user", "Old plan", "u"),
            codexMessage("assistant", "<proposed_plan>Old</proposed_plan>", "a")),
        ...codexTurn("other", "default", codexMessage("user", "Other work", "u"))],
    ]) {
        const batches = await captureHook(t, [...previous, ...codexTurn("i", "default",
            codexMessage("user", "Implement the plan.", "u"), codexMessage("assistant", "Done", "a"))]);
        assert.deepEqual(batches[0].map(message => message.text), ["Implement the plan.", "Done"]);
    }
});

test("question capture tolerates malformed results and keeps free text answers", async (t) => {
    const malformed = {...questionCall,payload:{...questionCall.payload,call_id:"bad",arguments:"{"}};
    const answers = {...questionAnswer,payload:{...questionAnswer.payload,output:JSON.stringify({answers:{scope:{answers:["Custom scope", "With exceptions"]}}})}};
    const batches = await captureHook(t, codexTurn("p", "plan",
        codexMessage("user", "Request", "u"), malformed, questionCall, answers,
        {type:"response_item",payload:{type:"function_call_output",call_id:"choice",output:"Tool failed"}},
    ));
    assert.deepEqual(batches[0].map(message => message.text), ["Request",
        "Which scope?\n\n- Local: Only this project\n- Global: All projects", "Which scope?\nCustom scope\nWith exceptions"]);
});
