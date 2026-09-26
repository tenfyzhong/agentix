import assert from "node:assert/strict";
import { test } from "node:test";
import { withDeadline } from "../jev-io.mjs";

test("deadline_rejects_even_when_operation_ignores_abort", async t => {
    t.mock.timers.enable({ apis: ["setTimeout"] });
    let signal;
    const pending = withDeadline(undefined, s => { signal = s; return new Promise(() => {}); });
    const rejected = assert.rejects(pending, /aborted|deadline/i);
    t.mock.timers.tick(8000);
    await rejected;
    assert.equal(signal.aborted, true);
});
test("external_signal_does_not_disable_internal_deadline", async t => {
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const pending = withDeadline(new AbortController().signal, () => new Promise(() => {}));
    const rejected = assert.rejects(pending);
    t.mock.timers.tick(8000);
    await rejected;
});
test("already_aborted_deadline_performs_no_work", async () => {
    const controller = new AbortController(); controller.abort();
    await assert.rejects(withDeadline(controller.signal, () => assert.fail("No work after cancellation")));
});
test("nested_operations_share_one_deadline_signal", async () => {
    await withDeadline(undefined, signal => withDeadline(signal, nested => assert.equal(nested, signal)));
});
test("external_cancellation_returns_without_waiting_for_operation", async () => {
    const controller = new AbortController();
    const pending = withDeadline(controller.signal, () => new Promise(() => {}));
    controller.abort();
    await assert.rejects(pending);
});
test("response_reader_bounds_streamed_and_declared_size", async () => {
    const { readJson } = await import("../jev-io.mjs");
    assert.deepEqual(await readJson(new Response('{"ok":true}'), new AbortController().signal), {ok:true});
    await assert.rejects(readJson(new Response("{}", {headers:{"content-length":"1048577"}}), new AbortController().signal), /large/);
    await assert.rejects(readJson(new Response(" ".repeat(1048577)), new AbortController().signal), /large/);
    await assert.rejects(readJson(new Response("{"), new AbortController().signal));
});
