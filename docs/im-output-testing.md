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
| Structured sections under all four reasoning/tool visibility combinations | `configured_process_output_survives_final_answer_and_deduplicates_items` |
| Streamed commentary, late completion classification, repeated start events, cold restore | Commentary tests in `render_tests.rs` |
| You/Agent single-newline spacing, alternating collapsed process panels | `turn_sections_render_as_independent_collapsible_process_panels` |
| HTTP send, update with final answer, disable actions without losing panels or changing their IDs | `structured_cards_survive_send_update_and_action_disabling` |
| Background panel nesting and actions | `background_sections_keep_panels_and_actions` |
| Long Chinese/emoji content, truncation, 40 panels, unique IDs, retained final answer | `long_unicode_sections_and_many_panels_retain_the_final_answer` |
| Old serialized views without sections | `legacy_views_without_sections_still_render_plain_markdown` |

The Feishu tests validate card JSON and requests against a mock OpenAPI server.
They do not emulate the Feishu client. Client acceptance still needs a live turn:
expand a Reasoning or Tool Call panel, let another update arrive, and inspect the
expanded state and final layout on the supported desktop/mobile clients. The
renderer currently sends `expanded: false` on each render; stable element IDs
alone do not prove that a client preserves a user's expanded state. No automated
client-interaction coverage or line-coverage percentage is claimed here.
