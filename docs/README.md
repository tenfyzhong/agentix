# Technical documentation

Installation, configuration, commands, and other user guides live in the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki). New users should start with [Getting started](https://github.com/tenfyzhong/agentix/wiki/Getting-Started). For development setup, tests, and releases, see [CONTRIBUTING.md](../CONTRIBUTING.md).

## Architecture and protocols

- [Product design](product-design.md)
- [Architecture and message flow](architecture.md)
- [Native host protocol](host-protocol.md)
- [Session lifecycle](session-lifecycle.md)
- [Background completions](background-completions.md)
- [Task state machines](task-state-machines.md)
- [Task workflow mechanisms](task-workflow-mechanisms.md)
- [Taskix memory design](taskix-memory-design.md)
- [Jev lifecycle decisions](jev-lifecycle-decisions.md)
- [Claude bridge protocol and verification](host-protocol.md#protocol-and-lifecycle)

## Testing and measurements

- [Integration coverage](integration-coverage.md)
- [IM output testing](im-output-testing.md)
- [Performance measurements](performance.md)
- [Project resolution performance](project-resolution-performance.md)
- [Task routing performance](task-routing-performance.md)
- [Jev confidence evaluation](jev-confidence-thresholds.md)
- [Task board validation](task-board.md#validation)
- [Event retention validation](taskix-event-retention-validation.md)
- [Memory acceptance](taskix-memory-acceptance.md)
- [Memory calibration](taskix-memory-calibration.md)
- [Memory performance](taskix-memory-performance.md)
- [Memory quality benchmark](taskix-memory-quality-benchmark.md)
- [Backup verification](integration-coverage.md#taskix-backup-verification)
- [CI cost and Codex proxy verification](integration-coverage.md#ci-test-cost)

## Implementation reviews

- [Architecture review](architecture-review.md)
- [Background completions review](background-completions-review.md)
- [Native bridge review](native-bridge-review.md)
- [Slack review](slack-review.md)
- [PR 92 full review](pr-92-full-review.md)

Obsolete user-guide files are removed after migration. Technical chapters from mixed guides are consolidated into the documents indexed above.
