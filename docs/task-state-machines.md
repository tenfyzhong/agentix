# Task, Job and Issue state machines

SQLite enforces these transitions. The desktop Taskix Sync plugin submits saved Obsidian `status` edits through the same CLI; a rejected edit is restored and produces a notification. See [Obsidian setup and supported edits](https://github.com/tenfyzhong/agentix/wiki/Obsidian#status-edits).

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
    COMPLETED --> ACTIVE: task reopen
    ACTIVE --> CANCELLED: job cancel
    PENDING_REVIEW --> CANCELLED: job cancel
    note right of PENDING_REVIEW
        Verification is pending.
        An explicitly requested independent or associated Job may start.
    end note
    CANCELLED --> ACTIVE: task retry / reopen
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

PENDING_REVIEW is unfinished: it cannot be archived and blocks Project archival and submission of its associated Issue. Job approval and readiness never complete an Issue. COMPLETED and CANCELLED Jobs may be archived/unarchived; archival is an independent property. Historical COMPLETED Jobs remain completed after upgrades.

## Issue

```mermaid
stateDiagram-v2
    [*] --> TODO: issue create
    TODO --> ACTIVE: first successful associated Task start / human activation
    TODO --> PENDING_REVIEW: explicit submit
    ACTIVE --> PENDING_REVIEW: explicit submit
    PENDING_REVIEW --> ACTIVE: human reject with reason
    PENDING_REVIEW --> COMPLETED: human approve
    TODO --> CANCELLED: cancel
    ACTIVE --> CANCELLED: cancel
    PENDING_REVIEW --> CANCELLED: cancel
    COMPLETED --> TODO: explicit reopen
    CANCELLED --> TODO: explicit reopen
```

Submission and approval require every associated Job to be COMPLETED or CANCELLED. Requirement, Acceptance Criteria and Notes are optional; a title-only Issue can be submitted. Status transitions do not inspect authored prose. Approval rechecks readiness and requires a human actor. Zero-Job Issues can follow the same offline acceptance workflow. A Job ending never completes or resets its Issue. Cancellation revokes unfinished Jobs/Tasks and their leases while preserving delivered outcomes. Reopening preserves history. There is no Issue lease; several Jobs can implement one Issue. The Task state machine above remains unchanged. See [Project Issue design](project-issues.md).
