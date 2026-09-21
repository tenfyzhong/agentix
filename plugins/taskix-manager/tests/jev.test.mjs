import assert from "node:assert/strict";
import { test } from "node:test";
import { routePrompt, jevConfig } from "../jev.mjs";

const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://example.test/v1/systemone", TASKIX_JEV_API_KEY: "test-key" };
const job = { id: "job_a", project_id: "p", status: "PENDING_REVIEW", revision: 3, prompt: "Fix login", title: "Login", conversation: [{ role: "assistant", text: "Ready to deliver" }] };
const context = { project_id: "p", previous_job: job, inbox_todos: [] };
const options = { session: "s", cwd: "/work" };
function fixture(jobs = [job], tasks = []) {
    const calls = [];
    const runner = async (args) => {
        calls.push(args);
        if (args[0] === "routing" && args[1] === "candidates") return { result: { complete: true,
            candidates: jobs.map(job => ({ job, tasks: tasks.filter(t => t.job_id === job.id) })) } };
        if (args[0] === "routing" && args[1] === "revision") return { result: jobs.find(j => j.id === args[2]) };
        throw new Error(`Unexpected mutation: ${args}`);
    };
    return { runner, calls };
}
function response(request, selected = "followup:job_a", overrides = {}) {
    return { answers: Object.fromEntries(Object.entries(request.questions).map(([id, q]) => {
        const choice = id === "route" ? selected : "unrelated";
        return [id, { type: "choice", choice, confidence: .99, probabilities: Object.fromEntries(Object.keys(q.criteria).map(key => [key, key === choice ? 1 : 0])), ...(id === "route" ? overrides : {}) }];
    })) };
}
const fetchFor = (selected, overrides) => async (_url, init) => ({ ok: true, json: async () => response(JSON.parse(init.body), selected, overrides) });

for (const patch of [{TASKIX_JEV_ENABLED:"false"}, {TASKIX_JEV_ENABLED:""}, {TASKIX_JEV_URL:""}, {TASKIX_JEV_API_KEY:" "}]) {
    test(`disabled_or_incomplete_configuration_preserves_legacy_${JSON.stringify(patch)}`, async () => {
        const actual = {...env, ...patch};
        assert.equal(jevConfig(actual), undefined);
        const result = await routePrompt({prompt:"create PR", context, options, env:actual, runner:()=>assert.fail("no task query"), fetch:()=>assert.fail("no network")});
        assert.equal(result, undefined);
    });
}

test("confident_followup_has_only_selected_context_and_no_mutations", async () => {
    const f = fixture([job, {...job,id:"job_other",prompt:"Unrelated secret background"}]);
    let request;
    const result = await routePrompt({prompt:"Create PR",context,options,env,runner:f.runner,fetch:async(url,init)=>{
        assert.equal(url,env.TASKIX_JEV_URL);
        assert.equal(init.headers.Authorization,"Bearer test-key");
        assert.equal(init.redirect,"error");
        request=JSON.parse(init.body);
        return {ok:true,json:async()=>response(request)};
    }});
    assert.equal(result.decision.action,"followup");
    assert.equal(result.context.job.id,"job_a");
    assert.equal(result.context.previous_job,undefined);
    assert.ok(!JSON.stringify(result).includes("Unrelated secret background"));
    assert.ok(!JSON.stringify(result).includes("test-key"));
    assert.equal(request.model,"jev-latest");
    assert.ok(f.calls.every(c=>["candidates","revision"].includes(c[1])));
});

for (const [name, selected, overrides] of [
    ["low_confidence","followup:job_a",{confidence:.3}],
    ["insufficient_information","uncertain",{}],
    ["conflicting_candidates","followup:job_a",{probabilities:{"followup:job_a":.5,"new_job":.5,"discussion":0,"uncertain":0}}],
    ["unknown_choice","job_invented",{}],
    ["missing_distribution","followup:job_a",{probabilities:undefined}],
    ["invalid_confidence","followup:job_a",{confidence:2}],
]) test(`${name}_returns_agent_fallback`,async()=>{
    const result=await routePrompt({prompt:"Continue",context,options,env,runner:fixture().runner,fetch:fetchFor(selected,overrides)});
    assert.equal(result.decision.action,"agent");
});

