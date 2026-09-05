CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (json_valid(data)),
    root TEXT GENERATED ALWAYS AS (json_extract(data, '$.root')) STORED UNIQUE
);
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (json_valid(data)),
    project_id TEXT GENERATED ALWAYS AS (json_extract(data, '$.project_id')) STORED REFERENCES projects(id)
);
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (json_valid(data)),
    job_id TEXT GENERATED ALWAYS AS (json_extract(data, '$.job_id')) STORED REFERENCES jobs(id)
);
CREATE TABLE IF NOT EXISTS plans (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (json_valid(data)),
    task_id TEXT GENERATED ALWAYS AS (json_extract(data, '$.task_id')) STORED REFERENCES tasks(id),
    version INTEGER GENERATED ALWAYS AS (json_extract(data, '$.version')) STORED,
    UNIQUE(task_id, version)
);
CREATE TABLE IF NOT EXISTS task_leases (
    id TEXT PRIMARY KEY REFERENCES tasks(id),
    data TEXT NOT NULL CHECK (json_valid(data)),
    executor_ref TEXT GENERATED ALWAYS AS (json_extract(data, '$.executor_ref')) STORED,
    session_ref TEXT GENERATED ALWAYS AS (json_extract(data, '$.session_ref')) STORED,
    UNIQUE(executor_ref, session_ref)
);
CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    dependency_id TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY(task_id, dependency_id),
    CHECK(task_id != dependency_id)
);
CREATE TABLE IF NOT EXISTS task_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    job_id TEXT,
    data TEXT NOT NULL CHECK (json_valid(data))
);
CREATE INDEX IF NOT EXISTS events_by_job ON task_events(job_id, sequence);
CREATE INDEX IF NOT EXISTS tasks_by_job ON tasks(job_id);
CREATE INDEX IF NOT EXISTS jobs_by_project ON jobs(project_id);
CREATE TABLE IF NOT EXISTS idempotency_keys (
    key TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    result TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projection_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS document_deletions (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (json_valid(data))
);
PRAGMA user_version = 6;
PRAGMA application_id = 0x4158544b;
