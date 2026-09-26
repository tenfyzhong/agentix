import assert from "node:assert/strict";
import { test } from "node:test";
import { routePrompt, jevConfig } from "../jev.mjs";

const env = { TASKIX_JEV_ENABLED: "true", TASKIX_JEV_URL: "https://example.test/v1/systemone", TASKIX_JEV_API_KEY: "test-key" };
test("jev_config_defaults_to_065_and_preserves_explicit_thresholds", () => {
    for (const value of [undefined, "", "   "]) {
        assert.equal(jevConfig({ ...env, TASKIX_JEV_MIN_CONFIDENCE: value }).threshold, 0.65);
    }
    for (const value of ["0.5", "0.65", " 0.9 ", "1"]) {
        assert.equal(jevConfig({ ...env, TASKIX_JEV_MIN_CONFIDENCE: value }).threshold, Number(value));
    }
});

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
        const discussion = selected === "discussion" || selected.startsWith("discussion:");
        const owner = discussion ? selected.includes(":") ? Object.keys(request.questions.route.criteria).find(k => k.endsWith(":" + selected.split(":")[1])) : "new_job" : selected;
        const choice = id === "review_policy" ? "required" : id === "intent" ? discussion ? "question" : "work" : id === "route" ? owner : "unrelated";
        return [id, { type: "choice", choice, confidence: .99, probabilities: Object.fromEntries(Object.keys(q.criteria).map(key => [key, key === choice ? 1 : 0])), ...(id === "route" ? overrides : {}) }];
    })) };
}
const fetchFor = (selected, overrides) => async (_url, init) => ({ ok: true, json: async () => response(JSON.parse(init.body), selected, overrides) });

test("candidate_dialogue_retains_latest_user_requirement_before_progress_updates", async () => {
    const requirement = { role: "user", text: "Also support reconnecting the proxy socket" };
    const updates = Array.from({ length: 5 }, (_, i) => ({ role: "assistant", text: `Progress ${i}` }));
    let request;
    await routePrompt({ prompt: "Review this change", context, options, env,
        runner: fixture([{ ...job, conversation: [requirement, ...updates] }]).runner,
        fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request) };
        } });
    assert.deepEqual(request.state.candidates[0].job.conversation, [requirement, ...updates.slice(-2)]);
});

test("complete_empty_candidate_set_is_explicit_even_when_history_describes_work", async () => {
    let request;
    const result = await routePrompt({ prompt: "Implement the recommendation", context: { project_id: "p" },
        options, env, runner: fixture([]).runner,
        history: [{ role: "assistant", text: "The investigation is complete. Fix the timeout by bounding reads." }],
        fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request, "new_job") };
        } });
    assert.deepEqual(request.state.candidate_scope, {
        complete: true, project_id: "p", eligible_statuses: ["ACTIVE", "PENDING_REVIEW"], count: 0,
    });
    assert.equal(request.state.dialogue_focus.previous_assistant_message.text,
        "The investigation is complete. Fix the timeout by bounding reads.");
    assert.equal(result.decision.action, "new_job");
});

test("pending_delivery_does_not_receive_empty_candidate_guidance", async () => {
    let request;
    const result = await routePrompt({ prompt: "Create PR", context, options, env,
        runner: fixture().runner, fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request) };
        } });
    assert.equal(request.state.candidate_scope, undefined);
    assert.doesNotMatch(request.questions.route.criteria.new_job, /zero candidates|untracked discussion/i);
    assert.doesNotMatch(request.questions.route.criteria.uncertain, /empty complete candidate/i);
    assert.equal(result.decision.action, "followup");
    assert.equal(result.decision.job_id, job.id);
});

test("completed_task_evidence_reaches_jev_from_sql_snapshot", async () => {
    const completed = { id: "task_done", title: "Delivered socket reconnect", status: "DONE" };
    let request;
    await routePrompt({ prompt: "Create PR", context, options, env,
        runner: fixture([{ ...job, completed_tasks: [completed] }]).runner,
        fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request) };
        } });
    assert.deepEqual(request.state.candidates[0].completed_tasks, [completed]);
    assert.deepEqual(request.questions.route.criteria["followup:job_a"].delivered_work, [completed.title]);
    assert.match(request.questions.route.criteria.uncertain, /spans multiple independent Jobs/);
    assert.deepEqual(request.state.candidates[0].tasks, []);
});

