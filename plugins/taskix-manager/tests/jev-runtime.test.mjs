import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, rm, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runHook, registerExtension } from "../runtime.mjs";

const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://jev.test/v1/systemone", TASKIX_JEV_API_KEY: "private" };
const job = {id:"job_a",project_id:"p",revision:1,status:"PENDING_REVIEW",title:"Login",prompt:"Fix login",review_policy:"required"};
async function fixture(t, choice="followup:job_a") {
    const cacheDir=await mkdtemp(join(tmpdir(),"taskix-routing-test-"));
    t.after(()=>rm(cacheDir,{recursive:true,force:true}));
    const context={project_id:"p",previous_job:job,inbox_todos:[{id:"inbox_other",content:"Unrelated Inbox body"}]};
    const calls=[];
    let requests=0;
    const runner=async args=>{
        calls.push(args);
        if(args[0]==="context")return {result:context};
        if(args[1]==="snapshot")return {result:{...context,routing:{complete:true,candidates:[{job,tasks:[]}]}}};
        if(args[1]==="candidates")return {result:{complete:true,candidates:[{job,tasks:[]}]}};
        if(args[1]==="revision")return {result:job};
        if(args[0]==="hook")return {result:{inbox_cancellations:context.inbox_cancellations || []}};
        assert.fail(`Unexpected write ${args}`);
    };
    const routing={env,cacheDir,fetch:async(_url,init)=>{
        requests++;
        const q=JSON.parse(init.body).questions;
        return {ok:true,json:async()=>({answers:Object.fromEntries(Object.entries(q).map(([id,v])=>{
            const selected=id==="route"?choice:"unrelated";
            return [id,{type:"choice",choice:selected,confidence:.99,probabilities:Object.fromEntries(Object.keys(v.criteria).map(k=>[k,k===selected?1:0]))}];
        }))})};
    }};
    const event={session_id:"session",cwd:"/work",turn_id:"turn_1"};
    return {runner,routing,event,context,calls,requests:()=>requests};
}

test("hook_routes_before_model_and_suppresses_repeated_tool_context",async t=>{
    const f=await fixture(t);
    const start=await runHook({...f.event,hook_event_name:"SessionStart"},f.runner,f.routing);
    assert.ok(!start.hookSpecificOutput.additionalContext.includes("Unrelated Inbox body"));
    const prompt=await runHook({...f.event,hook_event_name:"UserPromptSubmit",prompt:"Create PR"},f.runner,f.routing);
    assert.match(prompt.hookSpecificOutput.additionalContext,/job followup/);
    assert.ok(!prompt.hookSpecificOutput.additionalContext.includes("Unrelated Inbox body"));
    assert.ok(prompt.hookSpecificOutput.additionalContext.length<1800);
    for(let i=0;i<3;i++)assert.deepEqual(await runHook({...f.event,hook_event_name:"PreToolUse"},f.runner,f.routing),{});
    assert.equal(f.requests(),1);
    f.context.inbox_cancellations=[{id:"inbox_cancelled",job_id:"job_a"}];
    const cancel=await runHook({...f.event,hook_event_name:"PostToolUse"},f.runner,f.routing);
    assert.match(cancel.hookSpecificOutput.additionalContext,/cancelled/);
});

test("uncertain_prompt_is_delegated_once_and_next_turn_is_not_suppressed",async t=>{
    const f=await fixture(t,"uncertain");
    const prompt=await runHook({...f.event,hook_event_name:"UserPromptSubmit",prompt:"Continue"},f.runner,f.routing);
    assert.match(prompt.hookSpecificOutput.additionalContext,/Agent/);
    assert.doesNotMatch(prompt.hookSpecificOutput.additionalContext,/Unrelated Inbox body/);
    assert.match(prompt.hookSpecificOutput.additionalContext,/reasoning_effort=low/);
    assert.deepEqual(await runHook({...f.event,hook_event_name:"PreToolUse"},f.runner,f.routing),{});
    const next=await runHook({...f.event,turn_id:"turn_2",hook_event_name:"PreToolUse"},f.runner,f.routing);
    assert.match(next.hookSpecificOutput.additionalContext,/Resolve whether/);
});

