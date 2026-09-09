# Task, Job and Inbox state machines

SQLite enforces these transitions. The desktop Taskix Sync plugin submits saved Obsidian `status` edits through the same CLI; a rejected edit is restored and produces a notification. See [Obsidian setup and supported edits](../plugins/taskix-manager/obsidian/README.md#status-edits).

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
    note right of PENDING_REVIEW
        Verification is pending.
        Explicitly requested next Inbox work may start.
    end note
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

`pending_review_at` records the most recent transition into PENDING_REVIEW, including automatic readiness and explicit resubmission. Metadata edits, rejection, and approval preserve it; the Dashboard Pending review view sorts pending Jobs oldest first by this field; Recent Jobs sorts all statuses by latest update. Job document timestamps use the computer’s local time zone.

Approval alone sets `completed_at` and emits `job.completed`. Submission emits `job.pending_review`; rejection emits `job.rejected`. Resubmission and approval clear the current review reason; events retain the history. The CLI checks state and readiness, while the reviewer is responsible for performing acceptance checks. Agents must not approve their own delivery unless the user explicitly authorizes them to perform that verification.

PENDING_REVIEW is unfinished: it cannot be archived, blocks Project archival but does not block explicitly requested new Inbox intake, and sets its Inbox entry PENDING_REVIEW without a lease. Rejection returns that entry to ACTIVE for explicit recovery of its existing Job. Approval checks it off. COMPLETED and CANCELLED Jobs may be archived/unarchived; archival is an independent property, not another status. Database schema 9 preserves historical COMPLETED Jobs.

## Inbox item

| Checkbox | State |
| --- | --- |
| `- [ ]` | TODO |
| `- [/]` | ACTIVE |
| `- [r]` | PENDING_REVIEW |
| `- [x]` | COMPLETED |
| `- [-]` | CANCELLED |

```mermaid
stateDiagram-v2
    [*] --> TODO: submission
    TODO --> ACTIVE: explicit claim-next / set-status ACTIVE
    ACTIVE --> TODO: release / interruption / lease expiry / queue without active leases
    ACTIVE --> PENDING_REVIEW: Tasks ready / submit ready Job / mark unlinked item
    PENDING_REVIEW --> ACTIVE: verification rejected
    PENDING_REVIEW --> TODO: queue for recovery (reject review)
    PENDING_REVIEW --> COMPLETED: verification approved / complete unlinked item
    TODO --> PENDING_REVIEW: mark unlinked item
    TODO --> COMPLETED: complete an unlinked item
    ACTIVE --> COMPLETED: complete an unlinked item
    TODO --> CANCELLED: cancellation / withdrawal
    ACTIVE --> CANCELLED: cancellation / withdrawal
    PENDING_REVIEW --> CANCELLED: cancellation / withdrawal
    COMPLETED --> ACTIVE: reopen with slash
    CANCELLED --> ACTIVE: reopen with slash
    COMPLETED --> TODO: reopen with blank checkbox
    CANCELLED --> TODO: reopen with blank checkbox
```

The connected plugin submits checkbox edits through `inbox set-status` with revision checks and idempotency keys. Failed edits restore the matching checkbox and notify the user. Job readiness automatically sets the Inbox item PENDING_REVIEW; review rejection and approval map to ACTIVE and COMPLETED. An explicit TODO represents queued/released work, even when its existing Job remains ACTIVE.

Manual status changes never create Jobs or claim agent leases. Unlinked items can be marked ACTIVE or PENDING_REVIEW and completed directly. Activation resumes an existing linked Job; submitting or completing linked work retains the Job readiness and review checks. Explicit intake creates the Job for an eligible unlinked entry. Agents still need an explicit intake request to claim unleased TODO/ACTIVE entries, then follow the Task claim/Plan/start workflow. Reopening preserves Task outcomes and does not rerun DONE Tasks. Archived work must be unarchived first; withdrawn entries cannot be revived. Cancelled entries must be reopened before completion, and completed entries before cancellation.

Schema 10 migrates legacy Inbox IN_PROGRESS/DONE values to ACTIVE/COMPLETED and restores pending review from the linked Job. The CLI accepts legacy names as aliases; synchronization updates receipts and checkbox symbols. Plain CLI sync retains cancellation/withdrawal import but does not replay other status changes from checkbox drift.
