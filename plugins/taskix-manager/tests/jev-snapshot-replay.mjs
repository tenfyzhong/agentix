import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";

// Replay existing historical records through the production SQL, without
// changing the real Taskix database or duplicating its text truncation rules.
const source = readFileSync(new URL("../../../crates/agentix-task/src/routing.rs", import.meta.url), "utf8");
const query = name => {
    const value = source.match(new RegExp(`const ${name}: &str = "([\\s\\S]*?)";`))?.[1];
    if (!value) throw new Error(`Missing production routing query: ${name}`);
    return value.replaceAll("?1", "?");
};
const jobsQuery = query("JOBS"), tasksQuery = query("TASKS");

export function boundedSnapshot(context) {
    const db = new DatabaseSync(":memory:");
    try {
        db.exec("CREATE TABLE jobs(id TEXT PRIMARY KEY,project_id TEXT,data TEXT); CREATE TABLE tasks(id TEXT PRIMARY KEY,job_id TEXT,data TEXT)");
        const insertJob = db.prepare("INSERT INTO jobs VALUES (?,?,?)");
        const insertTask = db.prepare("INSERT INTO tasks VALUES (?,?,?)");
        for (const { job, tasks } of context.routing.candidates) {
            insertJob.run(job.id, job.project_id, JSON.stringify(job));
            for (const task of tasks) insertTask.run(task.id, task.job_id, JSON.stringify(task));
        }
        const rows = db.prepare(jobsQuery).all(context.project_id).map(row => JSON.parse(Object.values(row)[0]));
        let complete = context.routing.complete === true && rows.length <= 32;
        const candidates = rows.slice(0, 32).map(job => {
            complete &&= job.truncated === 0;
            delete job.truncated;
            return { job, tasks: [] };
        });
        const tasks = db.prepare(tasksQuery).all(JSON.stringify(candidates.map(c => c.job.id)))
            .map(row => JSON.parse(Object.values(row)[0]));
        complete &&= tasks.length <= 256;
        for (const task of tasks.slice(0, 256)) {
            complete &&= task.truncated === 0;
            delete task.truncated;
            candidates.find(c => c.job.id === task.job_id).tasks.push(task);
        }
        return { ...context, routing: { complete, candidates } };
    } finally { db.close(); }
}