test("disabled_prompt_hook_does_not_change_existing_injection",async t=>{
    const f=await fixture(t);
    f.routing.env={};
    assert.deepEqual(await runHook({...f.event,hook_event_name:"UserPromptSubmit",prompt:"Create PR"},f.runner,f.routing),{});
    const tool=await runHook({...f.event,hook_event_name:"PreToolUse"},f.runner,f.routing);
    assert.match(tool.hookSpecificOutput.additionalContext,/Unrelated Inbox body/);
    assert.equal(f.requests(),0);
});

for(const host of ["pi","omp"])test(`${host}_classifies_before_agent_start`,async t=>{
    const f=await fixture(t,"new_job"),handlers=new Map();
    registerExtension({on:(n,h)=>handlers.set(n,h),registerTool(){}},host,f.runner,globalThis,undefined,f.routing);
    const result=await handlers.get("before_agent_start")({prompt:"Implement search"},{cwd:"/work",sessionManager:{getSessionId:()=>"session"}});
    assert.match(result.message.content,/new_job/);
    assert.ok(!result.message.content.includes("Unrelated Inbox body"));
    assert.equal(f.requests(),1);
});

test("shared_manifest_registers_prompt_routing_and_package_includes_router",async()=>{
    const hooks=JSON.parse(await readFile(new URL("../hooks/hooks.json",import.meta.url),"utf8"));
    assert.equal(hooks.hooks.UserPromptSubmit[0].hooks[0].timeout,30);
    const pkg=JSON.parse(await readFile(new URL("../package.json",import.meta.url),"utf8"));
    assert.ok(pkg.files.includes("jev.mjs"));
    assert.ok(pkg.files.includes("routing-state.mjs"));
});

for (const eventName of ["Stop", "Interrupt", "SessionEnd"])test(`${eventName}_clears_prompt_receipt`,async t=>{
    const f=await fixture(t);
    await runHook({...f.event,hook_event_name:"UserPromptSubmit",prompt:"Create PR"},f.runner,f.routing);
    await runHook({...f.event,hook_event_name:eventName},f.runner,f.routing);
    const result=await runHook({...f.event,hook_event_name:"PreToolUse"},f.runner,f.routing);
    assert.match(result.hookSpecificOutput.additionalContext,/Resolve whether/);
});

test("claude_receipt_without_turn_id_does_not_leak_into_another_session",async t=>{
    const f=await fixture(t);
    delete f.event.turn_id;
    await runHook({...f.event,hook_event_name:"UserPromptSubmit",prompt:"Create PR"},f.runner,f.routing);
    assert.deepEqual(await runHook({...f.event,hook_event_name:"PreToolUse"},f.runner,f.routing),{});
    const another=await runHook({...f.event,session_id:"another",hook_event_name:"PreToolUse"},f.runner,f.routing);
    assert.match(another.hookSpecificOutput.additionalContext,/Resolve whether/);
});

test("unwritable_receipt_retains_legacy_tool_fallback",async t=>{
    const f=await fixture(t);
    f.routing.cacheDir=new URL("../package.json",import.meta.url).pathname;
    await runHook({...f.event,hook_event_name:"UserPromptSubmit",prompt:"Create PR"},f.runner,f.routing);
    const result=await runHook({...f.event,hook_event_name:"PreToolUse"},f.runner,f.routing);
    assert.match(result.hookSpecificOutput.additionalContext,/Resolve whether/);
});

for (const configuration of [{}, { TASKIX_JEV_ENABLED: "true" }, { ...env, TASKIX_JEV_API_KEY: "" }]) {
    test(`disabled_prompt_has_zero_cli_calls_${JSON.stringify(configuration)}`, async t => {
        const f = await fixture(t);
        f.routing.env = configuration;
        await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, f.runner, f.routing);
        assert.deepEqual(f.calls, []);
    });
}

for (const hook of ["PreToolUse", "PostToolUse"]) test(`${hook}_routed_turn_uses_only_heartbeat_and_keeps_cancellation`, async t => {
    const f = await fixture(t);
    await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Create PR" }, f.runner, f.routing);
    f.calls.length = 0;
    f.context.inbox_cancellations = [{ id: "cancelled_entry", job_id: "job_a" }];
    const result = await runHook({ ...f.event, hook_event_name: hook }, f.runner, f.routing);
    assert.deepEqual(f.calls, [["hook", "heartbeat"]]);
    assert.match(result.hookSpecificOutput.additionalContext, /cancelled_entry/);
});