test("pending_delivery_retains_bounded_completed_task_evidence", async () => {
    const tasks = Array.from({ length: 12 }, (_, i) => ({ id: `task_${i}`, job_id: job.id,
        title: `Completed change ${i}`, status: "DONE" }));
    tasks.push({ id: "cancelled", job_id: job.id, title: "Abandoned", status: "CANCELLED" });
    let request;
    const result = await routePrompt({ prompt: "Create PR", context, options, env,
        runner: fixture([job], tasks).runner, fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request) };
        } });
    assert.deepEqual(request.state.candidates[0].completed_tasks.map(t => t.id), tasks.slice(4, 12).map(t => t.id));
    assert.ok(request.state.candidates[0].completed_tasks.every(t => t.status === "DONE"));
    assert.deepEqual(result.context.tasks, []);
});

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

for (const text of ["界".repeat(9000), "😀".repeat(7000), "x".repeat(30000)]) {
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
    assert.equal(request.state.recent_conversation.length, 4);
    assert.deepEqual(request.state.candidates[0].job.conversation, []);
    assert.deepEqual(request.state.candidates[0].job.recent_conversation_indices, [0, 1, 2, 3]);
    for (const message of request.state.recent_conversation.slice(-2)) {
        assert.ok(Buffer.byteLength(message.text) <= (message.role === "user" ? 1024 : 1536));
        assert.ok(!message.text.includes("\ufffd"));
        assert.match(message.text, /\[excerpt\]/);
    }
    assert.ok(JSON.stringify(request.state).includes("Old output"));
    assert.ok(Buffer.byteLength(JSON.stringify(request)) <= 30000);
    assert.ok(Buffer.byteLength(JSON.stringify(request.state.recent_conversation)) <= 8192);
});

test("request_budget_includes_questions_and_accepts_exact_boundary", async () => {
    let body;
    const run = (prompt, ctx = context) => routePrompt({ prompt, context: ctx, options, env, runner: fixture([]).runner,
        fetch: async (_url, init) => { body = init.body; return { ok: true, json: async () => response(JSON.parse(body), "new_job") }; } });
    await run("x");
    const overhead = Buffer.byteLength(body) - 3;
    const prompt = "x".repeat(Math.floor((30000 - overhead) / 3));
    assert.equal((await run(prompt)).decision.action, "new_job");
    assert.ok(Buffer.byteLength(body) <= 30000 && Buffer.byteLength(body) >= 29998);
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

for (const [prompt, choice] of [["这样修改", "followup:job_a"], ["有性能问题吗", "discussion:job_a"]]) {
    test(`anaphoric_prompt_keeps_multi_turn_referent_${choice}`, async () => {
        const history = [
            { role: "user", text: "优化登录 Job，减少数据库查询" },
            { role: "assistant", text: "方案是缓存登录查询结果" },
            { role: "user", text: "需要考虑缓存失效" },
            { role: "assistant", text: "写入时失效，保留回源" },
            { role: "user", text: "还有别的风险吗" },
            { role: "assistant", text: "需要验证并发更新" },
        ];
        const f = fixture();
        let request;
        const result = await routePrompt({ prompt, context, options, env, history, runner: f.runner,
            fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request, choice) }; } });
        assert.equal(result.decision.action, choice.split(":")[0]);
        assert.equal(result.decision.job_id, job.id);
        assert.deepEqual(request.state.recent_conversation, history);
        assert.equal(request.state.previous_job_id, job.id);
        assert.equal(result.context.job.id, job.id);
        assert.deepEqual(f.calls.map(c => c[1]), ["candidates", "revision"]);
    });
}

test("recent_decision_tail_and_utf8_budget_survive_long_assistant_answer", async () => {
    let request;
    const history = [{ role: "assistant", text: "方案背景：" + "界😀".repeat(2000) + "最终方案：写入后使登录缓存失效" }];
    await routePrompt({ prompt: "这样修改", context, options, env, history, runner: fixture().runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request) }; } });
    const text = request.state.recent_conversation[0].text;
    assert.ok(text.startsWith("方案背景："));
    assert.ok(text.endsWith("最终方案：写入后使登录缓存失效"));
    assert.ok(Buffer.byteLength(text) <= 1536);
    assert.ok(!text.includes("\ufffd"));
});

for (const status of ["ACTIVE", "PENDING_REVIEW"]) test(`job_discussion_preserves_lifecycle_${status}`, async () => {
    const selected = { ...job, status };
    const f = fixture([selected]);
    const result = await routePrompt({ prompt: "有性能问题吗", context, options, env, runner: f.runner, fetch: fetchFor("discussion:job_a") });
    assert.equal(result.decision.action, "discussion");
    assert.equal(result.decision.job_id, job.id);
    assert.equal(selected.status, status);
    assert.ok(f.calls.every(c => c[0] === "routing"));
});

