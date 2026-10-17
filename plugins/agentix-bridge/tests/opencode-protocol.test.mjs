import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validate } from '../protocol/validate.mjs';

test('opencode_interaction_events_round_trip_without_changing_v2', () => {
    const request = {
        rpc_id: 'request-1', method: 'opencode.permission', session_id: 'ses_1',
        turn_id: 'msg_1', item_id: null, kind: 'command_approval', title: 'Permission',
        detail: 'Run command', available_decisions: ['once', 'always', 'reject'],
        payload: {}, auto_resolution_ms: null,
    };
    assert.equal(validate('Event', { InteractionRequested: request }), true);
    assert.equal(validate('Event', { InteractionResolved: { session_id: 'ses_1', request_id: 'request-1' } }), true);
});
