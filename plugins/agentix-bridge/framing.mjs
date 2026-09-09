import { MAX_FRAME_BYTES } from './protocol/constants.mjs';

/** Assemble each frame once, retaining fragments instead of repeatedly copying its prefix. */
export class FrameDecoder {
    constructor(limit = MAX_FRAME_BYTES) { this.limit = limit; this.parts = []; this.bytes = 0; }
    push(chunk) {
        const frames = [];
        let offset = 0;
        while (offset < chunk.length) {
            const newline = chunk.indexOf(10, offset);
            const end = newline < 0 ? chunk.length : newline;
            const part = chunk.subarray(offset, end);
            if (this.bytes + part.length + 1 > this.limit) throw new Error('Bridge frame exceeds maximum size');
            if (part.length) { this.parts.push(part); this.bytes += part.length; }
            if (newline < 0) break;
            frames.push(Buffer.concat(this.parts, this.bytes));
            this.parts = []; this.bytes = 0;
            offset = newline + 1;
        }
        return frames;
    }
}
