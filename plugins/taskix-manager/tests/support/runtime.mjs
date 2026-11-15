// Existing routing/lifecycle unit fixtures model an enabled workspace. Policy
// admission itself is exercised against the real runtime in tracking.test.mjs.
import { runHook as hook, registerExtension as extension } from '../../runtime.mjs';
export { buildArgs, runTaskix } from '../../runtime.mjs';
const enabled = runner => runner && ((args, options) => args[0] === 'project' && args[1] === 'check'
    ? Promise.resolve({result:{tracking:{enabled:true}}}) : runner(args,options));
export const runHook = (event, runner, routing) => hook(event, enabled(runner), routing);
export const registerExtension = (api, host, runner, ...rest) => extension(api,host,enabled(runner),...rest);
