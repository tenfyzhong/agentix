import assert from 'node:assert/strict';
import {test} from 'node:test';
import * as jev from '../jev.mjs';
import {runHook, registerExtension} from '../runtime.mjs';
import {mkdtemp, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
const env={TASKIX_JEV_ENABLED:'true',TASKIX_JEV_URL:'https://jev.test/evaluate',TASKIX_JEV_API_KEY:'secret'};
const pending={revision:7,complete:true,session_id:'s',turns:[
    {turn_id:'one',messages:[{id:'a',role:'user',text:'Cards lose long output'},{id:'b',role:'assistant',text:'Split by capacity'}]},
    {turn_id:'other',messages:[{id:'c',role:'user',text:'Explain a different project'}]},
    {turn_id:'now',messages:[{id:'d',role:'user',text:'Implement the latest card plan'}]},
]};
const target={title:'Capacity-based cards',goal:'Keep short cards intact and continue long cards',prompt:'Implement the latest card plan'};
function answer(body, choices=['related','unrelated']) {
    return {answers:Object.fromEntries(Object.entries(body.questions).map(([id,q],i)=>[id,{type:'choice',choice:choices[i],confidence:.99,probabilities:Object.fromEntries(Object.keys(q.criteria).map(k=>[k,k===choices[i]?1:0]))}]))};
}
test('Jev selects multiple discussion turns with complete source text and guarded revision',async()=>{
    let request;
    const result=await jev.classifyDiscussionTurns({target,pending,currentTurn:'now',env,fetch:async(_url,init)=>{
        request=JSON.parse(init.body);return {ok:true,json:async()=>answer(request)};
    }});
    assert.equal(result.status,'selected');
    assert.deepEqual(result.turn_ids,['one','now']);
    assert.equal(result.revision,7);
    assert.deepEqual(request.state.turns[0].messages,pending.turns[0].messages);
});
for (const [name,patch] of [['disabled',{env:{}}],['incomplete',{pending:{...pending,complete:false}}],['oversize',{target:{...target,goal:'界'.repeat(30000)}}]]) {
    test(`discussion_${name}_falls_back_without_http`,async()=>{
        const result=await jev.classifyDiscussionTurns({target,pending,currentTurn:'now',env,fetch:()=>assert.fail('Must not contact Jev'),...patch});
        assert.equal(result.status,'agent');
        assert.equal(result.turn_ids,undefined);
    });
}
for (const choice of ['uncertain','invented']) {
    test(`discussion_${choice}_cannot_partially_attach`,async()=>{
        const result=await jev.classifyDiscussionTurns({target,pending,currentTurn:'now',env,fetch:async(_url,init)=>({ok:true,json:async()=>answer(JSON.parse(init.body),['related',choice])})});
        assert.equal(result.status,'agent');
        assert.equal(result.turn_ids,undefined);
    });
}

test('Codex stages the current turn before tools and preserves its identity on Stop',async t=>{
    const dir=await mkdtemp(join(tmpdir(),'taskix-discussion-test-'));t.after(()=>rm(dir,{recursive:true,force:true}));
    const path=join(dir,'transcript.jsonl');
    const rows=[{type:'event_msg',payload:{type:'task_started',turn_id:'turn'}},
        {type:'response_item',payload:{type:'message',role:'user',id:'u',content:'Implement this'}}];
    await writeFile(path,rows.map(JSON.stringify).join('\n'));
    const captures=[];
    const runner=async(args)=>{
        if(args[1]==='record')captures.push(JSON.parse(await readFile(args[args.indexOf('--file')+1],'utf8')));
        return {result:{}};
    };
    const event={session_id:'s',turn_id:'turn',transcript_path:path,cwd:dir};
    await runHook({...event,hook_event_name:'PreToolUse'},runner,{cacheDir:dir});
    assert.equal(captures[0]?.turn_id,'turn');
    await runHook({...event,hook_event_name:'PreToolUse'},runner,{cacheDir:dir});
    assert.equal(captures.length,1,'one draft write per initial turn, not per tool');
    await runHook({...event,hook_event_name:'Stop'},runner,{cacheDir:dir});
    assert.equal(captures[1].turn_id,'turn');
    assert.equal(captures[0].messages[0].id,captures[1].messages[0].id);
});
for(const host of ['pi','omp'])test(`${host}_stages_prompt_before_agent_work_and_appends_replies`,async t=>{
    const handlers=new Map(),captures=[];
    const runner=async(args)=>{if(args[1]==='record')captures.push(JSON.parse(await readFile(args[args.indexOf('--file')+1],'utf8')));return {result:{}};};
    registerExtension({on:(key,fn)=>handlers.set(key,fn),registerTool(){}},host,runner,{setInterval:()=>1,clearInterval(){}});
    const ctx={cwd:'/work',sessionManager:{getSessionId:()=>host},ui:{notify(){}}};
    t.after(()=>handlers.get('session_shutdown')({},ctx));
    await handlers.get('session_start')({},ctx);
    await handlers.get('before_agent_start')({prompt:'Discuss cards'},ctx);
    assert.equal(captures[0]?.messages[0].text,'Discuss cards');
    assert.ok(captures[0].turn_id);
    await handlers.get('agent_end')({messages:[{role:'user',content:'Discuss cards'},{role:'assistant',content:'Use capacity'}]},ctx);
    assert.equal(captures[1].turn_id,captures[0].turn_id);
    assert.equal(captures[1].messages.filter(m=>m.role==='user').length,1);
    assert.equal(captures[1].messages[0].id,captures[0].messages[0].id);
});

test('Pi bridge and Taskix share a turn identity for duplicate event delivery',async t=>{
    const {SessionState}=await import('../../agentix-bridge/session.mjs');
    const handlers=new Map();
    const ctx={cwd:'/work',sessionManager:{getSessionId:()=> 'shared',getEntries:()=>[]},isIdle:()=>true};
    const captures=[];
    const runner=async args=>{if(args[1]==='record')captures.push(JSON.parse(await readFile(args[args.indexOf('--file')+1],'utf8')));return {result:{}};};
    registerExtension({on:(event,fn)=>handlers.set(event,fn),registerTool(){}},'pi',runner,{setInterval:()=>1,clearInterval(){}});
    t.after(()=>handlers.get('session_shutdown')({},ctx));
    await handlers.get('session_start')({},ctx);
    await handlers.get('before_agent_start')({prompt:'Same prompt'},ctx);
    const bridge=new SessionState(ctx);bridge.pendingText='Same prompt';bridge.start();
    assert.equal(bridge.turn.id,captures[0].turn_id);
    assert.equal(captures[0].messages[0].id,`${bridge.turn.id}:${bridge.turn.id}:user`);
    await handlers.get('agent_end')({messages:[{role:'assistant',content:'Reply'}]},ctx);
    assert.equal(captures[1].messages[1].id,`${bridge.turn.id}:${bridge.turn.id}:assistant`);
});

for(const mode of ['low_confidence','bad_probabilities','transport','aborted'])test(`discussion_${mode}_falls_back_without_exposing_service_details`,async()=>{
    const controller=new AbortController();
    if(mode==='aborted')controller.abort();
    const result=await jev.classifyDiscussionTurns({target,pending,currentTurn:'now',env,signal:controller.signal,fetch:async(_url,init)=>{
        if(mode==='transport')throw new Error('secret service failure');
        const data=answer(JSON.parse(init.body));
        if(mode==='low_confidence')data.answers.turn_0.confidence=.1;
        if(mode==='bad_probabilities')data.answers.turn_0.probabilities.related=.1;
        return {ok:true,json:async()=>data};
    }});
    assert.equal(result.status,'agent');
    assert.ok(!JSON.stringify(result).includes('secret'));
});

for(const changed of ['candidates','target'])test(`discussion_helper_revalidates_${changed}_before_returning_write_arguments`,async()=>{
    const {selectDiscussion}=await import('../discussion.mjs');
    const job={id:'job_one',title:'Cards',prompt:'Original',goal:'Split',status:'ACTIVE',revision:3};
    const calls=[];
    const runner=async(args)=>{
        calls.push(args);
        if(args[0]==='job')return {result:job};
        if(args[0]==='routing')return {result:{...job,revision:changed==='target'?4:3}};
        return {result:args.at(-1)==='1'?{revision:changed==='candidates'?8:7}:pending};
    };
    const result=await selectDiscussion({target:{...target,job_id:job.id},current_turn:'now'},{session:'s'},runner,{env,fetch:async(_url,init)=>({ok:true,json:async()=>answer(JSON.parse(init.body))})});
    assert.equal(result.status,'agent');
    assert.equal(result.args,undefined);
    assert.ok(calls.every(args=>['show','list','revision'].includes(args[1])));
});

test('native agent_end retries preserve the staged user message identity',async t=>{
    const handlers=new Map(),captures=[];
    const runner=async args=>{if(args[1]==='record')captures.push(JSON.parse(await readFile(args[args.indexOf('--file')+1],'utf8')));return {result:{}};};
    registerExtension({on:(key,fn)=>handlers.set(key,fn),registerTool(){}},'pi',runner,{setInterval:()=>1,clearInterval(){}});
    const ctx={cwd:'/work',sessionManager:{getSessionId:()=> 'retry'}};
    t.after(()=>handlers.get('session_shutdown')({},ctx));
    await handlers.get('session_start')({},ctx);
    await handlers.get('before_agent_start')({prompt:'Same prompt'},ctx);
    const event={messages:[{role:'user',content:'Same prompt'},{role:'assistant',content:'Reply'}]};
    await handlers.get('agent_end')(event,ctx);
    await handlers.get('agent_end')(event,ctx);
    assert.deepEqual(captures[1],captures[2]);
});