test("older_heartbeat_without_cancellations_retains_live_context_check", async t => {
    const f = await fixture(t);
    await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Create PR" }, f.runner, f.routing);
    f.calls.length = 0;
    f.context.inbox_cancellations = [{ id: "legacy_cancel", job_id: "job_a" }];
    const runner = async args => {
        const result = await f.runner(args);
        return args[0] === "hook" ? { result: {} } : result;
    };
    const result = await runHook({ ...f.event, hook_event_name: "PreToolUse" }, runner, f.routing);
    assert.deepEqual(f.calls, [["hook", "heartbeat"], ["context"]]);
    assert.match(result.hookSpecificOutput.additionalContext, /legacy_cancel/);
});

test("agent_fallback_has_a_total_budget_and_preserves_assignment_references", async t => {
    const f = await fixture(t, "uncertain");
    f.context.job_id = "job_a";
    f.context.task_id = "task_owned";
    f.context.task = { id: "task_owned", reason: "界".repeat(100000) };
    f.context.previous_job = { ...job, prompt: "x".repeat(100000) };
    f.context.inbox_todos = Array.from({ length: 32 }, (_, i) => ({ id: `inbox_${i}`, content: "界".repeat(10000) }));
    const output = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, f.runner, f.routing);
    const content = output.hookSpecificOutput.additionalContext;
    assert.ok(content.length < 1800, `fallback injected ${content.length} characters`);
    assert.doesNotMatch(content, /inbox_31|task_owned/);
    const path = JSON.parse(content.split("Snapshot: ")[1]);
    const snapshot = JSON.parse(await readFile(path, "utf8"));
    assert.equal(snapshot.assignment.task_id, "task_owned");
    assert.equal(snapshot.assignment.job_id, "job_a");
    assert.equal(snapshot.inbox.length, 32);
    assert.ok(snapshot.expires > Date.now());
});

test("oversized_prompt_history_defers_without_a_jev_request", async t => {
    const f = await fixture(t);
    const path = join(f.routing.cacheDir, "transcript.jsonl");
    await writeFile(path, JSON.stringify({ type: "user", uuid: "u", message: { role: "user", content: "x".repeat(300000) } }));
    const result = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue", transcript_path: path }, f.runner, f.routing);
    assert.equal(f.requests(), 0);
    assert.match(result.hookSpecificOutput.additionalContext, /history_unavailable/);
    assert.ok(result.hookSpecificOutput.additionalContext.length <= 12000);
});

for (const stalled of ["snapshot"]) test(`prompt_deadline_covers_${stalled}_and_defers`, async t => {
    const f = await fixture(t);
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const entered = Promise.withResolvers();
    const runner = async (args, options) => {
        if (args.includes(stalled)) {
            entered.resolve(options.signal);
            if (!options.signal) throw new Error("Missing shared deadline");
            return new Promise((_, reject) => options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true }));
        }
        return f.runner(args);
    };
    const pending = runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, runner, f.routing);
    const settled = pending.then(value => ({ value }), error => ({ error }));
    const signal = await entered.promise;
    t.mock.timers.tick(8000);
    const result = await settled;
    assert.ok(signal instanceof AbortSignal, "Preparation must receive the shared signal");
    assert.equal(signal.aborted, true);
    assert.equal(result.error, undefined);
    assert.match(result.value.hookSpecificOutput.additionalContext, /Agent/);
    assert.equal(f.requests(), 0);
});

test("prompt_preparation_and_jev_share_one_signal", async t => {
    const f = await fixture(t);
    const signals = [];
    const runner = async (args, options) => { signals.push(options.signal); return f.runner(args); };
    const fetch = f.routing.fetch;
    f.routing.fetch = async (url, init) => { signals.push(init.signal); return fetch(url, init); };
    await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Create PR" }, runner, f.routing);
    assert.ok(signals.length >= 3);
    assert.ok(signals.every(signal => signal instanceof AbortSignal && signal === signals[0]));
});

