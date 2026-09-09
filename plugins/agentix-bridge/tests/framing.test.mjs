import test from 'node:test';
import assert from 'node:assert/strict';
import { FrameDecoder } from '../framing.mjs';

test('native framing applies the limit to individual frames, including newline, not a socket chunk', () => {
    const decoder = new FrameDecoder(8);
    assert.deepEqual(decoder.push(Buffer.from('1234567\nx\n')).map(b => b.toString()), ['1234567', 'x']);
    assert.throws(() => decoder.push(Buffer.from('12345678\n')), /maximum/);
});
test('native framing preserves split UTF-8 and fragmented/coalesced frames', () => {
    const decoder = new FrameDecoder(64);
    const input = Buffer.from('{"text":"中文"}\n{"n":2}\n');
    const lines = [];
    for (const byte of input) lines.push(...decoder.push(Buffer.from([byte])));
    assert.deepEqual(lines.map(b => JSON.parse(b.toString('utf8'))), [{ text: '中文' }, { n: 2 }]);
    assert.deepEqual(decoder.push(Buffer.from('partial')), []);
    assert.deepEqual(decoder.push(Buffer.from('\nnext\n')).map(b => b.toString()), ['partial', 'next']);
});
test('native framing rejects an oversized incomplete frame before a newline arrives', () => {
    const decoder = new FrameDecoder(8);
    decoder.push(Buffer.from('1234'));
    assert.throws(() => decoder.push(Buffer.from('5678')), /maximum/);
});
