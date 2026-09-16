# Card ordering follow-up review (2026-09-17)

This records three regressions found in the initial card writer and their subsequent fixes. Each original regression failed before production changes and now passes. The descriptions below explain the original failure modes; the resolution section describes current behavior.

## P1: Disabling controls discards pending final content

`CardWrites::disable` reserves a new content revision even though it only changes controls. If update A is in flight, final body B is waiting, and an action callback requests disable C, B becomes obsolete. A completes, B returns success without writing, and C only disables A's controls. The final answer body is lost from the card.

`Engine::handle_action` calls `disable_consumed_actions` for accepted callbacks, making this a real second writer alongside background completion. The regression ends with body A instead of B.

Fix direction: represent control changes separately from full-content replacement. Compose pending content and control state in the writer, or preserve the queued content before applying a control-only operation. Merely dropping revision increments is insufficient if an older full update can subsequently restore disabled controls.

Test: `card_review_disable_must_preserve_pending_final_content`.

## P1: Local rate-limit waiting permanently disables card delivery

The five-second edit deadline covers the entire adapter call, including its local outbound queue and rate limiter. The replacement send has the same deadline. Telegram's shared `retry_after` cooldown survives cancellation. With a 30-second cooldown, both calls expire without reaching the provider, and `replacement_uncertain` remains set. Even after cooldown expires, future updates immediately fail without trying the transport.

A paused-clock fixture verifies zero wire calls before the failure and reproduces inability to deliver after the cooldown expires. This is a deterministic model of the existing adapter boundary, not a live Telegram experiment.

Fix direction: distinguish waiting/not-dispatched from dispatched/unknown in the transport contract. Bound local admission separately, preserve queued latest content across cooldowns, and start the remote uncertainty deadline at dispatch. Do not simply clear all timeout tombstones; genuinely in-flight writes still need isolation.

Test: `card_review_local_cooldown_must_not_permanently_disable_card`.

## P2: Definite replacement failure is treated as unknown delivery

The writer sets `replacement_uncertain` before sending, then propagates every error with `?`. A definite `Rejected` or local `InvalidPayload` result therefore retains the same permanent block as a lost response. Correcting the payload or restoring access cannot recover delivery through that logical card.

The regression injects a definite rejection for replacement B, restores a working transport, and attempts C. C still returns `replacement card delivery is uncertain` without sending. The initial edit path similarly retires a card for every error class, including definite local failures.

Fix direction: classify definite rejection/local validation failures separately from unknown commit outcomes. Preserve retirement of the original uncertain message, but allow a safe replacement attempt after a definite failed send. Normalize concrete adapter error mappings as part of this contract.

Test: `card_review_definite_replacement_rejection_must_allow_recovery`.

## Resolution

- Control disabling uses a separate revision barrier. The final body survives, actions through the barrier stay disabled, and fresh revisions can introduce new controls.
- A task-local `DeliveryAttempt` separates MessageCenter/cooldown admission from provider dispatch. The total operation budget is 60 seconds, with a five-second deadline per dispatched request. Cancellation before dispatch restores retryability; unknown in-flight outcomes still isolate the original message.
- Definite edit failures preserve the original target. Definite replacement failures permit a later replacement without reviving the retired original. Telegram and Feishu map provider rejection separately from transport uncertainty. Slack malformed/missing success responses remain unknown, including a missing newly created message ID.
- Feishu retains the cached view until action disabling succeeds. An additional mock-server regression reproduced the lost-cache retry before the fix.

Tests also cover separate timing budgets, rejection followed by cooldown, unsent local failure, cancellation while waiting, definite edit rejection, adapter progress markers, and a Slack HTTP 429 retry delay exceeding the wire budget. These use deterministic fixtures/local mock servers; they do not establish real-provider delivery guarantees.

## Evidence and limits

Run the new regressions with:

```sh
cargo test -p agentix-core --lib card_review_ -- --nocapture
```

All three originally failed for the described assertions and now pass. The local-timeout fixture now uses a 90-second cooldown to exceed the new 60-second local budget; a separate cancellation test verifies successful delivery through a 30-second cooldown. Existing serialization, alias, reload and late-commit regressions remain enabled.

Retired aliases remain retained for the lifetime of the runtime. The performance follow-up replaced the original full-registry scan on every reservation with incremental cleanup of at most eight candidates per lookup. The mapping memory still grows with uncertain writes; full-process-restart isolation remains outside the documented guarantee. See the repeatable measurements in [background-completions.md](background-completions.md).

## Second review: findings and fixes (2026-09-17)

The three original regressions above still pass. The following three regressions were reproduced before changes and are now fixed; the descriptions record their original failure modes.

### P1: Disabled-channel recipients abort local completion archival

