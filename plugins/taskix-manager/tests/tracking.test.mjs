import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runHook, registerExtension } from '../runtime.mjs';

const excluded = async args => {
    if (args[0] === 'project' && args[1] === 'check') return {result:{tracking:{enabled:false}}};
    if (args[0] === 'hook' && args[1] !== 'record') return {result:{}};
    throw new Error(`Excluded workspace must not call ${args.join(' ')}`);
};
for (const name of ['SessionStart','UserPromptSubmit','PreToolUse','PostToolUse','Stop','SessionEnd']) {
    test(`excluded_hook_${name}_skips_intake`, async () => {
        assert.deepEqual(await runHook({hook_event_name:name,session_id:'s',cwd:'/private/demo',prompt:'test',transcript_path:'/missing/transcript'},excluded),{});
    });
}
for (const host of ['pi','omp','opencode','antigravity']) {
    test(`excluded_extension_${host}_skips_intake`, async () => {
        const handlers = new Map();
        let timers = 0;
        registerExtension({on:(name,fn)=>handlers.set(name,fn),registerTool(){}},host,excluded,{setInterval(){timers++;return 1;},clearInterval(){}});
        const ctx = {cwd:'/private/demo',sessionManager:{getSessionId:()=> 's'}};
        await handlers.get('session_start')({},ctx);
        assert.equal(timers,1, "lifecycle renewal remains available for existing leases");
        assert.equal(await handlers.get('before_agent_start')({prompt:'test'},ctx),undefined);
        await handlers.get('agent_end')({messages:[{role:'assistant',content:'test'}]},ctx);
        await handlers.get('session_shutdown')({},ctx);
    });
}

test('antigravity_checks_policy_before_reading_native_transcript', async t => {
    const { createAntigravityHooks } = await import('../antigravity/runtime.mjs');
    const { mkdtemp, rm } = await import('node:fs/promises');
    const { tmpdir } = await import('node:os');
    const { join } = await import('node:path');
    const root = await mkdtemp(join(tmpdir(),'tracking-agy-'));
    t.after(()=>rm(root,{recursive:true,force:true}));
    const hooks = createAntigravityHooks({root,key:'test',runner:excluded});
    assert.deepEqual(await hooks.handle({conversationId:'s',workspacePaths:['/private/demo'],transcriptPath:'/missing/transcript',hookEventName:'PreInvocation'}),{});
});

for (const host of ['pi','omp','opencode']) test(`excluded_${host}_still_releases_interrupted_lease`, async () => {
    const handlers = new Map(), calls=[];
    const runner=async args=>{calls.push(args);return excluded(args);};
    registerExtension({on:(name,fn)=>handlers.set(name,fn),registerTool(){}},host,runner,{setInterval:()=>1,clearInterval(){}});
    const ctx={cwd:'/private/demo',sessionManager:{getSessionId:()=> 's'},isIdle:()=>true};
    await handlers.get('session_start')({},ctx);
    await handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'aborted'}]},ctx);
    if (host==='pi') await handlers.get('agent_settled')({},ctx);
    assert.ok(calls.some(args=>args[0]==='hook'&&args[1]==='interrupt'));
});

test('excluded_antigravity_stop_releases_interrupted_lease', async t => {
    const { createAntigravityHooks } = await import('../antigravity/runtime.mjs');
    const { mkdtemp, rm } = await import('node:fs/promises');
    const { tmpdir } = await import('node:os');
    const { join } = await import('node:path');
    const root = await mkdtemp(join(tmpdir(),'tracking-agy-'));
    t.after(()=>rm(root,{recursive:true,force:true}));
    const calls=[];
    const hooks=createAntigravityHooks({root,key:'test',runner:async args=>{calls.push(args);return excluded(args);}});
    await hooks.handle({conversationId:'s',workspacePaths:['/private/demo'],transcriptPath:'/missing/transcript',hookEventName:'Stop',fullyIdle:true,terminationReason:'TERMINAL_CANCELLED'});
    assert.ok(calls.some(args=>args[0]==='hook'&&args[1]==='interrupt'));
});