test("waiting_task_is_discovered_without_previous_job",async()=>{
    const waiting={id:"task_wait",job_id:"job_a",status:"WAITING_USER",reason:"Which region?"};
    const active={...job,status:"ACTIVE"};
    const result=await routePrompt({prompt:"Asia",context:{...context,previous_job:null},options,env,runner:fixture([active],[waiting]).runner,fetch:fetchFor("resume:job_a")});
    assert.equal(result.decision.action,"resume");
    assert.equal(result.context.tasks[0].reason,"Which region?");
});

test("transport_failure_is_redacted_and_falls_back",async()=>{
    const result=await routePrompt({prompt:"Continue",context,options,env,runner:fixture().runner,fetch:async()=>{throw new Error("test-key upstream failure");}});
    assert.equal(result.decision.action,"agent");
    assert.ok(!JSON.stringify(result).includes("test-key"));
});

test("changed_candidate_revision_falls_back_before_delivery",async()=>{
    const f=fixture();
    const runner=async args=>args[1]==="revision"?{result:{...job,status:"COMPLETED",revision:4}}:f.runner(args);
    const result=await routePrompt({prompt:"Create PR",context,options,env,runner,fetch:fetchFor("followup:job_a")});
    assert.equal(result.decision.action,"agent");
});

test("inbox_ambiguity_falls_back_instead_of_silently_dropping_entries",async()=>{
    const ctx={...context,inbox_todos:[{id:"inbox_a",content:"Fix login"}]};
    const result=await routePrompt({prompt:"Fix login",context:ctx,options,env,runner:fixture().runner,fetch:async(_url,init)=>{
        const data=response(JSON.parse(init.body),"new_job");
        data.answers.inbox_0.confidence=.2;
        return {ok:true,json:async()=>data};
    }});
    assert.equal(result.decision.action,"agent");
});

for (const [name, patch] of [["invalid_url",{TASKIX_JEV_URL:"file:///private/key"}], ["credential_url",{TASKIX_JEV_URL:"https://user:key@example.test"}], ["invalid_threshold",{TASKIX_JEV_MIN_CONFIDENCE:"NaN"}]]) {
    test(name,()=>assert.equal(jevConfig({...env,...patch}),undefined));
}

test("custom_model_and_threshold_are_applied",async()=>{
    const result=await routePrompt({prompt:"Create PR",context,options,env:{...env,TASKIX_JEV_MODEL:"jev-pinned",TASKIX_JEV_MIN_CONFIDENCE:"0.95"},runner:fixture().runner,fetch:async(_url,init)=>{
        const req=JSON.parse(init.body);
        assert.equal(req.model,"jev-pinned");
        return {ok:true,json:async()=>response(req,"followup:job_a",{confidence:.94})};
    }});
    assert.equal(result.decision.action,"agent");
});

test("foreign_archived_and_completed_jobs_are_never_candidates",async()=>{
    const jobs=[job,{...job,id:"foreign",project_id:"other"},{...job,id:"archived",archived_at:1},{...job,id:"completed",status:"COMPLETED"}];
    const result=await routePrompt({prompt:"Create PR",context,options,env,runner:fixture(jobs).runner,fetch:async(_url,init)=>{
        const req=JSON.parse(init.body);
        assert.deepEqual(req.state.candidates.map(c=>c.job.id),[job.id]);
        return {ok:true,json:async()=>response(req)};
    }});
    assert.equal(result.decision.action,"followup");
});