test("job_discussion_does_not_redirect_current_assignment", async () => {
    const result = await routePrompt({ prompt: "有性能问题吗", context: { ...context, job_id: "job_b" }, options, env,
        runner: fixture([job, { ...job, id: "job_b", status: "ACTIVE" }]).runner, fetch: fetchFor("discussion:job_a") });
    assert.equal(result.decision.reason, "assignment_conflict");
});

test("job_discussion_rechecks_revision", async () => {
    const f = fixture();
    const result = await routePrompt({ prompt: "有性能问题吗", context, options, env,
        runner: args => args[1] === "revision" ? { result: { ...job, revision: 4 } } : f.runner(args), fetch: fetchFor("discussion:job_a") });
    assert.equal(result.decision.reason, "candidate_changed");
});

test("history_window_is_bounded_and_preserves_same_text_from_different_roles", async () => {
    const history = Array.from({ length: 100 }, (_, i) => ({ role: i % 2 ? "assistant" : "user", text: `message ${i}` }));
    history.push({ role: "user", text: "缓存失效" }, { role: "assistant", text: "缓存失效" });
    let request;
    await routePrompt({ prompt: "这样修改", context, options, env, history, runner: fixture().runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request) }; } });
    assert.deepEqual(request.state.recent_conversation, history.slice(-8));
});

test("escaped_history_cannot_exceed_serialized_budget_or_drop_candidates", async () => {
    const history = Array.from({ length: 20 }, (_, i) => ({ role: "assistant", text: `${i}` + "\u0000".repeat(10000) }));
    let request;
    const jobs = [job, { ...job, id: "job_b", prompt: "Unrelated work" }];
    await routePrompt({ prompt: "这样修改", context, options, env, history, runner: fixture(jobs).runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); assert.ok(Buffer.byteLength(init.body) <= 30000); return { ok: true, json: async () => response(request) }; } });
    assert.ok(Buffer.byteLength(JSON.stringify(request.state.recent_conversation)) <= 8192);
    assert.deepEqual(request.state.candidates.map(c => c.job.id), ["job_a", "job_b"]);
});

test("optional_history_yields_to_complete_candidate_set_at_request_budget", async () => {
    let body;
    const run = (history, prompt) => routePrompt({ prompt, context, options, env, history, runner: fixture().runner,
        fetch: async (_url, init) => { body = init.body; return { ok: true, json: async () => response(JSON.parse(body)) }; } });
    await run([], "x");
    const prompt = "x".repeat(Math.floor((30000 - Buffer.byteLength(body) + 3) / 3));
    const result = await run([{ role: "user", text: "Earlier context" }], prompt);
    assert.equal(result.decision.action, "followup");
    assert.ok(Buffer.byteLength(body) <= 30000 && Buffer.byteLength(body) >= 29998);
    assert.equal(JSON.parse(body).state.candidates.length, 1);
    assert.deepEqual(JSON.parse(body).state.recent_conversation, []);
});

test("history_trimming_retains_candidate_conversation_evidence", async () => {
    const anchor = { role: "user", text: "登录方案采用写入失效" };
    const history = [anchor, ...Array.from({ length: 7 }, (_, i) => ({ role: "assistant", text: `${i}` + "\u0000".repeat(10000) }))];
    let request;
    await routePrompt({ prompt: "这样修改", context, options, env, history,
        runner: fixture([{ ...job, conversation: [anchor] }]).runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request) }; } });
    assert.ok(!request.state.recent_conversation.some(m => m.text === anchor.text));
    assert.deepEqual(request.state.candidates[0].job.conversation, [anchor]);
});

test("deduplicated_advice_retains_candidate_source_references", async () => {
    const advice = { role: "assistant", text: "建议只读取轮次状态，跳过文本恢复" };
    let request;
    await routePrompt({ prompt: "按照建议进行修改", context, options, env, history: [advice],
        runner: fixture([{ ...job, conversation: [advice] }, { ...job, id: "job_b", title: "Task board", conversation: [] }]).runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request) }; } });
    assert.deepEqual(request.state.candidates[0].job.recent_conversation_indices, [0]);
    assert.deepEqual(request.state.candidates[0].job.conversation, []);
    assert.deepEqual(request.state.candidates[1].job.recent_conversation_indices, []);
    assert.match(request.questions.route.criteria['followup:job_a'].topic, /Login/);
    assert.match(request.questions.route.criteria['followup:job_b'].topic, /Task board/);
});

