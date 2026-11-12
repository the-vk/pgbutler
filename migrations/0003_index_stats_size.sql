-- Estimated tuple count (pg_class.reltuples, 0 if never analyzed) and on-disk
-- size (pg_relation_size) of each index at collection time.
ALTER TABLE index_stats ADD COLUMN index_tuples INTEGER NOT NULL DEFAULT 0;
ALTER TABLE index_stats ADD COLUMN index_size_bytes INTEGER NOT NULL DEFAULT 0;
