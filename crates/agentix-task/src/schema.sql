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
CREATE TABLE IF NOT EXISTS inbox_entries (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (json_valid(data)),
    project_id TEXT GENERATED ALWAYS AS (json_extract(data, '$.project_id')) STORED REFERENCES projects(id)
);
CREATE INDEX IF NOT EXISTS inbox_by_project ON inbox_entries(project_id);
CREATE INDEX IF NOT EXISTS projects_by_name ON projects(json_extract(data, '$.name'));
CREATE INDEX IF NOT EXISTS tasks_by_project ON tasks(json_extract(data, '$.project_id'));
CREATE INDEX IF NOT EXISTS tasks_by_project_activity ON tasks(json_extract(data, '$.project_id'), json_extract(data, '$.updated_at'));
CREATE INDEX IF NOT EXISTS jobs_by_project_activity ON jobs(project_id, json_extract(data, '$.updated_at'));
CREATE INDEX IF NOT EXISTS tasks_by_job_status ON tasks(job_id, json_extract(data, '$.status'), id);
CREATE INDEX IF NOT EXISTS tasks_by_project_name ON tasks(json_extract(data, '$.project_id'), id, json_extract(data, '$.name'));
CREATE INDEX IF NOT EXISTS jobs_by_project_name ON jobs(project_id, id, json_extract(data, '$.name'));
CREATE INDEX IF NOT EXISTS tasks_by_project_day_sequence ON tasks(json_extract(data, '$.project_id'), CAST(json_extract(data, '$.created_at')/86400 AS INTEGER), json_extract(data, '$.sequence'));
CREATE INDEX IF NOT EXISTS jobs_by_project_day_sequence ON jobs(project_id, CAST(json_extract(data, '$.created_at')/86400 AS INTEGER), json_extract(data, '$.sequence'));
CREATE INDEX IF NOT EXISTS tasks_by_session ON tasks(json_extract(data, '$.last_session'));
CREATE INDEX IF NOT EXISTS tasks_by_session_job_activity ON tasks(json_extract(data, '$.last_session'), job_id, json_extract(data, '$.updated_at'));
CREATE INDEX IF NOT EXISTS tasks_by_job_session_activity ON tasks(job_id, json_extract(data, '$.updated_at')) WHERE json_extract(data, '$.last_session') IS NOT NULL;
CREATE INDEX IF NOT EXISTS leases_by_session ON task_leases(session_ref);
CREATE INDEX IF NOT EXISTS jobs_by_session ON jobs(json_extract(data, '$.session_id'));
CREATE INDEX IF NOT EXISTS inbox_by_job ON inbox_entries(json_extract(data, '$.job_id'));
CREATE INDEX IF NOT EXISTS inbox_by_session ON inbox_entries(json_extract(data, '$.last_session'));
CREATE INDEX IF NOT EXISTS leases_by_expiry ON task_leases(json_extract(data, '$.lease_expires_at'));
CREATE INDEX IF NOT EXISTS inbox_by_expiry ON inbox_entries(json_extract(data, '$.lease.lease_expires_at'));
CREATE INDEX IF NOT EXISTS dependencies_by_dependency ON task_dependencies(dependency_id, task_id);
CREATE INDEX IF NOT EXISTS events_by_project ON task_events(json_extract(data, '$.project_id'), sequence);
CREATE TABLE IF NOT EXISTS document_registry (
    key TEXT PRIMARY KEY,
    path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS documents_by_path ON document_registry(path);
CREATE TABLE IF NOT EXISTS pending_documents (
    key TEXT PRIMARY KEY,
    generation TEXT NOT NULL
);
PRAGMA user_version = 12;
PRAGMA application_id = 0x4158544b;