test("current_assignment_is_not_redirected_to_another_job",async()=>{
    const result=await routePrompt({prompt:"Another task",context:{...context,job_id:"job_b"},options,env,runner:fixture([job,{...job,id:"job_b",status:"ACTIVE"}]).runner,fetch:fetchFor("followup:job_a")});
    assert.equal(result.decision.action,"agent");
});

test("all_semantic_inbox_matches_are_preserved_as_ids",async()=>{
    const ctx={...context,inbox_todos:[{id:"inbox_a",content:"Login"},{id:"inbox_b",content:"Regression tests"}]};
    const result=await routePrompt({prompt:"Fix login with tests",context:ctx,options,env,runner:fixture().runner,fetch:async(_url,init)=>{
        const req=JSON.parse(init.body),data=response(req,"new_job");
        for(const id of ["inbox_0","inbox_1"])data.answers[id]={type:"choice",choice:"match",confidence:1,probabilities:{match:1,unrelated:0,uncertain:0}};
        return {ok:true,json:async()=>data};
    }});
    assert.deepEqual(result.decision.inbox_ids,["inbox_a","inbox_b"]);
    assert.ok(!JSON.stringify(result.context).includes("Regression tests"));
});

for (const [name, fetch] of [
    ["http_error",async()=>({ok:false,json:()=>assert.fail("must not parse error body")})],
    ["malformed_json",async()=>({ok:true,json:async()=>{throw new Error("private upstream body");}})],
])test(`${name}_falls_back`,async()=>{
    const result=await routePrompt({prompt:"Create PR",context,options,env,runner:fixture().runner,fetch});
    assert.equal(result.decision.action,"agent");
});

test("request_timeout_aborts_and_falls_back",async t=>{
    t.mock.timers.enable({apis:["setTimeout"]});
    const entered=Promise.withResolvers();
    const pending=routePrompt({prompt:"Create PR",context,options,env,runner:fixture().runner,fetch:async(_url,init)=>{
        entered.resolve();
        return new Promise((_,reject)=>init.signal.addEventListener("abort",()=>reject(new Error("timeout")),{once:true}));
    }});
    await entered.promise;
    t.mock.timers.tick(8000);
    assert.equal((await pending).decision.action,"agent");
});

test("large_selected_history_is_bounded_for_the_coding_agent",async()=>{
    const large={...job,conversation:[{role:"assistant",text:"x".repeat(40000)}]};
    const result=await routePrompt({prompt:"Create PR",context,options,env,runner:fixture([large]).runner,fetch:fetchFor("followup:job_a")});
    assert.equal(result.decision.action,"followup");
    assert.ok(JSON.stringify(result.context).length<6000);
    assert.match(result.context.job.conversation[0].text,/truncated/);
});

for (const count of [1, 8, 32]) test(`candidate_lookup_uses_two_cli_calls_for_${count}_jobs`, async () => {
    const jobs = Array.from({ length: count }, (_, i) => ({ ...job, id: i ? `job_${i}` : job.id }));
    const calls = [];
    const runner = async args => {
        calls.push(args);
        if (args[0] !== "routing") throw new Error("Per-job reads are forbidden");
        if (args[1] === "candidates") return { result: { complete: true, candidates: jobs.map(job => ({ job, tasks: [] })) } };
        if (args[1] === "revision") return { result: job };
        throw new Error("Unexpected command");
    };
    const result = await routePrompt({ prompt: "Create PR", context, options, env, runner, fetch: fetchFor("followup:job_a") });
    assert.equal(result.decision.action, "followup");
    assert.deepEqual(calls, [["routing", "candidates", "p"], ["routing", "revision", "job_a"]]);
});

test("incomplete_snapshot_defers_without_contacting_jev", async () => {
    const result = await routePrompt({ prompt: "Continue", context, options, env,
        runner: async () => ({ result: { complete: false, candidates: [{ job, tasks: [] }] } }),
        fetch: () => assert.fail("Incomplete evidence must not be classified"),
    });
    assert.equal(result.decision.action, "agent");
    assert.equal(result.decision.reason, "incomplete_snapshot");
});

