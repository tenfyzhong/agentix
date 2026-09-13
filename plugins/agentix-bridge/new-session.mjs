import { failure } from './protocol/errors.mjs';
import { setTimeout as sleep } from 'node:timers/promises';

/** Restricted terminal control. No caller-supplied command or shell text. */
export class NativeSessionControl {
    constructor(kind, options) {
        if (!['omp', 'claude'].includes(kind)) throw failure('invalid_request', 'Unsupported native session control');
        this.kind = kind; this.options = options;
    }
    async verify(terminal) {
        const [, command, , , mode, dead] = await terminal.inspect();
        if (command !== this.kind || mode !== '0' || dead !== '0') throw failure('busy', 'Original agent is not the active terminal pane');
        await terminal.foreground();
    }
    async newSession(busy) {
        const terminal = await this.options.resolve();
        await this.verify(terminal);
        if (busy()) {
            if (this.options.stop) await this.options.stop();
            else await terminal.call(['send-keys', '-t', terminal.pane, 'C-c']);
            const deadline = Date.now() + 30000;
            while (busy()) {
                if (Date.now() >= deadline) throw failure('busy', 'The active task did not stop');
                await sleep(50);
            }
        }
        await this.options.ready(terminal);
        await this.verify(terminal);
        await terminal.call(['send-keys', '-t', terminal.pane, '-l', this.kind === 'claude' ? '/clear' : '/new']);
        await this.verify(terminal);
        await terminal.call(['send-keys', '-t', terminal.pane, 'Enter']);
    }
}
