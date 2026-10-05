-- One-time compatibility backfill for existing schema 1-3 databases.
-- Run inside the store migration transaction before ID migration.
INSERT OR IGNORE INTO memory_projection(memory_id) SELECT id FROM memories;
INSERT OR IGNORE INTO memory_compactions(memory_id,project_id,revision,dirty_at)
    SELECT id,project_id,revision,coalesce(json_extract(data,'$.updated_at'),unixepoch()) FROM memories;

-- Older databases can contain a trigger without the revision fence.
DROP TRIGGER IF EXISTS compaction_failure;
CREATE TRIGGER compaction_failure AFTER UPDATE OF state ON work_items WHEN new.state='failed' BEGIN
    UPDATE memory_compactions SET suspended=1 WHERE work_id=new.id AND revision=json_extract(new.payload,'$.compact.revision');
END;
DROP INDEX IF EXISTS compaction_dirty;
DROP TRIGGER IF EXISTS compaction_insert;
DROP TRIGGER IF EXISTS compaction_update;
CREATE TRIGGER IF NOT EXISTS compaction_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memory_compactions(memory_id,project_id,revision,dirty,dirty_at) VALUES(new.id,new.project_id,new.revision,(new.status IN ('active','conflicted') AND coalesce(json_extract(new.data,'$.actor')='agent',0)),coalesce(json_extract(new.data,'$.updated_at'),unixepoch()));
END;
CREATE TRIGGER IF NOT EXISTS compaction_update AFTER UPDATE OF revision ON memories BEGIN
    UPDATE memory_compactions SET revision=new.revision,dirty=(new.status IN ('active','conflicted') AND coalesce(json_extract(new.data,'$.actor')='agent',0)),suspended=0,dirty_at=coalesce(json_extract(new.data,'$.updated_at'),unixepoch()) WHERE memory_id=new.id;
END;
UPDATE memory_compactions SET dirty=0 WHERE memory_id IN (SELECT id FROM memories WHERE status NOT IN ('active','conflicted') OR json_extract(data,'$.actor')<>'agent');
INSERT OR REPLACE INTO memory_metadata(key,value) VALUES('memory_auxiliary_version','1');
