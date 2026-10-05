CREATE TABLE IF NOT EXISTS sources (
    receipt_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    data TEXT NOT NULL CHECK(json_valid(data))
);
CREATE INDEX IF NOT EXISTS sources_by_project ON sources(project_id,receipt_id);
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    status TEXT NOT NULL,
    data TEXT NOT NULL CHECK(json_valid(data)),
    valid_until INTEGER GENERATED ALWAYS AS (json_extract(data,'$.content.valid_until')) STORED
);
CREATE INDEX IF NOT EXISTS memories_by_project ON memories(project_id,status,id);
CREATE INDEX IF NOT EXISTS memories_by_project_id ON memories(project_id,id);
CREATE TABLE IF NOT EXISTS memory_versions (
    memory_id TEXT NOT NULL REFERENCES memories(id),
    revision INTEGER NOT NULL,
    data TEXT NOT NULL CHECK(json_valid(data)),
    PRIMARY KEY(memory_id,revision)
);
CREATE TABLE IF NOT EXISTS suppressions (
    project_id TEXT NOT NULL,
    evidence_key TEXT NOT NULL,
    memory_id TEXT NOT NULL REFERENCES memories(id),
    PRIMARY KEY(project_id,evidence_key)
);
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    project_token, title, body, tags, scope, tokenize='unicode61'
);
CREATE TABLE IF NOT EXISTS memory_metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS embedding_profiles (
    project_id TEXT PRIMARY KEY,
    generation INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    dimensions INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_vectors (
    memory_id TEXT NOT NULL REFERENCES memories(id),
    project_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    vector BLOB NOT NULL,
    PRIMARY KEY(memory_id,generation)
);
CREATE INDEX IF NOT EXISTS vectors_by_project ON memory_vectors(project_id,generation,memory_id);
CREATE TABLE IF NOT EXISTS embedding_failures (
    memory_id TEXT NOT NULL REFERENCES memories(id),
    generation INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    attempts INTEGER NOT NULL,
    available_at INTEGER NOT NULL,
    error TEXT NOT NULL,
    PRIMARY KEY(memory_id,generation,revision)
);
INSERT OR IGNORE INTO memory_metadata(key,value) VALUES ('tokenizer_version','1');
PRAGMA application_id=0x41584d4d;

CREATE TABLE IF NOT EXISTS source_heads (
    instance_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    receipt_id TEXT NOT NULL REFERENCES sources(receipt_id),
    PRIMARY KEY(instance_id,session_id,turn_id)
);
CREATE TABLE IF NOT EXISTS work_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    receipt_id TEXT NOT NULL REFERENCES sources(receipt_id),
    kind TEXT NOT NULL CHECK(kind IN ('extract','consolidate')),
    state TEXT NOT NULL DEFAULT 'pending',
    payload TEXT NOT NULL CHECK(json_valid(payload)),
    result TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    available_at INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER,
    generation INTEGER NOT NULL DEFAULT 0,
    owner TEXT,
    lease_until INTEGER,
    error TEXT
);
CREATE INDEX IF NOT EXISTS work_ready ON work_items(state,available_at,project_id,priority,id);
CREATE INDEX IF NOT EXISTS work_running ON work_items(project_id,kind,lease_until) WHERE state='running';
CREATE INDEX IF NOT EXISTS work_source ON work_items(receipt_id,state);
CREATE TABLE IF NOT EXISTS scheduler_projects (project_id TEXT PRIMARY KEY,last_served INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO memory_metadata(key,value) VALUES ('scheduler_tick','0');
CREATE TABLE IF NOT EXISTS work_audits (
    work_id INTEGER PRIMARY KEY REFERENCES work_items(id),
    generation INTEGER NOT NULL,
    data TEXT NOT NULL CHECK(json_valid(data))
);
CREATE TABLE IF NOT EXISTS context_deliveries (
    project_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(project_id,session_id,memory_id)
);
CREATE INDEX IF NOT EXISTS context_deliveries_expiry ON context_deliveries(updated_at);
CREATE TABLE IF NOT EXISTS context_receipts (
    project_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    data TEXT NOT NULL CHECK(json_valid(data)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(project_id,session_id,turn_id)
);
CREATE INDEX IF NOT EXISTS context_receipts_expiry ON context_receipts(updated_at);

CREATE TABLE IF NOT EXISTS memory_projection (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id),
    published_revision INTEGER NOT NULL DEFAULT 0,
    published_hash TEXT NOT NULL DEFAULT '',
    imported_hash TEXT NOT NULL DEFAULT '',
    prepared_revision INTEGER NOT NULL DEFAULT 0,
    prepared_hash TEXT NOT NULL DEFAULT '',
    error TEXT
);

CREATE INDEX IF NOT EXISTS sources_by_sequence ON sources(json_extract(data,'$.sequence'));
CREATE INDEX IF NOT EXISTS sources_by_conversation ON sources(project_id,instance_id,json_extract(data,'$.session_id'),json_extract(data,'$.sequence'));

CREATE TABLE IF NOT EXISTS memory_reviews (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id),
    revision INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    work_id INTEGER NOT NULL REFERENCES work_items(id)
);

