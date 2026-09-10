// Subprocess fixture: emulate a native watcher that misses every notification.
// The real MCP server must consume hooks published after startup by polling.
import fs from 'node:fs';
import { EventEmitter } from 'node:events';
import { syncBuiltinESMExports } from 'node:module';

fs.watch = () => {
    const watcher = new EventEmitter();
    watcher.close = () => {};
    return watcher;
};
syncBuiltinESMExports();
