import test from "node:test";
import assert from "node:assert/strict";
import { fixture, copy } from "./support/obsidian-plugin.mjs";

async function jobFixture(t, linked = false) {
    const f = await fixture(); t.after(() => f.engine.dispose());
    Object.assign(f.row, { kind: "job", id: "job_one", status: "ACTIVE" });
    Object.assign(f.row.properties, { status: "ACTIVE", issue_id: linked ? "issue_old" : null,
        issue: linked ? "[[Tasks/Projects/Demo/Issues/Old]]" : null });
    Object.assign(f.files.get(f.row.path), { ...f.row.properties, id: f.row.id });
    f.io.resolveLink = (link) => ({ id: "issue_new", path: "Tasks/Projects/Demo/Issues/New.md" });
    const lookup = f.io.lookup;
    f.io.lookup = async id => id === "issue_new" ? { kind: "issue", id, project_id: "prj_one", path: "Tasks/Projects/Demo/Issues/New.md" } : lookup(id);
    await f.engine.initialize();
    return f;
}

for (const linked of [false, true]) test(`Job Issue link ${linked ? "replaces" : "creates"} association with revision guard`, async t => {
    const f = await jobFixture(t, linked);
    f.io.execute = async args => {
        f.calls.push(copy(args));
        Object.assign(f.row.properties, { issue_id: "issue_new", issue: "[[Tasks/Projects/Demo/Issues/New|New]]" });
        f.row.revision++;
        return { result: { id: f.row.id, revision: f.row.revision } };
    };
    const file = f.files.get(f.row.path);
    file.issue = "[[Tasks/Projects/Demo/Issues/New]]";
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls.length, 1);
    assert.deepEqual(f.calls[0].slice(0, 5), ["job", "update", "job_one", "--issue", "issue_new"]);
    assert.ok(f.calls[0].includes("--expect-revision"));
    assert.equal(file.issue_id, "issue_new");
    assert.equal(f.notices.length, 0);
});

test("clearing the Job Issue link unlinks without changing status", async t => {
    const f = await jobFixture(t, true);
    f.io.execute = async args => {
        f.calls.push(copy(args)); Object.assign(f.row.properties, { issue_id: null, issue: null });
        f.row.revision++; return { result: { id: f.row.id, revision: f.row.revision } };
    };
    const file = f.files.get(f.row.path); file.issue = "";
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls.length, 1); assert.ok(f.calls[0].includes("--unlink-issue"));
    assert.equal(file.issue_id, null); assert.equal(file.status, "ACTIVE");
});

test("foreign or missing Job Issue links are rejected and restored", async t => {
    const f = await jobFixture(t, true);
    const lookup = f.io.lookup;
    f.io.lookup = async id => id === "issue_new" ? { kind: "issue", id, project_id: "prj_other", path: "Tasks/Projects/Demo/Issues/New.md" } : lookup(id);
    const file = f.files.get(f.row.path); file.issue = "[[Other]]";
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls.length, 0); assert.equal(file.issue, f.row.properties.issue);
    assert.match(f.notices.at(-1), /another Project/);
});

test("rejected Job Issue updates restore the old link and report the error", async t => {
    const f = await jobFixture(t, true);
    f.io.execute = async args => { f.calls.push(copy(args)); throw new Error("The taskix revision changed"); };
    const file = f.files.get(f.row.path); file.issue = null;
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls.length, 1); assert.equal(file.issue, f.row.properties.issue);
    assert.match(f.notices.at(-1), /revision changed/);
});

test("a copied Issue document cannot be associated by its duplicated ID", async t => {
    const f = await jobFixture(t);
    f.io.resolveLink = () => ({ id: "issue_new", path: "Tasks/Projects/Demo/Issues/Copy.md" });
    const file = f.files.get(f.row.path); file.issue = "[[Copy]]";
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls.length, 0); assert.equal(file.issue, null);
    assert.match(f.notices.at(-1), /copied/);
});

test("newer Job projections do not replay Issue links as user edits", async t => {
    const f = await jobFixture(t);
    f.row.revision++;
    Object.assign(f.row.properties, { issue_id: "issue_new", issue: "[[New]]" });
    const file = f.files.get(f.row.path);
    Object.assign(file, f.row.properties, { revision: f.row.revision });
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls.length, 0); assert.equal(file.issue_id, "issue_new");
});

test("Job revision conflicts reject association before resolving the target", async t => {
    const f = await jobFixture(t, true);
    const file = f.files.get(f.row.path); file.issue = "[[New]]";
    f.engine.observe(f.row.path, copy(file)); f.row.revision++;
    await f.engine.flush();
    assert.equal(f.calls.length, 0); assert.equal(file.issue, f.row.properties.issue);
    assert.match(f.notices.at(-1), /revision changed/);
});