CREATE TABLE IF NOT EXISTS source_turns (
    project_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    first_sequence INTEGER NOT NULL,
    receipt_id TEXT NOT NULL REFERENCES sources(receipt_id),
    revision INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    PRIMARY KEY(instance_id,session_id,turn_id)
);
CREATE INDEX IF NOT EXISTS source_turns_by_order ON source_turns(project_id,instance_id,session_id,first_sequence);
CREATE TABLE IF NOT EXISTS work_retention (
    work_id INTEGER PRIMARY KEY REFERENCES work_items(id) ON DELETE CASCADE,
    observed_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS work_retention_age ON work_retention(observed_at,work_id);
CREATE INDEX IF NOT EXISTS work_cancelled ON work_items(id) WHERE state='cancelled';
CREATE INDEX IF NOT EXISTS memory_reviews_by_work ON memory_reviews(work_id);

CREATE TABLE IF NOT EXISTS memory_compactions (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    dirty INTEGER NOT NULL DEFAULT 1,
    dirty_at INTEGER NOT NULL,
    checked_at INTEGER NOT NULL DEFAULT 0,
    suspended INTEGER NOT NULL DEFAULT 0,
    work_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS compaction_ready ON memory_compactions(project_id,dirty_at,memory_id) WHERE dirty=1 AND suspended=0;
CREATE TRIGGER IF NOT EXISTS compaction_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memory_compactions(memory_id,project_id,revision,dirty,dirty_at) VALUES(new.id,new.project_id,new.revision,(new.status IN ('active','conflicted') AND coalesce(json_extract(new.data,'$.actor')='agent',0)),coalesce(json_extract(new.data,'$.updated_at'),unixepoch()));
END;
CREATE TRIGGER IF NOT EXISTS compaction_update AFTER UPDATE OF revision ON memories BEGIN
    UPDATE memory_compactions SET revision=new.revision,dirty=(new.status IN ('active','conflicted') AND coalesce(json_extract(new.data,'$.actor')='agent',0)),suspended=0,dirty_at=coalesce(json_extract(new.data,'$.updated_at'),unixepoch()) WHERE memory_id=new.id;
END;
CREATE TRIGGER IF NOT EXISTS compaction_failure AFTER UPDATE OF state ON work_items WHEN new.state='failed' BEGIN
    UPDATE memory_compactions SET suspended=1 WHERE work_id=new.id AND revision=json_extract(new.payload,'$.compact.revision');
END;

CREATE TABLE IF NOT EXISTS memory_id_renames (
    old_id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memories(id),
    projection_pending INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS memory_id_renames_by_memory ON memory_id_renames(memory_id);

CREATE TABLE IF NOT EXISTS memory_facts (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    fact_key TEXT NOT NULL,
    status TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS fact_lookup ON memory_facts(project_id,fact_key,status,memory_id);
CREATE UNIQUE INDEX IF NOT EXISTS fact_active ON memory_facts(project_id,fact_key) WHERE status='active';

CREATE TABLE IF NOT EXISTS memory_fact_origins (
    memory_id TEXT NOT NULL REFERENCES memories(id),
    source_memory_id TEXT NOT NULL REFERENCES memories(id),
    source_revision INTEGER NOT NULL,
    PRIMARY KEY(memory_id,source_memory_id,source_revision)
);
CREATE INDEX IF NOT EXISTS fact_origins_by_source ON memory_fact_origins(source_memory_id,memory_id);
