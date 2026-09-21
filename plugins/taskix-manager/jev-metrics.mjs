import { Worker } from "node:worker_threads";
import { randomUUID } from "node:crypto";
import { mkdir, open, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export function metricsPath(env = process.env) {
    return env.TASKIX_JEV_METRICS_DB?.trim() || join(env.XDG_STATE_HOME || join(homedir(), ".local", "state"), "taskix", "jev-metrics.sqlite");
}

export function answerScore(question, answer, criteria, threshold) {
    const keys = Object.keys(criteria), probabilities = answer?.probabilities;
    const known = keys.includes(answer?.choice);
    const valid = answer?.type === "choice" && known && Number.isFinite(answer.confidence) && answer.confidence >= 0 && answer.confidence <= 1 &&
        probabilities && Object.keys(probabilities).length === keys.length &&
        keys.every(k => Number.isFinite(probabilities[k]) && probabilities[k] >= 0 && probabilities[k] <= 1) &&
        Math.abs(keys.reduce((sum, k) => sum + probabilities[k], 0) - 1) <= .001;
    const probability = valid ? probabilities[answer.choice] : null;
    const margin = valid ? probability - Math.max(...keys.filter(k => k !== answer.choice).map(k => probabilities[k])) : null;
    const issue = !valid ? "invalid_answer" : answer.choice === "uncertain" ? "uncertain_choice" :
        answer.confidence < threshold || probability < threshold ? "low_confidence" : margin < .2 ? "small_margin" : null;
    return { question, choice: known ? answer.choice : null, confidence: valid ? answer.confidence : null, probability, margin, valid: valid ? 1 : 0, issue };
}

export async function measuredRouting({ env, options, config }, callback) {
    const metric = { id: randomUUID(), started_at: Date.now(), called: false, answers: [], outcome: { action: "agent", reason: "preparation_unavailable" } };
    const start = performance.now();
    try { return await callback(metric); }
    finally {
        metric.duration_ms = performance.now() - start;
        // Snapshot values now: a timed-out operation must not mutate an already
        // finalized event while asynchronous filesystem setup is in progress.
        const event = structuredClone(metric);
        await writeMetricBounded({ path: metricsPath(env),
            options: { session: options?.session, turn_id: options?.turn_id },
            config: { model: config.model, threshold: config.threshold }, metric: event });
    }
}

export async function writeMetric({ path, options, config, metric }) {
    let db;
    try {
        await mkdir(dirname(path), { recursive: true, mode: 0o700 });
        const file = await open(path, "a", 0o600); await file.close();
        const { DatabaseSync } = await import("node:sqlite");
        db = new DatabaseSync(path);
        db.exec("PRAGMA busy_timeout=25; BEGIN IMMEDIATE");
        const version = db.prepare("PRAGMA user_version").get().user_version;
        const application = db.prepare("PRAGMA application_id").get().application_id;
        const empty = version === 0 && application === 0 &&
            db.prepare("SELECT count(*) AS count FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'").get().count === 0;
        if (empty) {
            db.exec(await readFile(new URL("./metrics-schema.sql", import.meta.url), "utf8"));
        } else if (version !== 1 || application !== 0x544a4556) {
            db.exec("ROLLBACK");
            return "unsupported_schema";
        }
        db.prepare("INSERT INTO requests (id, started_at, session_id, turn_id, project_id, model, threshold, duration_ms, called, accepted, action, reason, review, answer_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)").run(
            metric.id, metric.started_at, options?.session ?? null, options?.turn_id ?? null, metric.project_id ?? null,
            config.model, config.threshold, metric.duration_ms, Number(metric.called),
            Number(metric.outcome.action !== "agent"), metric.outcome.action,
            metric.outcome.reason ?? null, metric.answers.length);
        const insert = db.prepare("INSERT INTO answers (request_id, question, subject_id, choice, confidence, probability, margin, valid, issue) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)");
        for (const answer of metric.answers) insert.run(metric.id, answer.question, answer.subject_id ?? null, answer.choice, answer.confidence, answer.probability, answer.margin, answer.valid, answer.issue);
        db.exec("COMMIT");
        return true;
    } catch {
        try { db?.exec("ROLLBACK"); } catch { /* No open transaction. */ }
        // Metrics are best-effort: never change routing or expose raw errors.
        return false;
    } finally { try { db?.close(); } catch { /* Do not affect routing. */ } }
}

// Bound parent waiting, including worker startup and filesystem operations.
// SQLite runs off the host event loop. A timeout drops the observation, never routing.
export function writeMetricBounded(payload, { createWorker = data => new Worker(new URL("./jev-metrics-worker.mjs", import.meta.url), { workerData: data, env: {}, stdout: true, stderr: true }) } = {}) {
    return new Promise(resolve => {
        let worker, timer, settled = false;
        const finish = value => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            if (value !== true) process.stderr.write(value === "unsupported_schema"
                ? "Taskix: Unsupported Jev metrics schema; write skipped. Use a compatible version or a new TASKIX_JEV_METRICS_DB path.\n"
                : "Taskix: Jev metrics write skipped.\n");
            worker?.unref();
            // Do not await a worker blocked in an OS operation.
            void worker?.terminate().catch(() => {});
            resolve(value === true);
        };
        timer = setTimeout(() => finish(false), 250);
        try {
            worker = createWorker(payload);
            worker.stdout?.resume();
            worker.stderr?.resume();
            worker.once("message", finish);
            worker.once("error", () => finish(false));
            worker.once("exit", () => finish(false));
        } catch { finish(false); }
    });
}
