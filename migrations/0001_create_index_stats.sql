-- Time series of pg_stat_all_indexes: one row per index per collection run.
-- Rows of the same run share `collected_at`. The counters are cumulative on the
-- server (reset by pg_stat_reset), so usage over a period is a difference
-- between snapshots.
CREATE TABLE index_stats (
    id            INTEGER PRIMARY KEY,
    collected_at  TEXT    NOT NULL,  -- UTC, ISO 8601
    relid         INTEGER NOT NULL,  -- table OID
    indexrelid    INTEGER NOT NULL,  -- index OID
    schemaname    TEXT    NOT NULL,
    relname       TEXT    NOT NULL,  -- table name
    indexrelname  TEXT    NOT NULL,  -- index name
    idx_scan      INTEGER NOT NULL,
    last_idx_scan TEXT,              -- UTC, ISO 8601; NULL if never scanned or unsupported (PostgreSQL < 16)
    idx_tup_read  INTEGER NOT NULL,
    idx_tup_fetch INTEGER NOT NULL
) STRICT;

-- OIDs change when an index is recreated, so name-based lookups are supported too.
CREATE INDEX index_stats_by_oid ON index_stats (indexrelid, collected_at);
CREATE INDEX index_stats_by_name ON index_stats (schemaname, relname, indexrelname, collected_at);
CREATE INDEX index_stats_by_time ON index_stats (collected_at);
