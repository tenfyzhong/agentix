import { setupOpenCode } from './runtime.mjs';

// Plugin.define is an identity helper; the native shape needs no SDK dependency.
export default {
    id: 'agentix.taskix-manager',
    setup: (context) => setupOpenCode(context),
};
