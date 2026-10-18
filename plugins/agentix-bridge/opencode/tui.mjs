import { OpenCodeBridge } from './runtime.mjs';

// Plugin.define is an identity helper; this dependency-free definition follows
// the OpenCode 2 TUI plugin contract directly.
export default {
    id: 'agentix.bridge',
    async setup(context) {
        const bridge = new OpenCodeBridge(context);
        await bridge.start();
        return () => bridge.close();
    },
};
