-- Taskix Jev metrics protocol v1. See metrics-schema.md.
PRAGMA application_id=0x544A4556;
PRAGMA user_version=1;

CREATE TABLE requests (
    id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, session_id TEXT, turn_id TEXT,
    project_id TEXT, model TEXT NOT NULL, threshold REAL NOT NULL,
    duration_ms REAL NOT NULL, called INTEGER NOT NULL, accepted INTEGER NOT NULL,
    action TEXT NOT NULL, reason TEXT, review TEXT CHECK(review IN ('correct','incorrect')),
    answer_count INTEGER NOT NULL
);

CREATE TABLE answers (
    request_id TEXT NOT NULL REFERENCES requests(id), question TEXT NOT NULL, subject_id TEXT,
    choice TEXT, confidence REAL, probability REAL, margin REAL,
    valid INTEGER NOT NULL, issue TEXT, PRIMARY KEY(request_id, question)
);
