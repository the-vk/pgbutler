-- Index kind flags, used to exclude indexes that must not be dropped
-- (primary key, unique, or supporting a foreign key) from recommendations.
-- Rows recorded before this migration default to 0 (unknown / plain index).
ALTER TABLE index_stats ADD COLUMN is_primary INTEGER NOT NULL DEFAULT 0;
ALTER TABLE index_stats ADD COLUMN is_unique INTEGER NOT NULL DEFAULT 0;
ALTER TABLE index_stats ADD COLUMN supports_foreign_key INTEGER NOT NULL DEFAULT 0;
