# IM output regression coverage

Run `make check` for formatting, strict workspace Clippy, all-feature Rust tests,
and plugin tests. The following suites cover the IM help, session naming, and
structured process output changes.

| Behavior | Automated coverage |
| --- | --- |
| Help descriptions, attached/detached commands, backend and read-only restrictions | `agentix-core/tests/engine.rs` help and command tests |
| Native session ID suffixes, distinct IDs, preserved names/previews, four agent kinds | `agentix-core/src/registry.rs::tests` |
| Reasoning start labels, process visibility policy | `agentix-core/src/output.rs::tests` |
| Commentary/final answer classification, null and unknown phase compatibility | `agentix-codex/tests/protocol.rs` |
| Multiline interleaving, duplicate item completion, final answer retention, errors | `agentix-core/src/engine/render_tests.rs` |
| Consecutive reasoning/tool grouping in cards and Markdown, item updates, duplicate completion, cold restore | `consecutive_process_blocks_merge_and_survive_updates_and_cold_storage` |
| Structured sections under all four reasoning/tool visibility combinations | `configured_process_output_survives_final_answer_and_deduplicates_items` |
| Streamed commentary, late completion classification, repeated start events, cold restore | Commentary tests in `render_tests.rs` |
| You/Agent single-newline spacing, alternating process panels | `turn_sections_render_as_independent_collapsible_process_panels` |
| HTTP send, update with final answer, disable actions without losing panels or changing their IDs | `structured_cards_survive_send_update_and_action_disabling` |
| Current process panel expanded, previous panels collapsed on the next block, including Background/history cards | `current_process_panel_expands_until_the_next_block_arrives` |
| Background, attach, and history share live formatting under all four visibility combinations | `background_and_history_share_live_process_format_and_visibility` |
| Completion after switching sessions recovers process items from history | `draining_completion_includes_process_items_produced_after_switching` |
| Process items plus an ID-less answer summary, streaming/completion, duplicate completion and cold restore | `history_process_summary_adopts_real_answer_id_after_cold_restore` |
| Truncated draining history preserves early items and updates matching IDs and the cumulative answer | `draining_truncated_history_preserves_received_items_and_updates_matching_ids` |
| Aggregated user input survives history, attach and background notifications; missing summaries use all user items | `history_routes_preserve_all_user_inputs`, `history_preserves_aggregate_user_input_and_aggregates_items_when_missing` |
| Summary-only history does not duplicate the completed answer | `summary_only_history_adopts_the_first_completed_output_id_without_duplication` |
| Running history retains process items when streaming continues | `attaching_running_history_retains_process_items_when_streaming_continues` |
| Structured output retains intermediate answers between process blocks | `structured_output_preserves_process_blocks_after_agent_output` |
| Background panel nesting and actions | `background_sections_keep_panels_and_actions` |
| Long Chinese/emoji content, truncation, 40 panels, unique IDs, retained final answer | `long_unicode_sections_and_many_panels_retain_the_final_answer` |
| Read-only Codex polling updates the native answer ID restored by attach | `external_writer_polling_delivers_content_without_live_notifications` |
| A reused assistant ID closes the tool panel without reordering, including cold restore and late tool completion | `cumulative_answer_closes_process_panel_without_reordering_and_survives_cold_restore` |
| Explicit expansion state takes precedence over section position; legacy views retain positional defaults | `explicit_process_expansion_overrides_section_position`, `current_process_panel_expands_until_the_next_block_arrives` |
| Repeated answer completion and unchanged native/summary history do not change the active panel | `repeated_answer_completion_does_not_close_current_tool`, `merging_unchanged_history_preserves_active_tool`, `unchanged_summary_fallback_merge_keeps_the_active_tool` |
| Resumed commentary expands its original panel and collapses the previous tool | `resumed_commentary_stream_reopens_its_panel` |
| Background error blocks collapse preceding process panels | `background_error_closes_the_previous_process_panel` |
| Native Pi/OMP history reconstructs reasoning/tool inputs and results, preserves mapped IDs across restore, and bounds long histories | `plugins/agentix-bridge/tests/session.test.mjs` native history tests |
| Hidden commentary placeholders leave plain output formatting unchanged | `hidden_commentary_placeholder_does_not_change_plain_output_format` |
| Native Codex goal objectives in paged/fallback history, read-only attach and background completion; unavailable rollouts preserve answers | `goal_input_is_restored_per_turn_for_history_and_read_only_attach` |
| Native goal input on the first live output without a user-message event | `goal_input_is_visible_on_first_live_output_without_user_message_event` |
| IM goal objectives activate completed, paused, and blocked goals without submitting a duplicate prompt | `im_goal_objective_activates_existing_stopped_goals` |
| Goal context extraction excludes internal instructions and requires matching session/turn boundaries | `agentix-codex/src/client/goal_input.rs::tests` |
| Old serialized views without sections | `legacy_views_without_sections_still_render_plain_markdown` |

The Feishu tests validate card JSON and requests against a mock OpenAPI server.
They do not emulate the Feishu client. Client acceptance still needs a live turn:
check that the current Reasoning or Tool Call panel opens automatically, then
collapses when the next visible block arrives. Also check manually opened older
panels and the final layout on supported desktop/mobile clients. The renderer
sends the desired expanded state on each update; stable element IDs alone do not
prove client behavior. No automated client-interaction coverage or line-coverage
percentage is claimed here.

Native Codex goal turns omit ordinary user-message items. Agentix reads the local
rollout path returned by `thread/read` and displays the exact turn's objective as
`/goal <objective>` in the You section. This fallback requires a locally readable
rollout; remote or missing rollout files leave the input unavailable without
discarding the answer. It never substitutes the thread's latest goal into history.