for (const text of ["界".repeat(9000), "😀".repeat(7000), "x".repeat(24000)]) {
    test(`complete_request_budget_rejects_${Buffer.byteLength(text)}_bytes`, async () => {
        let requests = 0;
        const result = await routePrompt({ prompt: text, context, options, env, runner: fixture().runner,
            fetch: async () => { requests++; return { ok: false }; } });
        assert.equal(requests, 0, "budget must cover UTF-8 and questions before HTTP");
        assert.equal(result.decision.reason, "context_too_large");
    });
}

test("compact_history_removes_duplicates_and_metadata_and_bounds_old_io", async () => {
    const messages = [
        { role: "user", text: "Old input" }, { role: "assistant", text: "Old output" },
        { role: "user", text: "界".repeat(1000) }, { role: "assistant", text: "😀".repeat(1000) },
        { role: "user", text: "Continue" },
    ];
    let request;
    const tasks = [{ id: "done", job_id: job.id, status: "DONE", title: "Completed detail" },
        { id: "waiting", job_id: job.id, status: "WAITING_USER", reason: "Which region?" }];
    const result = await routePrompt({ prompt: "Continue", context, options, env, history: messages,
        runner: fixture([{ ...job, conversation: messages, session_id: "private-session" }], tasks).runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request) }; } });
    assert.equal(result.decision.action, "followup");
    assert.equal(request.state.cwd, undefined);
    assert.equal(request.state.session, undefined);
    assert.equal(request.state.candidates[0].job.revision, undefined);
    assert.equal(request.state.candidates[0].job.session_id, undefined);
    assert.equal(request.state.candidates[0].tasks.length, 1);
    assert.equal(request.state.candidates[0].tasks[0].reason, "Which region?");
    assert.equal(request.state.recent_conversation.length, 2);
    assert.deepEqual(request.state.candidates[0].job.conversation, []);
    for (const message of request.state.recent_conversation) {
        assert.ok(Buffer.byteLength(message.text) <= (message.role === "user" ? 512 : 256));
        assert.ok(!message.text.includes("\ufffd"));
        assert.match(message.text, /\[excerpt\]/);
    }
    assert.ok(!JSON.stringify(request.state).includes("Old output"));
    assert.ok(Buffer.byteLength(JSON.stringify(request)) < 5000);
});

test("request_budget_includes_questions_and_accepts_exact_boundary", async () => {
    let body;
    const run = (prompt, ctx = context) => routePrompt({ prompt, context: ctx, options, env, runner: fixture([]).runner,
        fetch: async (_url, init) => { body = init.body; return { ok: true, json: async () => response(JSON.parse(body), "new_job") }; } });
    await run("x");
    const overhead = Buffer.byteLength(body) - 1;
    const prompt = "x".repeat(24000 - overhead);
    assert.equal((await run(prompt)).decision.action, "new_job");
    assert.equal(Buffer.byteLength(body), 24000);
    assert.equal(JSON.parse(body).state.prompt, prompt, "current prompt must stay verbatim");
    body = undefined;
    assert.equal((await run(prompt + "x")).decision.reason, "context_too_large");
    assert.equal(body, undefined);
    const inbox_todos = Array.from({ length: 32 }, (_, i) => ({ id: `inbox_${i}`, content: "small entry" }));
    assert.equal((await run("x".repeat(18000), { ...context, inbox_todos })).decision.reason, "context_too_large");
    assert.equal(body, undefined, "question overhead must not bypass the budget");
});

for (const revision of [undefined, -1, 1.5]) test(`missing_or_invalid_candidate_revision_defers_${revision}`, async () => {
    const invalid = { ...job, revision };
    const result = await routePrompt({ prompt: "Create PR", context, options, env, runner: fixture([invalid]).runner, fetch: fetchFor("followup:job_a") });
    assert.equal(result.decision.action, "agent");
});
