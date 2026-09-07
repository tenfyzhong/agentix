# Task, Job and Inbox state machines

SQLite enforces these transitions. The desktop Taskcli Sync plugin submits saved Obsidian `status` edits through the same CLI; a rejected edit is restored and produces a notification. See [Obsidian setup and supported edits](../plugins/agent-task-manager/obsidian/README.md#status-edits).

## Task

```mermaid
stateDiagram-v2
    [*] --> TODO: task add
    TODO --> IN_PROGRESS: claim
    BLOCKED --> IN_PROGRESS: claim / eligible session resume
    WAITING_USER --> IN_PROGRESS: claim
    state IN_PROGRESS {
        [*] --> PLANNING
        PLANNING --> EXECUTING: start (published Plan + DONE dependencies)
    }
    IN_PROGRESS --> DONE: done (EXECUTING + owning lease)
    IN_PROGRESS --> BLOCKED: block / release / interruption / lease expiry
    IN_PROGRESS --> WAITING_USER: wait
    IN_PROGRESS --> FAILED: fail
    IN_PROGRESS --> CANCELLED: cancel
    TODO --> BLOCKED: block / release
    TODO --> WAITING_USER: wait
    TODO --> CANCELLED: cancel
    BLOCKED --> WAITING_USER: wait
    WAITING_USER --> BLOCKED: block / release
    BLOCKED --> FAILED: fail
    WAITING_USER --> FAILED: fail
    BLOCKED --> CANCELLED: cancel
    WAITING_USER --> CANCELLED: cancel
    FAILED --> TODO: retry
    DONE --> TODO: reopen
    CANCELLED --> TODO: reopen
    classDef todo fill:#cbd5e1,stroke:#cbd5e1,color:#1f2937
    classDef blocked fill:#fed7aa,stroke:#fed7aa,color:#1f2937
    classDef waiting fill:#ddd6fe,stroke:#ddd6fe,color:#1f2937
    classDef done fill:#bbf7d0,stroke:#bbf7d0,color:#1f2937
    classDef failed fill:#fecaca,stroke:#fecaca,color:#1f2937
    classDef cancelled fill:#e2d7e7,stroke:#e2d7e7,color:#1f2937
    class TODO todo
    class BLOCKED blocked
    class WAITING_USER waiting
    class DONE done
    class FAILED failed
    class CANCELLED cancelled
```

Claim creates a fresh lease and always enters PLANNING. Plan creation/revision and heartbeat preserve the phase; start retains the lease. Leaving IN_PROGRESS clears its phase and lease. Session resumption only reclaims eligible system-blocked work. Retry/reopen require an unarchived Job; reopening a prerequisite is rejected if a dependent has already started execution. Job cancellation also cancels unfinished Tasks after their leases have been released, while preserving DONE/FAILED/CANCELLED outcomes. These guards apply in addition to the arrows.

## Job

```mermaid
stateDiagram-v2
    [*] --> ACTIVE: job create
    ACTIVE --> PENDING_REVIEW: readiness becomes true / job submit
    PENDING_REVIEW --> COMPLETED: job approve (verification passed)
    PENDING_REVIEW --> ACTIVE: job reject --reason (verification failed)
    PENDING_REVIEW --> ACTIVE: task reopen
    COMPLETED --> ACTIVE: task reopen / Inbox set-status TODO
    ACTIVE --> CANCELLED: job cancel
    PENDING_REVIEW --> CANCELLED: job cancel
    CANCELLED --> ACTIVE: task retry / reopen / Inbox set-status TODO
    classDef active fill:#bfdbfe,stroke:#bfdbfe,color:#1f2937
    classDef review fill:#fed7aa,stroke:#fed7aa,color:#1f2937
    classDef completed fill:#bbf7d0,stroke:#bbf7d0,color:#1f2937
    classDef cancelled fill:#e2d7e7,stroke:#e2d7e7,color:#1f2937
    class ACTIVE active
    class PENDING_REVIEW review
    class COMPLETED completed
    class CANCELLED cancelled
```

Readiness means at least one non-CANCELLED Task exists and every such Task is DONE. Only a false-to-true readiness transition submits automatically. Cancelling every Task never counts as delivery. A rejection records `review_reason` and preserves every Task's status; metadata edits, heartbeats, and sync do not resubmit it. Reopen a Task or add repair work to make changes, then finish that work, or explicitly use `job submit` after rechecking unchanged deliverables.

Approval alone sets `completed_at` and emits `job.completed`. Submission emits `job.pending_review`; rejection emits `job.rejected`. Resubmission and approval clear the current review reason; events retain the history. The CLI checks state and readiness, while the reviewer is responsible for performing acceptance checks. Agents must not approve their own delivery unless the user explicitly authorizes them to perform that verification.

PENDING_REVIEW is unfinished: it cannot be archived, blocks Project archival and new Inbox intake, and keeps its Inbox entry IN_PROGRESS without a lease. Rejection makes that entry available to resume its existing Job. Approval checks it off. COMPLETED and CANCELLED Jobs may be archived/unarchived; archival is an independent property, not another status. Database schema 9 preserves historical COMPLETED Jobs.

## Inbox item

```mermaid
stateDiagram-v2
    [*] --> TODO: submission
    TODO --> IN_PROGRESS: explicit claim-next
    IN_PROGRESS --> TODO: release / interruption / lease expiry / Job rejection
    IN_PROGRESS --> DONE: Job approval / checked box approves pending Job
    TODO --> DONE: set-status DONE (unlinked item only)
    TODO --> CANCELLED: cancellation / withdrawal
    IN_PROGRESS --> CANCELLED: cancellation / withdrawal / Job cancellation
    DONE --> TODO: set-status TODO / uncheck
    CANCELLED --> TODO: set-status TODO / uncheck
```

The connected Obsidian plugin maps saved checkbox edits to `inbox set-status`, with a revision check, idempotency key and rollback notification on failure. Reopening a terminal entry reuses its Job, sets that Job ACTIVE, and preserves all Task states. It does not automatically claim work or rerun DONE Tasks. Archived work must be unarchived first; withdrawn entries cannot be revived. Cancelled entries must be reopened before completion, and completed entries before cancellation.

TODO and IN_PROGRESS share the blank checkbox. PENDING_REVIEW keeps the entry IN_PROGRESS without a lease; approval requires the Job's readiness checks. A blank box alone cannot release active ownership or reject verification: use the corresponding lease-authorized release or Job rejection. Plain CLI sync retains cancellation/withdrawal import but does not replay completion/reopening from checkbox drift.