`queue_background_completion` iterates inherited conversation owners and propagates `MissingChannel` while assembling recipients. Reload shares the owner registry, while channel switching may remove its transport. One stale recipient therefore aborts admission for every eligible recipient. More importantly, `notify_unattached_turn_completion` now archives only after that optional admission succeeds, so the error leaves the terminal buffer hot and unarchived. The previous handler archived before optional delivery.

Regression: `review_disabled_channel_owner_does_not_prevent_completion_archive` inherits an owner for an unavailable channel and observes `Err(MissingChannel(Feishu))` and no cold record. The reload implementation permits channel changes and retains the interaction coordinator, making this state reachable.

Fix direction: filter unavailable transports while collecting optional recipients and make local terminal cleanup independent of all optional notification setup failures. Cover actual channel-switch reload and both unattached/draining cleanup paths.

### P2: Rejected final updates invalidate the visible Attach button

`Request::publish` revokes the previous action group and issues a fresh token before the card writer confirms delivery. If a loading card was delivered but the final update receives a definite rejection, the writer correctly preserves that card; its visible Attach token, however, has already been removed. The newly issued token was never displayed.

Regression: `review_rejected_final_update_preserves_visible_attach_token` delivers the loading view, rejects the final update, then consumes the visible token with the unchanged owner, backend generation and binding epoch. It fails with `ActionTokenError::Unknown`.

Fix direction: reuse a valid token across loading/final revisions, or commit token replacement only alongside confirmed card delivery. Preserve scope validation and account for definite failure, cancellation, skipped revisions and unknown remote outcomes.

### P2: Feishu structured cards hide loading and missing-content notices

`Request::view` appends loading/unavailable notices only to `OutboundView::body`. With any cached prompt, reasoning or tool section, Feishu's `card_sections::view_body` renders `sections` and ignores that fallback body. The user sees a completed-looking partial card without the explanation that content is still being fetched or could not be recovered. Telegram/Slack body rendering does not have this particular omission.

Regression: `review_missing_content_notice_reaches_rich_card_sections` starts with a cached prompt and no answer, lets lookup return no content, and confirms the unavailable notice exists only in the fallback body. The missing section assertion fails. The Feishu rendering branch was inspected directly; a live Feishu client was not used.

Fix direction: represent loading/missing-content notices in both fallback text and the structured section model, and assert the final Feishu wire payload for partial-cache loading and exhausted/missing history.

Run the completion regressions and the original ordering regressions with:

```sh
cargo test -p agentix-core --lib review_
```

The existing runtime-lifetime alias-retention and full-process-restart limits remain as documented; they are not counted as newly discovered regressions here.

### Second review resolution

- Disabled transports are skipped individually. Preparation captures the terminal snapshot and reserves accepted card revisions before local archival. A completion permit starts optional I/O only after archival and draining cleanup succeed; dropping it cancels the notice. Tests cover inherited owners after transport removal, draining cleanup, cancellation, and a newer card write during cleanup.
- Each recipient reuses its single-use Attach token for the same binding epoch. Failed, cancelled, or obsolete updates do not revoke a visible token. A changed epoch gets a scoped replacement token, while old tokens retain their normal stale-binding rejection. Consumed tokens are not reissued; a conversation already attached to the target shows disabled `Attached`. Binding is revalidated after the card writer wait.
- Loading and unavailable-content notices populate both body and non-collapsible sections. Each final view is rebuilt from the snapshot/history, removing the loading marker. Core tests cover production view construction; a complementary Feishu mock HTTP test checks send/update wire JSON with a cached prompt and each notice.

These tests use deterministic adapters and local mock HTTP servers. They do not substitute for live Feishu client acceptance, nor change the runtime-lifetime alias retention and full-process-restart limits above.

Validation for the second-review fixes: 459 core/Feishu tests passed (one opt-in benchmark ignored), all 10 CLI engine-runtime tests passed, and Clippy with all features/all targets and warnings denied passed for core, Feishu and the CLI. Formatting and whitespace checks passed. The five expected assertion failures from the pre-fix regression run are recorded in the local validation log; cancellation already passed before this change.

## Final integration audit

The full workspace gate exposed six Codex integration assertions that still assumed completion notification delivery was synchronous. Those tests now wait for the owned coordinator to settle before checking content or suppression. Source lookup failure verifies successful local completion with no notification; subagent tests also wait before asserting absence, preventing a false pass before the worker runs. Detached-session, restored-recipient and native goal-input coverage use the same explicit completion boundary.

The architecture audit follows completion capture/cleanup, coordinator cancellation and reload, card revision/control ordering, and provider dispatch/error classification. The implementation keeps one optional worker pool and one caller-owned card writer; adapter pacing and rendering remain shared. No additional event protocol or durable notification outbox is introduced. The resource-retention and full-process-restart limitations remain explicit in the design document.