for (const host of ["pi", "omp"]) test(`${host}_snapshot_preparation_is_in_routing_deadline`, async t => {
    const f = await fixture(t), handlers = new Map();
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const entered = Promise.withResolvers();
    const runner = async (args, options) => {
        if (args[1] === "snapshot") {
            entered.resolve(options.signal);
            if (!options.signal) throw new Error("Missing shared deadline");
            return new Promise((_, reject) => options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true }));
        }
        return f.runner(args);
    };
    registerExtension({ on: (n, h) => handlers.set(n, h), registerTool() {} }, host, runner, globalThis, undefined, f.routing);
    const pending = handlers.get("before_agent_start")({ prompt: "Continue" }, { cwd: "/work", sessionManager: { getSessionId: () => "session" } });
    const settled = pending.then(value => ({ value }), error => ({ error }));
    const signal = await entered.promise;
    t.mock.timers.tick(8000);
    const result = await settled;
    assert.ok(signal instanceof AbortSignal);
    assert.equal(result.error, undefined);
    assert.match(result.value.message.content, /Agent/);
});

test("jev_uses_only_time_remaining_after_preparation", async t => {
    const f = await fixture(t);
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const entered = Promise.withResolvers();
    const runner = async (args, options) => {
        if (args[1] === "snapshot") t.mock.timers.tick(6000);
        return f.runner(args, options);
    };
    f.routing.fetch = async (_url, init) => {
        entered.resolve(init.signal);
        return new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason), { once: true }));
    };
    const pending = runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, runner, f.routing);
    const signal = await entered.promise;
    t.mock.timers.tick(1999);
    assert.equal(signal.aborted, false);
    t.mock.timers.tick(1);
    assert.match((await pending).hookSpecificOutput.additionalContext, /Agent/);
    assert.equal(signal.aborted, true);
});

test("late_snapshot_result_cannot_restart_classification_after_timeout", async t => {
    const f = await fixture(t);
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const entered = Promise.withResolvers(), delayed = Promise.withResolvers();
    const runner = async args => {
        if (args[1] === "snapshot") { entered.resolve(); return delayed.promise; }
        return f.runner(args);
    };
    const pending = runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, runner, f.routing);
    await entered.promise;
    t.mock.timers.tick(8000);
    assert.match((await pending).hookSpecificOutput.additionalContext, /preparation_unavailable/);
    delayed.resolve({ result: f.context });
    await new Promise(setImmediate);
    assert.equal(f.requests(), 0);
    assert.ok(!f.calls.some(args => args[0] === "routing"));
});

test("prompt_uses_one_snapshot_and_one_revision_process", async t => {
    const f = await fixture(t);
    const calls = [];
    const runner = async args => {
        calls.push(args);
        if (args[0] === "routing" && args[1] === "snapshot") return { result: { ...f.context, routing: { complete: true, candidates: [{ job, tasks: [] }] } } };
        if (args[0] === "routing" && args[1] === "revision") return { result: job };
        throw new Error("Separate heartbeat/context/candidate processes are forbidden");
    };
    const result = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Create PR" }, runner, f.routing);
    assert.match(result.hookSpecificOutput.additionalContext, /Taskix route: followup/);
    assert.deepEqual(calls, [["routing", "snapshot"], ["routing", "revision", "job_a"]]);
});

test("older_cli_without_snapshot_defers_and_retains_legacy_tool_context", async t => {
    const f = await fixture(t);
    const runner = async args => {
        if (args[0] === "routing" && args[1] === "snapshot") throw new Error("unrecognized subcommand snapshot");
        return f.runner(args);
    };
    const prompt = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, runner, f.routing);
    assert.match(prompt.hookSpecificOutput.additionalContext, /Agent/);
    assert.match(prompt.hookSpecificOutput.additionalContext, /taskix context/);
    assert.equal(f.requests(), 0);
    const tool = await runHook({ ...f.event, hook_event_name: "PreToolUse" }, runner, f.routing);
    assert.match(tool.hookSpecificOutput.additionalContext, /Unrelated Inbox body/);
    assert.deepEqual(f.calls, [["hook", "heartbeat"], ["context"]]);
});

 test("classifier_child_skips_routing_and_tracking_without_affecting_parent", async t => {
    const f = await fixture(t, "uncertain");
    const output = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, f.runner, f.routing);
    const path = JSON.parse(output.hookSpecificOutput.additionalContext.split("Snapshot: ")[1]);
    f.calls.length = 0;
    const child = { ...f.event, session_id: "child" };
    assert.deepEqual(await runHook({ ...child, hook_event_name: "UserPromptSubmit", prompt: `TASKIX_ROUTING_CLASSIFIER ${JSON.stringify(path)}` }, f.runner, f.routing), {});
    for (const hook of ["PreToolUse", "PostToolUse", "Stop", "SessionEnd"]) {
        assert.deepEqual(await runHook({ ...child, hook_event_name: hook }, f.runner, f.routing), {});
    }
    assert.deepEqual(f.calls, []);
    await runHook({ ...f.event, hook_event_name: "PreToolUse" }, f.runner, f.routing);
    assert.deepEqual(f.calls, [["hook", "heartbeat"]]);
});
 test("snapshot_storage_failure_keeps_bounded_main_agent_fallback", async t => {
    const f = await fixture(t, "uncertain");
    f.routing.cacheDir = new URL("../package.json", import.meta.url).pathname;
    const output = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, f.runner, f.routing);
    assert.match(output.hookSpecificOutput.additionalContext, /bounded summaries/);
    assert.doesNotMatch(output.hookSpecificOutput.additionalContext, /Snapshot:/);
});

