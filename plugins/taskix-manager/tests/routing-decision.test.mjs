import assert from "node:assert/strict";
import { test } from "node:test";

const assignment = { project_id: "p", job_id: null };
const decision = { action: "followup", job_id: "job_a", revision: 7, inbox_ids: [], reason: "Same delivery", questions: [] };
const options = { session: "parent", cwd: "/work" };
const fresh = { id: "job_a", project_id: "p", revision: 7, status: "PENDING_REVIEW", archived_at: null };
const runner = async args => {
    assert.deepEqual(args, ["routing", "revision", "job_a"]);
    return { result: fresh };
};

test("classifier_result_produces_revision_guarded_followup_arguments", async () => {
    const { validateDecision } = await import("../routing-decision.mjs");
    const result = await validateDecision(decision, assignment, options, runner);
    assert.deepEqual(result.followup_args, ["job", "followup", "job_a", "--expect-revision", "7"]);
});

for (const [name, patch] of Object.entries({
    unknown_action: { action: "approve" }, unknown_id: { job_id: "--help" }, missing_revision: { revision: null },
    oversized_reason: { reason: "x".repeat(241) }, duplicate_inbox: { inbox_ids: ["inbox_a", "inbox_a"] },
    oversized_result: { extra: "x".repeat(1001) }, contradictory_discussion: { action: "discussion" },
})) test(`classifier_rejects_${name}_before_cli`, async () => {
    const { validateDecision } = await import("../routing-decision.mjs");
    await assert.rejects(validateDecision({ ...decision, ...patch }, assignment, options, () => assert.fail("invalid result must not query")), /Invalid classifier result/);
});

for (const [name, patch] of Object.entries({ revision: { revision: 8 }, status: { status: "ACTIVE" }, project: { project_id: "other" }, archived: { archived_at: 1 }, missing: null })) test(`classifier_rejects_changed_${name}`, async () => {
    const { validateDecision } = await import("../routing-decision.mjs");
    await assert.rejects(validateDecision(decision, assignment, options, async () => ({ result: patch === null ? null : { ...fresh, ...patch } })), /Routing evidence changed/);
});

test("classifier_preserves_parent_assignment_and_uncertainty", async () => {
    const { validateDecision } = await import("../routing-decision.mjs");
    await assert.rejects(validateDecision(decision, { ...assignment, job_id: "job_other" }, options, runner), /assignment/);
    const uncertain = { ...decision, action: "uncertain", job_id: null, revision: null, questions: ["Which delivery?"] };
    assert.deepEqual(await validateDecision(uncertain, assignment, options, () => assert.fail("no CLI for uncertain")), { decision: uncertain, followup_args: null });
});

for (const [name, patch] of Object.entries({ expired: { expires: 0 }, foreign_parent: { parent_session: "other" }, foreign_cwd: { cwd: "/other" }, version: { version: 2 } })) test(`snapshot_validator_rejects_${name}`, async () => {
    const { validateSnapshotDecision } = await import("../routing-decision.mjs");
    const packet = { version: 1, expires: Date.now() + 60000, parent_session: "parent", cwd: "/work", assignment, ...patch };
    await assert.rejects(validateSnapshotDecision(packet, decision, options, () => assert.fail("invalid snapshot must not query")), /Invalid routing snapshot/);
});

test("snapshot_validator_rechecks_live_parent_assignment", async () => {
    const { validateSnapshotDecision } = await import("../routing-decision.mjs");
    const packet = { version: 1, expires: Date.now() + 60000, parent_session: "parent", cwd: "/work", assignment };
    await assert.rejects(validateSnapshotDecision(packet, decision, options, async args => {
        assert.deepEqual(args, ["routing", "snapshot"]);
        return { result: { project_id: "p", job_id: "job_other" } };
    }), /assignment/);
});

test("snapshot_accepts_canonical_working_directory_alias", async t => {
    const { mkdtemp, realpath, rm } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    const dir = await mkdtemp(join(tmpdir(), "routing-cwd-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const { validateSnapshotDecision } = await import("../routing-decision.mjs");
    const packet = { version: 1, expires: Date.now() + 60000, parent_session: "parent", cwd: dir, assignment };
    const result = await validateSnapshotDecision(packet, decision, { ...options, cwd: await realpath(dir) }, async args => args[1] === "snapshot" ? { result: assignment } : runner(args));
    assert.equal(result.decision.job_id, "job_a");
});

for (const [name, patch] of Object.entries({
    consumed: { job_id: "job_other" }, deleted: { deleted: true },
    draft: { published: false }, pending: { content_pending: true },
    claimed: { lease: {} }, completed: { status: "DONE" }, missing: null,
})) test(`classifier_rejects_unavailable_inbox_${name}`, async () => {
    const { validateDecision } = await import("../routing-decision.mjs");
    const selected = { ...decision, inbox_ids: ["inbox_a"] };
    await assert.rejects(validateDecision(selected, assignment, options, async args => {
        if (args[0] === "routing") return runner(args);
        assert.deepEqual(args, ["inbox", "list", "--project", "p"]);
        return { result: patch === null ? [] : [{ id: "inbox_a", status: "TODO", published: true, ...patch }] };
    }), /Inbox evidence changed/);
});

test("classifier_preserves_all_available_inbox_matches_in_guarded_arguments", async () => {
    const { validateDecision } = await import("../routing-decision.mjs");
    const selected = { ...decision, inbox_ids: ["inbox_a", "inbox_b"] };
    const result = await validateDecision(selected, assignment, options, async args => args[0] === "routing" ? runner(args) : {
        result: selected.inbox_ids.map(id => ({ id, status: "TODO", published: true })),
    });
    assert.deepEqual(result.followup_args, ["job", "followup", "job_a", "--expect-revision", "7", "--inbox", "inbox_a", "--inbox", "inbox_b"]);
});