test("truncated_advice_keeps_all_matching_sources_without_forcing_ownership", async () => {
    const advice = { role: "assistant", text: "建议" + "界".repeat(1000) + "跳过文本恢复" };
    let request;
    const result = await routePrompt({ prompt: "按照建议进行修改", context, options, env, history: [advice],
        runner: fixture([{ ...job, conversation: [advice] }, { ...job, id: "job_b", conversation: [advice] }]).runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request, "uncertain") }; } });
    assert.deepEqual(request.state.candidates.map(c => c.job.recent_conversation_indices), [[0], [0]]);
    assert.equal(result.decision.action, "agent");
});

test("dialogue_focus_keeps_full_latest_advice_and_all_source_jobs", async () => {
    const question = { role: "user", text: "Review the login change" };
    const advice = { role: "assistant", text: "Context. " + "a".repeat(1700) + " Use a status-only RPC instead of reading message bodies. " + "b".repeat(1700) };
    let request;
    await routePrompt({ prompt: "Apply that recommendation", context, options, env, history: [question, advice],
        runner: fixture([{ ...job, conversation: [question, advice] }, { ...job, id: "job_b", title: "Other", conversation: [] }]).runner,
        fetch: async (_url, init) => { request = JSON.parse(init.body); return { ok: true, json: async () => response(request) }; } });
    assert.equal(request.state.dialogue_focus.previous_user_message.text, question.text);
    assert.equal(request.state.dialogue_focus.previous_assistant_message.text, advice.text);
    assert.deepEqual(request.state.dialogue_focus.source_jobs, [{ id: job.id, title: job.title }]);
});

for (const intent of ["work", "question"]) test(`separate_intent_and_ownership_${intent}`, async () => {
    let requested;
    const result = await routePrompt({ prompt: intent === "work" ? "Apply that advice" : "Would that affect exit?", context, options, env, runner: fixture().runner,
        fetch: async (_url, init) => {
            requested = JSON.parse(init.body);
            return { ok:true, json:async()=>({ answers: Object.fromEntries(Object.entries(requested.questions).map(([id,q]) => {
                const choice = id === "review_policy" ? "required" : id === "intent" ? intent : "followup:job_a";
                return [id,{type:"choice",choice,confidence:.99,probabilities:Object.fromEntries(Object.keys(q.criteria).map(k=>[k,k===choice?1:0]))}];
            })) }) };
        } });
    assert.ok(requested.questions.intent, "intent is independent of ownership");
    assert.ok(!Object.keys(requested.questions.route.criteria).some(k=>k.startsWith("discussion:")));
    assert.equal(result.decision.action, intent === "work" ? "followup" : "discussion");
    assert.equal(result.decision.job_id, job.id);
});

test("session_relationship_is_evidence_without_exposing_session_ids_or_selecting_a_job", async () => {
    const jobs = [
        { ...job, session_id: options.session },
        { ...job, id: "job_b", followup_session_id: options.session },
        { ...job, id: "job_c", session_id: "other" },
    ];
    let request;
    const result = await routePrompt({ prompt: "Continue", context, options, env, runner: fixture(jobs).runner,
        fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request, "uncertain") };
        } });
    assert.deepEqual(request.state.candidates.map(c => c.job.same_session), [true, true, false]);
    assert.equal(request.questions.route.criteria["followup:job_a"].same_session, true);
    assert.equal(request.questions.route.criteria["followup:job_a"].preceding_response_source, false);
    assert.equal(result.decision.action, "agent");
    assert.ok(request.state.candidates.every(c => c.job.session_id === undefined));
});

test("recent_dialogue_preserves_user_request_before_many_assistant_updates", async () => {
    const history = [
        { role: "user", text: "Review the login retry design before implementation." },
        ...Array.from({ length: 12 }, (_, i) => ({ role: "assistant", text: `Progress update ${i}: ` + "x".repeat(1800) })),
    ];
    let request;
    await routePrompt({ prompt: "Apply that recommendation", context, options, env, history, runner: fixture().runner,
        fetch: async (_url, init) => {
            request = JSON.parse(init.body);
            return { ok: true, json: async () => response(request) };
        } });
    assert.equal(request.state.dialogue_focus.previous_user_message?.text, history[0].text);
    assert.equal(request.state.recent_conversation.at(-1).role, "assistant");
    assert.ok(request.state.recent_conversation.length <= 8);
    assert.ok(Buffer.byteLength(JSON.stringify(request.state.recent_conversation)) <= 8192);
});
