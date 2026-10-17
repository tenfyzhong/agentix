import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMessages } from '../../shared/opencode-turns.mjs';

function retryHistory() {
    return [
        { id: 'msg_user', type: 'user', text: 'Continue the work' },
        {
            id: 'msg_failed_step',
            type: 'assistant',
            content: [{ type: 'text', text: 'Partial response. ' }],
            error: { type: 'provider.transport', message: 'Connection interrupted' },
            time: { created: 1, completed: 2 },
        },
    ];
}

test('failed_assistant_step_remains_in_progress_during_native_retry', () => {
    const { turns, messageTurns } = normalizeMessages(retryHistory());
    assert.equal(turns[0].status, 'inProgress');
    assert.equal(turns[0].items.at(-1).status, 'inProgress');
    assert.equal(messageTurns.get('msg_failed_step'), 'msg_user');
});

test('successful_retry_finishes_only_when_native_execution_records_idle', () => {
    const messages = [
        ...retryHistory(),
        {
            id: 'msg_retry_step',
            type: 'assistant',
            content: [{ type: 'text', text: 'Recovered response.' }],
            finish: 'stop',
            time: { created: 3, completed: 4 },
        },
    ];
    assert.equal(normalizeMessages(messages).turns[0].status, 'inProgress');

    messages.push({ id: 'msg_idle', type: 'idle', outcome: 'succeeded' });
    const { turns, messageTurns } = normalizeMessages(messages);
    assert.equal(turns[0].status, 'completed');
    assert.equal(turns[0].agent_text, 'Partial response. Recovered response.');
    assert.equal(messageTurns.get('msg_retry_step'), 'msg_user');
});

test('shutdown_after_failed_step_keeps_history_resumable_without_idle_marker', () => {
    // Native shutdown clears live activity but intentionally adds no idle message.
    // The persisted transcript must still identify the unfinished user turn.
    const persisted = retryHistory();
    const restored = normalizeMessages(structuredClone(persisted));
    assert.deepEqual(
        restored.turns.filter((turn) => turn.status === 'inProgress').map((turn) => turn.id),
        ['msg_user'],
    );
});

test('native_failed_idle_still_marks_the_user_turn_failed', () => {
    const messages = [
        ...retryHistory(),
        { id: 'msg_idle', type: 'idle', outcome: 'failed' },
    ];
    assert.equal(normalizeMessages(messages).turns[0].status, 'failed');
});