test("delegation_snapshots_are_private_immutable_and_exclude_credentials", async t => {
    const f = await fixture(t, "uncertain");
    f.context.lease = { token: "secret_lease" };
    const first = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "First" }, f.runner, f.routing);
    const second = await runHook({ ...f.event, turn_id: "two", hook_event_name: "UserPromptSubmit", prompt: "Second" }, f.runner, f.routing);
    const path = value => JSON.parse(value.hookSpecificOutput.additionalContext.split("Snapshot: ")[1]);
    assert.notEqual(path(first), path(second));
    const data = await readFile(path(first), "utf8");
    assert.equal(JSON.parse(data).prompt, "First");
    assert.doesNotMatch(data, /secret_lease|API_KEY/);
    const { stat } = await import("node:fs/promises");
    // Windows mode bits do not represent POSIX owner-only permissions.
    if (process.platform !== "win32") assert.equal((await stat(path(first))).mode & 0o777, 0o600);
    const expired = JSON.parse(data); expired.expires = 0;
    await writeFile(path(first), JSON.stringify(expired));
    f.calls.length = 0;
    await runHook({ ...f.event, session_id: "child", hook_event_name: "UserPromptSubmit", prompt: `TASKIX_ROUTING_CLASSIFIER ${JSON.stringify(path(first))}` }, f.runner, f.routing);
    assert.ok(f.calls.length > 0, "expired marker must not suppress tracking");
});

for (const invalid of ["missing_expiry", "parent_session", "different_cwd"]) test(`classifier_marker_rejects_${invalid}`, async t => {
    const f = await fixture(t, "uncertain");
    const output = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, f.runner, f.routing);
    const path = JSON.parse(output.hookSpecificOutput.additionalContext.split("Snapshot: ")[1]);
    if (invalid === "missing_expiry") {
        const packet = JSON.parse(await readFile(path, "utf8"));
        delete packet.expires;
        await writeFile(path, JSON.stringify(packet));
    }
    f.calls.length = 0;
    await runHook({ ...f.event, session_id: invalid === "parent_session" ? f.event.session_id : "child", cwd: invalid === "different_cwd" ? "/other" : f.event.cwd, hook_event_name: "UserPromptSubmit", prompt: `TASKIX_ROUTING_CLASSIFIER ${JSON.stringify(path)}` }, f.runner, f.routing);
    assert.ok(f.calls.length > 0);
});

// The revision checked by the router must survive into the actual write command.
test("followup_instruction_binds_the_observed_revision", async t => {
    const f = await fixture(t);
    const result = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Create PR" }, f.runner, f.routing);
    assert.match(result.hookSpecificOutput.additionalContext, /job followup job_a --expect-revision 1/);
});

test("classifier_guard_accepts_canonical_cwd_alias", async t => {
    const f = await fixture(t, "uncertain");
    const { realpath } = await import("node:fs/promises");
    f.event.cwd = f.routing.cacheDir;
    const result = await runHook({ ...f.event, hook_event_name: "UserPromptSubmit", prompt: "Continue" }, f.runner, f.routing);
    const path = JSON.parse(result.hookSpecificOutput.additionalContext.split("Snapshot: ")[1]);
    const before = f.calls.length;
    await runHook({ ...f.event, session_id: "child", cwd: await realpath(f.event.cwd), hook_event_name: "UserPromptSubmit", prompt: `TASKIX_ROUTING_CLASSIFIER ${JSON.stringify(path)}` }, f.runner, f.routing);
    assert.equal(f.calls.length, before);
});
